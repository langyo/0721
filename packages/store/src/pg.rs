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

use i0721_proto::{NewShare, ShareRecord};

use crate::entity::{
    file_model_from_new, file_model_to_record, share_model_to_record, user_model_from_new,
    user_model_to_record, FileColumn, FileModel, OAuthIdentityColumn, OAuthIdentityModel,
    SettingColumn, SettingModel, ShareColumn, ShareModel, UserColumn, UserModel,
};
use crate::{migrations, CreateUser, Store, StoreError};

/// A [`Store`] over a PostgreSQL connection pool.
#[derive(Debug, Clone)]
pub struct PgStore {
    pub executor: PgExecutor,
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

    // ── netdisk lane ─────────────────────────────────────────────────

    async fn list_folder(
        &self,
        owner_id: Uuid,
        parent_id: Option<Uuid>,
        kind: Option<String>,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<FileRecord>, StoreError> {
        let mut filter = FileColumn::DeletedAt
            .is_null()
            .into_inner()
            .and(FileColumn::OwnerId.eq(owner_id).into_inner());
        filter = match parent_id {
            Some(pid) => filter.and(FileColumn::ParentId.eq(pid).into_inner()),
            None => filter.and(FileColumn::ParentId.is_null().into_inner()),
        };
        if let Some(kind) = kind {
            filter = filter.and(FileColumn::Kind.eq(kind).into_inner());
        }
        let cmd = Command::Select {
            table: FileModel::TABLE.to_owned(),
            filter: Some(filter),
            order_by: vec![("created_at".to_owned(), false)],
            limit: Some(limit),
            offset: Some(offset),
            lock: None,
        };
        Ok(self
            .query_all::<FileModel>(cmd)
            .await?
            .into_iter()
            .map(file_model_to_record)
            .collect::<Result<Vec<_>, _>>()?)
    }

    async fn folder_name_taken(
        &self,
        owner_id: Uuid,
        parent_id: Option<Uuid>,
        name: &str,
        exclude: Option<Uuid>,
    ) -> Result<bool, StoreError> {
        // Entity-layer read (the raw COALESCE SQL hit the driver's
        // binary-decoding path for UUID params); the root/NULL folding
        // that COALESCE expressed happens here in Rust.
        let mut filter = FileColumn::DeletedAt
            .is_null()
            .into_inner()
            .and(FileColumn::OwnerId.eq(owner_id).into_inner())
            .and(FileColumn::Name.eq(name).into_inner());
        filter = match parent_id {
            Some(pid) => filter.and(FileColumn::ParentId.eq(pid).into_inner()),
            None => filter.and(FileColumn::ParentId.is_null().into_inner()),
        };
        if let Some(exclude_id) = exclude {
            filter = filter.and(FileColumn::Id.ne(exclude_id).into_inner());
        }
        let cmd = Command::Select {
            table: FileModel::TABLE.to_owned(),
            filter: Some(filter),
            order_by: vec![],
            limit: None,
            offset: None,
            lock: None,
        };
        Ok(!self.query_all::<FileModel>(cmd).await?.is_empty())
    }

    async fn create_folder(
        &self,
        owner_id: Uuid,
        parent_id: Option<Uuid>,
        name: &str,
    ) -> Result<FileRecord, StoreError> {
        if self
            .folder_name_taken(owner_id, parent_id, name, None)
            .await?
        {
            return Err(StoreError::Conflict(format!(
                "name {name:?} already exists in this folder"
            )));
        }
        let model = FileModel {
            id: Uuid::new_v4(),
            owner_id,
            parent_id,
            name: name.to_owned(),
            kind: "folder".to_owned(),
            mime: "inode/directory".to_owned(),
            size: 0,
            sha256: format!("dir:{}", Uuid::new_v4().simple()),
            storage_backend: "local".to_owned(),
            storage_key: String::new(),
            thumbnail_key: None,
            original_name: name.to_owned(),
            created_at: Timestamp::now(),
            deleted_at: None,
        };
        let stored = self
            .query_one::<FileModel>(insert_command::<FileModel>(&model))
            .await?
            .ok_or_else(|| StoreError::Db("INSERT returned no row".to_owned()))?;
        file_model_to_record(stored)
    }

    async fn set_parent(&self, id: Uuid, parent_id: Option<Uuid>) -> Result<(), StoreError> {
        let cmd = update_command::<FileModel>(
            vec![(
                "parent_id",
                parent_id
                    .map(|p| Value::Uuid(p.to_string()))
                    .unwrap_or(Value::Null),
            )],
            FileColumn::DeletedAt
                .is_null()
                .into_inner()
                .and(FileColumn::Id.eq(id).into_inner()),
        )?;
        self.executor.execute(&cmd).await?;
        Ok(())
    }

    async fn ancestor_ids(&self, id: Uuid) -> Result<Vec<Uuid>, StoreError> {
        // Iterative walk through the entity layer (the raw recursive
        // CTE hits the driver's binary-decoding path for UUID columns).
        let mut chain = Vec::new();
        let mut cursor = id;
        loop {
            let Some(row) = self.get_file(cursor).await? else {
                break;
            };
            let Some(parent) = row.parent_id else {
                break;
            };
            chain.push(parent);
            cursor = parent;
        }
        Ok(chain)
    }

    async fn subtree_live_ids(&self, id: Uuid) -> Result<Vec<Uuid>, StoreError> {
        // Iterative BFS through the entity layer (see ancestor_ids).
        let mut ids = vec![id];
        let mut frontier = vec![id];
        while let Some(current) = frontier.pop() {
            let cmd = Command::Select {
                table: FileModel::TABLE.to_owned(),
                filter: Some(
                    FileColumn::DeletedAt
                        .is_null()
                        .into_inner()
                        .and(FileColumn::ParentId.eq(current).into_inner()),
                ),
                order_by: vec![],
                limit: None,
                offset: None,
                lock: None,
            };
            for child in self.query_all::<FileModel>(cmd).await? {
                ids.push(child.id);
                frontier.push(child.id);
            }
        }
        Ok(ids)
    }

    async fn soft_delete_many(&self, ids: &[Uuid]) -> Result<u64, StoreError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let stamp = Timestamp::now().to_string();
        let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("${i}")).collect();
        let cmd = Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: format!(
                "UPDATE files SET deleted_at = '{}' WHERE deleted_at IS NULL AND id IN ({})",
                stamp,
                placeholders.join(", ")
            ),
            params: ids.iter().map(|id| Value::Uuid(id.to_string())).collect(),
        };
        let reply = self.executor.execute(&cmd).await?;
        let Reply::Affected(n) = reply else {
            return Err(StoreError::Db("expected an affected count".to_owned()));
        };
        Ok(n)
    }

    async fn get_files_by_ids(&self, ids: &[Uuid]) -> Result<Vec<FileRecord>, StoreError> {
        // Cascade batches are bounded by subtree size; N indexed reads
        // keep the row mapping inside the entity layer (raw rows cannot
        // be typed back without the projection).
        let mut out = Vec::new();
        for id in ids {
            if let Some(record) = self.get_file(*id).await? {
                out.push(record);
            }
        }
        Ok(out)
    }

    async fn create_share(&self, share: NewShare) -> Result<ShareRecord, StoreError> {
        if let Some(existing) = self
            .query_one::<ShareModel>(Command::Select {
                table: ShareModel::TABLE.to_owned(),
                filter: Some(ShareColumn::FileId.eq(share.file_id).into_inner()),
                order_by: vec![],
                limit: None,
                offset: None,
                lock: None,
            })
            .await?
        {
            if existing.owner_id == share.owner_id {
                return Ok(share_model_to_record(existing));
            }
        }
        let model = ShareModel {
            token: crate::new_share_token(),
            file_id: share.file_id,
            owner_id: share.owner_id,
            expires_at: share.expires_at,
            allow_preview: share.allow_preview,
            created_at: Timestamp::now(),
        };
        let stored = self
            .query_one::<ShareModel>(insert_command::<ShareModel>(&model))
            .await?
            .ok_or_else(|| StoreError::Db("INSERT returned no row".to_owned()))?;
        Ok(share_model_to_record(stored))
    }

    async fn get_share(&self, token: &str) -> Result<Option<ShareRecord>, StoreError> {
        let cmd = Command::Select {
            table: ShareModel::TABLE.to_owned(),
            filter: Some(ShareColumn::Token.eq(token).into_inner()),
            order_by: vec![],
            limit: None,
            offset: None,
            lock: None,
        };
        Ok(self
            .query_one::<ShareModel>(cmd)
            .await?
            .map(share_model_to_record))
    }

    async fn list_shares(&self, owner_id: Uuid) -> Result<Vec<ShareRecord>, StoreError> {
        let cmd = Command::Select {
            table: ShareModel::TABLE.to_owned(),
            filter: Some(ShareColumn::OwnerId.eq(owner_id).into_inner()),
            order_by: vec![("created_at".to_owned(), false)],
            limit: None,
            offset: None,
            lock: None,
        };
        Ok(self
            .query_all::<ShareModel>(cmd)
            .await?
            .into_iter()
            .map(share_model_to_record)
            .collect())
    }

    async fn delete_share(&self, token: &str) -> Result<bool, StoreError> {
        let removed = self
            .executor
            .delete::<ShareModel>(Some(ShareColumn::Token.eq(token).into_inner()))
            .await?;
        Ok(removed > 0)
    }

    async fn sum_live_sizes(&self, owner_id: Uuid) -> Result<i64, StoreError> {
        let cmd = Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "SELECT CAST(COALESCE(SUM(size), 0) AS BIGINT) AS used FROM files                    WHERE deleted_at IS NULL AND owner_id = $1 AND kind <> 'folder'"
                .to_owned(),
            params: vec![Value::Uuid(owner_id.to_string())],
        };
        let reply = self.executor.execute(&cmd).await?;
        let Reply::Rows { rows, .. } = reply else {
            return Err(StoreError::Db("expected rows from a SUM".to_owned()));
        };
        Ok(rows
            .first()
            .and_then(|row| row.iter().next())
            .and_then(|v| match v {
                Value::I64(n) => Some(*n),
                Value::I32(n) => Some(i64::from(*n)),
                _ => None,
            })
            .unwrap_or(0))
    }

    async fn set_user_quota(
        &self,
        user_id: Uuid,
        quota_bytes: Option<i64>,
    ) -> Result<(), StoreError> {
        let cmd = update_command::<UserModel>(
            vec![(
                "quota_bytes",
                quota_bytes.map(Value::I64).unwrap_or(Value::Null),
            )],
            UserColumn::Id.eq(user_id).into_inner(),
        )?;
        self.executor.execute(&cmd).await?;
        Ok(())
    }
}
