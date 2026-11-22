//! The PostgreSQL [`Store`], backed by [`lacrymira_pg::PgExecutor`].
//!
//! Model reads/writes go through the public `Command`/`Reply` contract
//! (with a small name-based reorder helper) rather than the executor's
//! `find_*`/`insert` helpers: as of lacrymira master 5ac4d56 the helper
//! layer decodes by name without a `TIMESTAMPTZ` arm, which breaks every
//! entity that carries a `jiff::Timestamp`. The `Reply::Rows` path
//! decodes `TIMESTAMPTZ` correctly, so this stays on it.

use i0721_proto::{FileRecord, NewFile, UserRecord};
use jiff::Timestamp;
use lacrymira::{Command, Condition, Entity, Reply, TypedColumnOps, Value};
use lacrymira_pg::{MigrationResult, MigrationRunner, PgExecutor};
use uuid::Uuid;

use crate::entity::{
    file_model_from_new, file_model_to_record, user_model_from_new, user_model_to_record,
    FileColumn, FileModel, OAuthIdentityColumn, OAuthIdentityModel, SettingColumn, SettingModel,
    UserColumn, UserModel,
};
use crate::{migrations, CreateUser, Store, StoreError};

/// A [`Store`] over a PostgreSQL connection pool.
#[derive(Debug, Clone)]
pub struct PgStore {
    executor: PgExecutor,
}

impl PgStore {
    /// Connect to `url` (verified eagerly).
    ///
    /// # Errors
    ///
    /// [`StoreError::Db`] on a bad URL, unreachable server or failed
    /// authentication.
    pub async fn connect(url: &str) -> Result<Self, StoreError> {
        Ok(Self {
            executor: PgExecutor::connect(url).await?,
        })
    }

    /// Run [`crate::migrations`] to completion (idempotent).
    ///
    /// # Errors
    ///
    /// [`StoreError::Db`] when a migration fails to apply.
    pub async fn migrate(&self) -> Result<MigrationResult, StoreError> {
        MigrationRunner::new(migrations())
            .up(self.executor.pool())
            .await
            .map_err(StoreError::from)
    }

    /// Execute a rows-producing command and decode every row into the
    /// entity's model (columns re-ordered by name).
    async fn query_all<E: Entity>(&self, cmd: Command) -> Result<Vec<E::Model>, StoreError> {
        let reply = self.executor.execute(&cmd).await?;
        let Reply::Rows { columns, rows } = reply else {
            return Err(StoreError::Db(format!(
                "expected rows from a {} command on {}",
                command_kind(&cmd),
                E::TABLE
            )));
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let mut values = vec![Value::Null; E::COLUMN_COUNT];
            for (index, name) in E::COLUMNS.iter().enumerate() {
                let position = columns
                    .iter()
                    .position(|column| column == name)
                    .ok_or_else(|| {
                        StoreError::Db(format!(
                            "column {name:?} missing from the {} result",
                            E::TABLE
                        ))
                    })?;
                values[index] = row[position].clone();
            }
            out.push(E::from_values(&values).ok_or_else(|| {
                StoreError::Db(format!(
                    "a {} row failed to decode into the model",
                    E::TABLE
                ))
            })?);
        }
        Ok(out)
    }

    /// Execute and return the first decoded row, if any.
    async fn query_one<E: Entity>(&self, cmd: Command) -> Result<Option<E::Model>, StoreError> {
        Ok(self.query_all::<E>(cmd).await?.into_iter().next())
    }
}

fn command_kind(cmd: &Command) -> &'static str {
    match cmd {
        Command::Select { .. } => "SELECT",
        Command::Insert { .. } => "INSERT",
        Command::Update { .. } => "UPDATE",
        Command::Delete { .. } => "DELETE",
        Command::Count { .. } => "COUNT",
        Command::Sql { .. } => "SQL",
        Command::Kv { .. } => "KV",
    }
}

fn insert_command<E: Entity>(model: &E::Model) -> Command {
    Command::Insert {
        table: E::TABLE.to_owned(),
        columns: E::COLUMNS
            .iter()
            .map(|column| (*column).to_owned())
            .collect(),
        rows: vec![E::to_values(model)],
        on_conflict: None,
    }
}

fn update_command<E: Entity>(
    sets: Vec<(&str, Value)>,
    filter: Condition,
) -> Result<Command, StoreError> {
    let mut seen = std::collections::HashSet::new();
    for (column, _) in &sets {
        if !seen.insert(*column) {
            return Err(StoreError::Db(format!("duplicate SET column {column:?}")));
        }
    }
    Ok(Command::Update {
        table: E::TABLE.to_owned(),
        sets: sets
            .into_iter()
            .map(|(column, value)| (column.to_owned(), value))
            .collect(),
        filter: Some(filter),
    })
}

fn user_by_pk(id: Uuid) -> Command {
    Command::Select {
        table: UserModel::TABLE.to_owned(),
        filter: Some(UserColumn::Id.eq(id).into_inner()),
        order_by: vec![],
        limit: None,
        offset: None,
        lock: None,
    }
}

fn live_file_by(column: FileColumn, value: impl Into<Value>) -> Command {
    Command::Select {
        table: FileModel::TABLE.to_owned(),
        filter: Some(
            FileColumn::DeletedAt
                .is_null()
                .into_inner()
                .and(column.eq(value).into_inner()),
        ),
        order_by: vec![],
        limit: None,
        offset: None,
        lock: None,
    }
}

#[async_trait::async_trait]
impl Store for PgStore {
    async fn list_users(&self, offset: u64, limit: u64) -> Result<Vec<UserRecord>, StoreError> {
        let cmd = Command::Select {
            table: UserModel::TABLE.to_owned(),
            filter: None,
            order_by: vec![("created_at".to_owned(), false)],
            limit: Some(limit),
            offset: Some(offset),
            lock: None,
        };
        self.query_all::<UserModel>(cmd)
            .await?
            .into_iter()
            .map(user_model_to_record)
            .collect()
    }

    async fn count_users(&self) -> Result<u64, StoreError> {
        let count = self.executor.count::<UserModel>(None).await?;
        Ok(count as u64)
    }

    async fn get_user(&self, id: Uuid) -> Result<Option<UserRecord>, StoreError> {
        self.query_one::<UserModel>(user_by_pk(id))
            .await?
            .map(user_model_to_record)
            .transpose()
    }

    async fn get_user_by_username(&self, username: &str) -> Result<Option<UserRecord>, StoreError> {
        let cmd = Command::Select {
            table: UserModel::TABLE.to_owned(),
            filter: Some(UserColumn::Username.eq(username).into_inner()),
            order_by: vec![],
            limit: None,
            offset: None,
            lock: None,
        };
        self.query_one::<UserModel>(cmd)
            .await?
            .map(user_model_to_record)
            .transpose()
    }

    async fn create_user(&self, user: CreateUser) -> Result<UserRecord, StoreError> {
        if self.get_user_by_username(&user.username).await?.is_some() {
            return Err(StoreError::Conflict(format!(
                "username {:?} is already taken",
                user.username
            )));
        }
        let model = user_model_from_new(
            Uuid::new_v4(),
            &user.username,
            &user.display_name,
            user.role,
            user.password_hash.as_deref(),
            Timestamp::now(),
        );
        let stored = self
            .query_one::<UserModel>(insert_command::<UserModel>(&model))
            .await?
            .ok_or_else(|| StoreError::Db("INSERT returned no row".to_owned()))?;
        user_model_to_record(stored)
    }

    async fn delete_user(&self, id: Uuid) -> Result<bool, StoreError> {
        let removed = self
            .executor
            .delete::<UserModel>(Some(UserColumn::Id.eq(id).into_inner()))
            .await?;
        Ok(removed > 0)
    }

    async fn list_files(&self, offset: u64, limit: u64) -> Result<Vec<FileRecord>, StoreError> {
        let cmd = Command::Select {
            table: FileModel::TABLE.to_owned(),
            filter: Some(FileColumn::DeletedAt.is_null().into_inner()),
            order_by: vec![("created_at".to_owned(), true)],
            limit: Some(limit),
            offset: Some(offset),
            lock: None,
        };
        self.query_all::<FileModel>(cmd)
            .await?
            .into_iter()
            .map(file_model_to_record)
            .collect()
    }

    async fn count_files(&self) -> Result<u64, StoreError> {
        let count = self
            .executor
            .count::<FileModel>(Some(FileColumn::DeletedAt.is_null().into_inner()))
            .await?;
        Ok(count as u64)
    }

    async fn get_file(&self, id: Uuid) -> Result<Option<FileRecord>, StoreError> {
        self.query_one::<FileModel>(live_file_by(FileColumn::Id, id))
            .await?
            .map(file_model_to_record)
            .transpose()
    }

    async fn get_file_by_owner_and_sha256(
        &self,
        owner_id: Uuid,
        sha256: &str,
    ) -> Result<Option<FileRecord>, StoreError> {
        let cmd = Command::Select {
            table: FileModel::TABLE.to_owned(),
            filter: Some(
                FileColumn::DeletedAt
                    .is_null()
                    .into_inner()
                    .and(FileColumn::OwnerId.eq(owner_id).into_inner())
                    .and(FileColumn::Sha256.eq(sha256).into_inner()),
            ),
            order_by: vec![],
            limit: None,
            offset: None,
            lock: None,
        };
        self.query_one::<FileModel>(cmd)
            .await?
            .map(file_model_to_record)
            .transpose()
    }

    async fn get_file_by_storage_key(&self, key: &str) -> Result<Option<FileRecord>, StoreError> {
        self.query_one::<FileModel>(live_file_by(FileColumn::StorageKey, key))
            .await?
            .map(file_model_to_record)
            .transpose()
    }

    async fn create_file(&self, file: NewFile) -> Result<FileRecord, StoreError> {
        if self
            .get_file_by_owner_and_sha256(file.owner_id, &file.sha256)
            .await?
            .is_some()
        {
            return Err(StoreError::Conflict(format!(
                "content {sha} is already uploaded by this user",
                sha = file.sha256
            )));
        }
        let model = file_model_from_new(Uuid::new_v4(), &file, Timestamp::now());
        let stored = self
            .query_one::<FileModel>(insert_command::<FileModel>(&model))
            .await?
            .ok_or_else(|| StoreError::Db("INSERT returned no row".to_owned()))?;
        file_model_to_record(stored)
    }

    async fn rename_file(&self, id: Uuid, name: &str) -> Result<Option<FileRecord>, StoreError> {
        // Single guarded UPDATE: `WHERE id = $1 AND deleted_at IS NULL`.
        // No read-then-write window — a concurrent soft delete makes this
        // answer `None` (0 rows) instead of resurrecting the row.
        let cmd = update_command::<FileModel>(
            vec![("name", Value::Text(name.to_owned()))],
            FileColumn::Id
                .eq(id)
                .into_inner()
                .and(FileColumn::DeletedAt.is_null().into_inner()),
        )?;
        self.query_one::<FileModel>(cmd)
            .await?
            .map(file_model_to_record)
            .transpose()
    }

    async fn soft_delete_file(&self, id: Uuid) -> Result<bool, StoreError> {
        // Single guarded UPDATE, same shape as rename: the second call of
        // a racing pair simply marks 0 rows.
        let cmd = update_command::<FileModel>(
            vec![("deleted_at", Value::Timestamp(Timestamp::now().to_string()))],
            FileColumn::Id
                .eq(id)
                .into_inner()
                .and(FileColumn::DeletedAt.is_null().into_inner()),
        )?;
        Ok(self.query_one::<FileModel>(cmd).await?.is_some())
    }

    async fn count_live_files_by_storage_key(&self, key: &str) -> Result<u64, StoreError> {
        let count = self
            .executor
            .count::<FileModel>(Some(
                FileColumn::DeletedAt
                    .is_null()
                    .into_inner()
                    .and(FileColumn::StorageKey.eq(key).into_inner()),
            ))
            .await?;
        Ok(count as u64)
    }

    async fn get_oauth_identity(
        &self,
        provider: &str,
        subject: &str,
    ) -> Result<Option<Uuid>, StoreError> {
        let cmd = Command::Select {
            table: OAuthIdentityModel::TABLE.to_owned(),
            filter: Some(
                OAuthIdentityColumn::Provider
                    .eq(provider)
                    .into_inner()
                    .and(OAuthIdentityColumn::Subject.eq(subject).into_inner()),
            ),
            order_by: vec![],
            limit: None,
            offset: None,
            lock: None,
        };
        Ok(self
            .query_one::<OAuthIdentityModel>(cmd)
            .await?
            .map(|model| model.user_id))
    }

    async fn link_oauth_identity(
        &self,
        provider: &str,
        subject: &str,
        user_id: Uuid,
    ) -> Result<(), StoreError> {
        let model = OAuthIdentityModel {
            provider: provider.to_owned(),
            subject: subject.to_owned(),
            user_id,
        };
        match self
            .query_one::<OAuthIdentityModel>(insert_command::<OAuthIdentityModel>(&model))
            .await
        {
            Ok(_) => Ok(()),
            Err(StoreError::Conflict(reason)) => Err(StoreError::Conflict(reason)),
            Err(StoreError::Db(reason)) => {
                // A composite-PK collision surfaces as a unique-violation
                // database error through the escape hatch; normalize the
                // class the memory lane already speaks.
                if reason.to_ascii_lowercase().contains("duplicate key") {
                    Err(StoreError::Conflict(format!(
                        "identity {provider}/{subject} is already linked"
                    )))
                } else {
                    Err(StoreError::Db(reason))
                }
            }
        }
    }

    async fn get_setting(&self, key: &str) -> Result<Option<serde_json::Value>, StoreError> {
        let cmd = Command::Select {
            table: SettingModel::TABLE.to_owned(),
            filter: Some(SettingColumn::Key.eq(key).into_inner()),
            order_by: vec![],
            limit: None,
            offset: None,
            lock: None,
        };
        Ok(self
            .query_one::<SettingModel>(cmd)
            .await?
            .map(|model| model.value))
    }

    async fn set_setting(&self, key: &str, value: serde_json::Value) -> Result<(), StoreError> {
        // Escape hatch: the upsert needs `excluded.value`, which is an SQL
        // expression — a bound parameter cannot express it.
        let cmd = Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "INSERT INTO settings (key, value) VALUES ($1, $2) \
                   ON CONFLICT (key) DO UPDATE SET value = excluded.value"
                .to_owned(),
            params: vec![Value::Text(key.to_owned()), Value::Json(value.to_string())],
        };
        self.executor
            .execute(&cmd)
            .await
            .map_err(StoreError::from)?;
        Ok(())
    }
}
