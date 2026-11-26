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
use std::sync::OnceLock;
use uuid::Uuid;

use i0721_proto::{NewShare, ShareRecord};

use crate::entity::{
    api_key_model_to_record, file_model_from_new, file_model_to_record, share_model_to_record,
    storage_bucket_model_to_record, user_model_from_new, user_model_to_record, ApiKeyColumn,
    ApiKeyModel, BucketGrantColumn, BucketGrantModel, FileColumn, FileModel, OAuthIdentityColumn,
    OAuthIdentityModel, SettingColumn, SettingModel, ShareColumn, ShareModel, StorageBucketColumn,
    StorageBucketModel, UserColumn, UserModel,
};
use crate::{migrations, BootstrapBucketParams, CreateUser, Store, StoreError};

/// The process-wide bootstrap facts the bucket migration needs. Set once
/// by the main binary after the R-U decision; [`PgStore::migrate`] reads
/// it so every lane that calls the parameterless form still carries the
/// facts.
static BOOTSTRAP_PARAMS: OnceLock<BootstrapBucketParams> = OnceLock::new();

/// A [`Store`] over a PostgreSQL connection pool.
#[derive(Debug, Clone)]
pub struct PgStore {
    /// The underlying PostgreSQL executor. PUB for the pg_lane tests'
    /// escape hatch (hand-planted pre-migration rows, index drops).
    /// Business code must go through the Store trait — raw SQL is the
    /// documented escape hatch (`Command::Sql`), never `executor`.
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
        self.migrate_with(BOOTSTRAP_PARAMS.get()).await
    }

    /// Same, with explicit bootstrap facts (the main binary resolves
    /// them fail-closed from the environment and the pending set; tests
    /// pass a fixed set). Must be called once before [`Self::migrate`]
    /// when the process serves real migrations — see [`Self::set_bootstrap_params`].
    ///
    /// # Errors
    ///
    /// [`StoreError::Db`] when a migration fails to apply.
    pub async fn migrate_with(
        &self,
        bootstrap: Option<&BootstrapBucketParams>,
    ) -> Result<MigrationResult, StoreError> {
        MigrationRunner::new(migrations(bootstrap))
            .up(self.executor.pool())
            .await
            .map_err(StoreError::from)
    }

    /// Register the process-wide bootstrap facts. The FIRST set wins (a
    /// process runs one migration set); later differing sets are a
    /// programming error and fail loudly at debug, quietly at release —
    /// the migration facts of a process must not change mid-flight.
    pub fn set_bootstrap_params(params: BootstrapBucketParams) {
        match BOOTSTRAP_PARAMS.get() {
            Some(existing) => assert!(
                *existing == params,
                "bootstrap bucket params must be set once, consistently"
            ),
            None => {
                let _ = BOOTSTRAP_PARAMS.set(params);
            }
        }
    }

    /// The recorded (already applied) migration versions on this
    /// database — the R-U wiring's pending-set source. Empty when the
    /// bookkeeping table has no rows (a fresh database).
    ///
    /// # Errors
    ///
    /// [`StoreError::Db`] when the bookkeeping table cannot be read.
    pub async fn applied_versions(&self) -> Result<Vec<String>, StoreError> {
        // A FRESH database has no bookkeeping table yet — the runner
        // creates it as the first step of `up`. Probe before querying so
        // the startup gate reads "nothing applied" there instead of a raw
        // missing-relation error.
        let probe = Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "SELECT (to_regclass('seaql_migrations') IS NULL)::text".to_owned(),
            params: vec![],
        };
        let reply = self.executor.execute(&probe).await?;
        let Reply::Rows { rows, .. } = reply else {
            return Err(StoreError::Db(
                "expected a row from the bookkeeping-table probe".to_owned(),
            ));
        };
        let absent = rows
            .first()
            .and_then(|row| row.first())
            .map(|value| matches!(value, Value::Text(text) if text == "true"))
            .unwrap_or(false);
        if absent {
            return Ok(Vec::new());
        }
        let cmd = Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "SELECT version FROM seaql_migrations".to_owned(),
            params: vec![],
        };
        let reply = self.executor.execute(&cmd).await?;
        let Reply::Rows { rows, .. } = reply else {
            return Err(StoreError::Db(
                "expected rows from the migration bookkeeping table".to_owned(),
            ));
        };
        Ok(rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .filter_map(|value| match value {
                Value::Text(text) => Some(text),
                _ => None,
            })
            .collect())
    }

    /// Whether the bucket migration (0007) still needs to run (R-U ②).
    ///
    /// `migrations(None)` deliberately omits 0007 — its SQL is only
    /// complete once the deployment facts are passed in — so the pending
    /// set must be asked about 0007 directly. On a fresh database the
    /// bookkeeping table does not exist yet; [`Self::applied_versions`]
    /// reads that as an empty applied set, which makes 0007 (and every
    /// other migration) pending.
    ///
    /// # Errors
    ///
    /// [`StoreError::Db`] when the bookkeeping table cannot be read.
    pub async fn bucket_migration_pending(&self) -> Result<bool, StoreError> {
        let applied = self.applied_versions().await?;
        Ok(!applied
            .iter()
            .any(|v| v == crate::migrate::BUCKET_MIGRATION_VERSION))
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
        Command::Aggregate { .. } => "AGGREGATE",
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
    returning: bool,
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
        returning,
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
        distinct: false,
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
        distinct: false,
    }
}

/// The live-rows filter shared by the folder listing and its count.
/// `buckets` is the §7 pushdown (Q14): `Some(set)` keeps only rows whose
/// `bucket_id` is in the caller's visible set — the SQL is the ONLY
/// filter site, so a count and its listing can never disagree.
/// MUT:M3b — dropping this filter (or applying it only to one of the two
/// lanes) lets invisible-bucket rows leak into either the items or the
/// total, which reddens the pushdown tests.
fn folder_filter(
    owner_id: Uuid,
    parent_id: Option<Uuid>,
    kind: Option<String>,
    buckets: Option<&[Uuid]>,
) -> lacrymira::Condition {
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
    if let Some(set) = buckets {
        // A NULL-bucket row (rollback-window legacy) resolves through the
        // bootstrap bucket by the §7 column rules; the visible set is
        // computed from the registry, so keep NULL-bucket rows when the
        // bootstrap is in the set. The comparison stays SQL-side (no
        // post-fetch filtering — that would let `total` leak counts).
        filter = filter.and(
            FileColumn::BucketId
                .in_(set.to_vec())
                .into_inner()
                .or(FileColumn::BucketId.is_null().into_inner()),
        );
    }
    filter
}

#[async_trait::async_trait]
impl Store for PgStore {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn list_users(&self, offset: u64, limit: u64) -> Result<Vec<UserRecord>, StoreError> {
        let cmd = Command::Select {
            table: UserModel::TABLE.to_owned(),
            filter: None,
            order_by: vec![("created_at".to_owned(), false)],
            limit: Some(limit),
            offset: Some(offset),
            lock: None,
            distinct: false,
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
            distinct: false,
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

    async fn list_files(
        &self,
        buckets: Option<&[Uuid]>,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<FileRecord>, StoreError> {
        let filter = FileColumn::DeletedAt.is_null().into_inner();
        let filter = match buckets {
            None => filter,
            Some(set) => filter
                .and(FileColumn::BucketId.in_(set.to_vec()).into_inner())
                .or(FileColumn::BucketId.is_null().into_inner()),
        };
        let cmd = Command::Select {
            table: FileModel::TABLE.to_owned(),
            filter: Some(filter),
            order_by: vec![("created_at".to_owned(), true)],
            limit: Some(limit),
            offset: Some(offset),
            lock: None,
            distinct: false,
        };
        self.query_all::<FileModel>(cmd)
            .await?
            .into_iter()
            .map(file_model_to_record)
            .collect()
    }

    async fn count_files(&self, buckets: Option<&[Uuid]>) -> Result<u64, StoreError> {
        let filter = FileColumn::DeletedAt.is_null().into_inner();
        let filter = match buckets {
            None => filter,
            Some(set) => filter
                .and(FileColumn::BucketId.in_(set.to_vec()).into_inner())
                .or(FileColumn::BucketId.is_null().into_inner()),
        };
        let count = self.executor.count::<FileModel>(Some(filter)).await?;
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
            distinct: false,
        };
        self.query_one::<FileModel>(cmd)
            .await?
            .map(file_model_to_record)
            .transpose()
    }

    async fn list_live_rows_by_storage_key(
        &self,
        key: &str,
    ) -> Result<Vec<FileRecord>, StoreError> {
        // `id ASC` is the deterministic traversal order R-J pins: the
        // same key must resolve identically on every request and on both
        // stores.
        let cmd = Command::Select {
            table: FileModel::TABLE.to_owned(),
            filter: Some(
                FileColumn::DeletedAt
                    .is_null()
                    .into_inner()
                    .and(FileColumn::StorageKey.eq(key).into_inner()),
            ),
            order_by: vec![("id".to_owned(), false)],
            limit: None,
            offset: None,
            lock: None,
            distinct: false,
        };
        self.query_all::<FileModel>(cmd)
            .await?
            .into_iter()
            .map(file_model_to_record)
            .collect()
    }

    async fn list_storage_buckets(
        &self,
    ) -> Result<Vec<i0721_proto::StorageBucketRecord>, StoreError> {
        let cmd = Command::Select {
            table: StorageBucketModel::TABLE.to_owned(),
            filter: None,
            order_by: vec![("created_at".to_owned(), false)],
            limit: None,
            offset: None,
            lock: None,
            distinct: false,
        };
        self.query_all::<StorageBucketModel>(cmd)
            .await?
            .into_iter()
            .map(storage_bucket_model_to_record)
            .collect()
    }

    async fn get_storage_bucket_by_slug(
        &self,
        slug: &str,
    ) -> Result<Option<i0721_proto::StorageBucketRecord>, StoreError> {
        let cmd = Command::Select {
            table: StorageBucketModel::TABLE.to_owned(),
            filter: Some(StorageBucketColumn::Slug.eq(slug).into_inner()),
            order_by: vec![],
            limit: None,
            offset: None,
            lock: None,
            distinct: false,
        };
        self.query_one::<StorageBucketModel>(cmd)
            .await?
            .map(storage_bucket_model_to_record)
            .transpose()
    }

    async fn upsert_storage_bucket(
        &self,
        slug: &str,
        display_name: &str,
        anonymous_read: bool,
        grant_on_signup: bool,
        enabled: bool,
    ) -> Result<i0721_proto::StorageBucketRecord, StoreError> {
        // The mutable-fields UPDATE keyed by slug; the identity fields
        // (slug/backend/location/endpoint_ref/is_bootstrap) are NOT in
        // the SET list — the RPC layer's strict full-document gate has
        // already refused unknown/missing fields, the store only ever
        // writes the four mutable ones.
        let cmd = update_command::<StorageBucketModel>(
            vec![
                ("display_name", Value::Text(display_name.to_owned())),
                ("anonymous_read", Value::Bool(anonymous_read)),
                ("grant_on_signup", Value::Bool(grant_on_signup)),
                ("enabled", Value::Bool(enabled)),
            ],
            StorageBucketColumn::Slug.eq(slug).into_inner(),
            true,
        )?;
        let stored = self
            .query_one::<StorageBucketModel>(cmd)
            .await?
            .ok_or_else(|| StoreError::Conflict(format!("no such bucket: {slug}")))?;
        storage_bucket_model_to_record(stored)
    }

    async fn delete_storage_bucket(&self, id: Uuid) -> Result<bool, StoreError> {
        let removed = self
            .executor
            .delete::<StorageBucketModel>(Some(StorageBucketColumn::Id.eq(id).into_inner()))
            .await?;
        Ok(removed > 0)
    }

    async fn count_live_files_by_bucket(&self, bucket_id: Option<Uuid>) -> Result<u64, StoreError> {
        let cmd = Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "SELECT COUNT(*) AS n FROM files WHERE deleted_at IS NULL \
                   AND bucket_id IS NOT DISTINCT FROM $1"
                .to_owned(),
            params: vec![bucket_id
                .map(|id| Value::Uuid(id.to_string()))
                .unwrap_or(Value::Null)],
        };
        let reply = self.executor.execute(&cmd).await?;
        let Reply::Rows { rows, .. } = reply else {
            return Err(StoreError::Db("expected rows from a COUNT".to_owned()));
        };
        let n = rows
            .first()
            .and_then(|row| row.iter().next())
            .and_then(|v| match v {
                Value::I64(n) => Some(*n),
                Value::I32(n) => Some(i64::from(*n)),
                _ => None,
            })
            .unwrap_or(0);
        Ok(n as u64)
    }

    async fn storage_keys_live_in_bucket(
        &self,
        bucket_id: Option<Uuid>,
    ) -> Result<std::collections::HashSet<String>, StoreError> {
        let cmd = Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "SELECT DISTINCT storage_key FROM files WHERE deleted_at IS NULL \
                   AND bucket_id IS NOT DISTINCT FROM $1"
                .to_owned(),
            params: vec![bucket_id
                .map(|id| Value::Uuid(id.to_string()))
                .unwrap_or(Value::Null)],
        };
        let reply = self.executor.execute(&cmd).await?;
        let Reply::Rows { rows, .. } = reply else {
            return Err(StoreError::Db("expected rows from a key scan".to_owned()));
        };
        Ok(rows
            .into_iter()
            .filter_map(|row| match row.first() {
                Some(Value::Text(key)) => Some(key.clone()),
                _ => None,
            })
            .collect())
    }

    async fn get_file_by_owner_bucket_and_sha256(
        &self,
        owner_id: Uuid,
        bucket_id: Option<Uuid>,
        sha256: &str,
    ) -> Result<Option<FileRecord>, StoreError> {
        let mut filter = FileColumn::DeletedAt
            .is_null()
            .into_inner()
            .and(FileColumn::OwnerId.eq(owner_id).into_inner())
            .and(FileColumn::Sha256.eq(sha256).into_inner());
        filter = match bucket_id {
            Some(id) => filter.and(FileColumn::BucketId.eq(id).into_inner()),
            None => filter.and(FileColumn::BucketId.is_null().into_inner()),
        };
        let cmd = Command::Select {
            table: FileModel::TABLE.to_owned(),
            filter: Some(filter),
            order_by: vec![],
            limit: None,
            offset: None,
            lock: None,
            distinct: false,
        };
        self.query_one::<FileModel>(cmd)
            .await?
            .map(file_model_to_record)
            .transpose()
    }

    async fn get_bucket_grant(
        &self,
        user_id: Uuid,
        bucket_id: Uuid,
    ) -> Result<Option<i0721_proto::BucketGrantRecord>, StoreError> {
        let cmd = Command::Select {
            table: BucketGrantModel::TABLE.to_owned(),
            filter: Some(
                BucketGrantColumn::UserId
                    .eq(user_id)
                    .into_inner()
                    .and(BucketGrantColumn::BucketId.eq(bucket_id).into_inner()),
            ),
            order_by: vec![],
            limit: None,
            offset: None,
            lock: None,
            distinct: false,
        };
        Ok(self
            .query_one::<BucketGrantModel>(cmd)
            .await?
            .map(crate::entity::bucket_grant_model_to_record))
    }

    async fn set_bucket_grant(
        &self,
        user_id: Uuid,
        bucket_id: Uuid,
        can_read: bool,
        can_write: bool,
        can_share: bool,
    ) -> Result<(), StoreError> {
        // Escape hatch: the upsert needs `excluded.*` expressions.
        let cmd = Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "INSERT INTO bucket_grants (user_id, bucket_id, can_read, can_write, can_share) \
                   VALUES ($1, $2, $3, $4, $5) \
                   ON CONFLICT (user_id, bucket_id) DO UPDATE SET \
                     can_read = excluded.can_read, can_write = excluded.can_write, \
                     can_share = excluded.can_share"
                .to_owned(),
            params: vec![
                Value::Uuid(user_id.to_string()),
                Value::Uuid(bucket_id.to_string()),
                Value::Bool(can_read),
                Value::Bool(can_write),
                Value::Bool(can_share),
            ],
        };
        self.executor
            .execute(&cmd)
            .await
            .map_err(StoreError::from)?;
        Ok(())
    }

    async fn list_bucket_grants_for_user(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<i0721_proto::BucketGrantRecord>, StoreError> {
        let cmd = Command::Select {
            table: BucketGrantModel::TABLE.to_owned(),
            filter: Some(BucketGrantColumn::UserId.eq(user_id).into_inner()),
            order_by: vec![],
            limit: None,
            offset: None,
            lock: None,
            distinct: false,
        };
        Ok(self
            .query_all::<BucketGrantModel>(cmd)
            .await?
            .into_iter()
            .map(crate::entity::bucket_grant_model_to_record)
            .collect())
    }

    async fn list_bucket_grants_for_bucket(
        &self,
        bucket_id: Uuid,
    ) -> Result<Vec<i0721_proto::BucketGrantRecord>, StoreError> {
        let cmd = Command::Select {
            table: BucketGrantModel::TABLE.to_owned(),
            filter: Some(BucketGrantColumn::BucketId.eq(bucket_id).into_inner()),
            order_by: vec![],
            limit: None,
            offset: None,
            lock: None,
            distinct: false,
        };
        Ok(self
            .query_all::<BucketGrantModel>(cmd)
            .await?
            .into_iter()
            .map(crate::entity::bucket_grant_model_to_record)
            .collect())
    }

    async fn settings_state(&self, key: &str) -> Result<crate::SettingsState, StoreError> {
        // The four-state read (R-F): a store failure is `Unreadable`
        // (NOT aDb error propagated as "absent"), a row whose payload
        // fails the strict parser is `Corrupt` with the raw text.
        let cmd = Command::Select {
            table: SettingModel::TABLE.to_owned(),
            filter: Some(SettingColumn::Key.eq(key).into_inner()),
            order_by: vec![],
            limit: None,
            offset: None,
            lock: None,
            distinct: false,
        };
        match self.query_one::<SettingModel>(cmd).await {
            Err(err) => Ok(crate::SettingsState::Unreadable {
                err: err.to_string(),
            }),
            Ok(None) => Ok(crate::SettingsState::Absent),
            Ok(Some(model)) => Ok(crate::SettingsState::Value(model.value)),
        }
    }

    async fn a_live_key_references_bucket_slug(&self, slug: &str) -> Result<bool, StoreError> {
        // The spec-verbatim R-B SQL (also re-run by the PG lane's tests).
        const BUCKET_KEY_REFERENCE_SQL: &str = "SELECT EXISTS (SELECT 1 FROM api_keys \
             WHERE revoked_at IS NULL AND jsonb_typeof(scopes->'buckets') = 'array' \
             AND scopes->'buckets' ? $1)";
        let cmd = Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: BUCKET_KEY_REFERENCE_SQL.to_owned(),
            params: vec![Value::Text(slug.to_owned())],
        };
        let reply = self.executor.execute(&cmd).await?;
        let Reply::Rows { rows, .. } = reply else {
            return Err(StoreError::Db(
                "expected rows from the key-reference probe".to_owned(),
            ));
        };
        Ok(rows
            .first()
            .and_then(|row| row.first())
            .and_then(|value| match value {
                Value::Bool(b) => Some(*b),
                Value::Text(text) => text.parse().ok(),
                _ => None,
            })
            .unwrap_or(false))
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
        // Same-layer uniqueness (the partial unique index's mirror —
        // exclude self; friendly Conflict instead of a raw 23505).
        if let Some(current) = self.get_file(id).await? {
            if self
                .folder_name_taken(
                    current.owner_id,
                    current.parent_id,
                    current.bucket_id,
                    name,
                    Some(id),
                )
                .await?
            {
                return Err(StoreError::Conflict(format!(
                    "name {name:?} already exists in this folder"
                )));
            }
        }
        // Single guarded UPDATE: `WHERE id = $1 AND deleted_at IS NULL`.
        // No read-then-write window — a concurrent soft delete makes this
        // answer `None` (0 rows) instead of resurrecting the row.
        let cmd = update_command::<FileModel>(
            vec![("name", Value::Text(name.to_owned()))],
            FileColumn::Id
                .eq(id)
                .into_inner()
                .and(FileColumn::DeletedAt.is_null().into_inner()),
            true,
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
            true,
        )?;
        Ok(self.query_one::<FileModel>(cmd).await?.is_some())
    }

    async fn count_live_files_by_storage_key(
        &self,
        bucket_id: Option<Uuid>,
        key: &str,
    ) -> Result<u64, StoreError> {
        let cmd = Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "SELECT COUNT(*) AS n FROM files WHERE deleted_at IS NULL \
                   AND bucket_id IS NOT DISTINCT FROM $1 AND storage_key = $2"
                .to_owned(),
            params: vec![
                bucket_id
                    .map(|id| Value::Uuid(id.to_string()))
                    .unwrap_or(Value::Null),
                Value::Text(key.to_owned()),
            ],
        };
        let reply = self.executor.execute(&cmd).await?;
        let Reply::Rows { rows, .. } = reply else {
            return Err(StoreError::Db("expected rows from a COUNT".to_owned()));
        };
        let n = rows
            .first()
            .and_then(|row| row.iter().next())
            .and_then(|v| match v {
                Value::I64(n) => Some(*n),
                Value::I32(n) => Some(i64::from(*n)),
                _ => None,
            })
            .unwrap_or(0);
        Ok(n as u64)
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
            distinct: false,
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

    async fn oauth_providers_for_user(&self, user_id: Uuid) -> Result<Vec<String>, StoreError> {
        let cmd = Command::Select {
            table: OAuthIdentityModel::TABLE.to_owned(),
            filter: Some(OAuthIdentityColumn::UserId.eq(user_id).into_inner()),
            order_by: vec![],
            limit: None,
            offset: None,
            lock: None,
            distinct: false,
        };
        // Dedupe in Rust rather than SQL (`SELECT DISTINCT`) so both lanes
        // run the same reduction: one provider holding two subjects is one
        // method, not two.
        let mut providers: Vec<String> = self
            .query_all::<OAuthIdentityModel>(cmd)
            .await?
            .into_iter()
            .map(|model| model.provider)
            .collect();
        providers.sort();
        providers.dedup();
        Ok(providers)
    }

    async fn count_live_files_by_thumbnail_key(
        &self,
        bucket_id: Option<Uuid>,
        key: &str,
    ) -> Result<u64, StoreError> {
        let cmd = Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "SELECT COUNT(*) AS n FROM files WHERE deleted_at IS NULL \
                   AND bucket_id IS NOT DISTINCT FROM $1 AND thumbnail_key = $2"
                .to_owned(),
            params: vec![
                bucket_id
                    .map(|id| Value::Uuid(id.to_string()))
                    .unwrap_or(Value::Null),
                Value::Text(key.to_owned()),
            ],
        };
        let reply = self.executor.execute(&cmd).await?;
        let Reply::Rows { rows, .. } = reply else {
            return Err(StoreError::Db("expected rows from a COUNT".to_owned()));
        };
        let n = rows
            .first()
            .and_then(|row| row.iter().next())
            .and_then(|v| match v {
                Value::I64(n) => Some(*n),
                Value::I32(n) => Some(i64::from(*n)),
                _ => None,
            })
            .unwrap_or(0);
        Ok(n as u64)
    }

    async fn create_api_key(&self, record: &i0721_proto::ApiKeyRecord) -> Result<(), StoreError> {
        let model = ApiKeyModel {
            id: record.id,
            user_id: record.user_id,
            name: record.name.clone(),
            key_prefix: record.key_prefix.clone(),
            key_hash: record.key_hash.clone(),
            // Parse the record's JSON text into a REAL JSONB value:
            // wrapping the string would store a JSON *string* and the
            // read path's `.to_string()` would then hand back a quoted
            // copy (the double-encode the round-trip test pins).
            // FAIL-CLOSED on a bad payload: a permissions column must not
            // silently degrade to the "empty = full" set, so an
            // unparseable scopes string is rejected at the write.
            scopes: serde_json::from_str(&record.scopes).map_err(|err| {
                StoreError::Db(format!(
                    "api key scopes must be valid JSON ({err}): {scopes:?}",
                    scopes = record.scopes
                ))
            })?,
            expires_at: record.expires_at.clone(),
            last_used_at: record.last_used_at.clone(),
            revoked_at: record.revoked_at.clone(),
            created_at: record.created_at,
        };
        let _ = self
            .query_one::<ApiKeyModel>(insert_command::<ApiKeyModel>(&model))
            .await?;
        Ok(())
    }

    async fn list_api_keys(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<i0721_proto::ApiKeyRecord>, StoreError> {
        let cmd = Command::Select {
            table: ApiKeyModel::TABLE.to_owned(),
            filter: Some(ApiKeyColumn::UserId.eq(user_id).into_inner()),
            order_by: vec![("created_at".to_owned(), false)],
            limit: None,
            offset: None,
            lock: None,
            distinct: false,
        };
        Ok(self
            .query_all::<ApiKeyModel>(cmd)
            .await?
            .into_iter()
            .map(api_key_model_to_record)
            .collect())
    }

    async fn revoke_api_key(&self, id: Uuid) -> Result<bool, StoreError> {
        let cmd = update_command::<ApiKeyModel>(
            vec![("revoked_at", Value::Text(Timestamp::now().to_string()))],
            ApiKeyColumn::Id
                .eq(id)
                .into_inner()
                .and(ApiKeyColumn::RevokedAt.is_null().into_inner()),
            false,
        )?;
        let reply = self.executor.execute(&cmd).await?;
        let Reply::Affected(n) = reply else {
            return Err(StoreError::Db("expected an affected count".to_owned()));
        };
        Ok(n > 0)
    }

    async fn update_api_key(
        &self,
        id: Uuid,
        name: &str,
        scopes_json: &str,
        expires_at: Option<&str>,
    ) -> Result<bool, StoreError> {
        // Same fail-closed write contract as `create_api_key`: the raw
        // text must parse before it can become the JSONB column, and a
        // bad payload is an ERROR (never a silently emptied permission
        // set).
        let scopes = serde_json::from_str::<serde_json::Value>(scopes_json).map_err(|err| {
            StoreError::Db(format!(
                "api key scopes must be valid JSON ({err}): {scopes_json:?}"
            ))
        })?;
        let cmd = update_command::<ApiKeyModel>(
            vec![
                ("name", Value::Text(name.to_owned())),
                ("scopes", Value::Json(scopes.to_string())),
                // `None` CLEARS the expiry: the whole-record edit
                // replaces every field it owns.
                (
                    "expires_at",
                    expires_at.map_or(Value::Null, |at| Value::Text(at.to_owned())),
                ),
            ],
            ApiKeyColumn::Id
                .eq(id)
                .into_inner()
                .and(ApiKeyColumn::RevokedAt.is_null().into_inner()),
            false,
        )?;
        let reply = self.executor.execute(&cmd).await?;
        let Reply::Affected(n) = reply else {
            return Err(StoreError::Db("expected an affected count".to_owned()));
        };
        Ok(n > 0)
    }

    async fn find_api_key_by_hash(
        &self,
        key_hash: &str,
    ) -> Result<Option<i0721_proto::ApiKeyRecord>, StoreError> {
        let cmd = Command::Select {
            table: ApiKeyModel::TABLE.to_owned(),
            filter: Some(
                ApiKeyColumn::KeyHash
                    .eq(key_hash)
                    .into_inner()
                    .and(ApiKeyColumn::RevokedAt.is_null().into_inner()),
            ),
            order_by: vec![],
            limit: None,
            offset: None,
            lock: None,
            distinct: false,
        };
        Ok(self
            .query_one::<ApiKeyModel>(cmd)
            .await?
            .map(api_key_model_to_record))
    }

    async fn find_api_key_by_id(
        &self,
        id: Uuid,
    ) -> Result<Option<i0721_proto::ApiKeyRecord>, StoreError> {
        let cmd = Command::Select {
            table: ApiKeyModel::TABLE.to_owned(),
            // By PRIMARY KEY, with NO state filter: ownership decisions
            // need the row whether it is live, expired or revoked.
            filter: Some(ApiKeyColumn::Id.eq(id).into_inner()),
            order_by: vec![],
            limit: None,
            offset: None,
            lock: None,
            distinct: false,
        };
        Ok(self
            .query_one::<ApiKeyModel>(cmd)
            .await?
            .map(api_key_model_to_record))
    }

    async fn touch_api_key(&self, id: Uuid) -> Result<(), StoreError> {
        let cmd = update_command::<ApiKeyModel>(
            vec![("last_used_at", Value::Text(Timestamp::now().to_string()))],
            ApiKeyColumn::Id.eq(id).into_inner(),
            false,
        )?;
        self.executor.execute(&cmd).await?;
        Ok(())
    }

    async fn get_setting(&self, key: &str) -> Result<Option<serde_json::Value>, StoreError> {
        let cmd = Command::Select {
            table: SettingModel::TABLE.to_owned(),
            filter: Some(SettingColumn::Key.eq(key).into_inner()),
            order_by: vec![],
            limit: None,
            offset: None,
            lock: None,
            distinct: false,
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

    async fn count_folder(
        &self,
        owner_id: Uuid,
        parent_id: Option<Uuid>,
        kind: Option<String>,
        buckets: Option<&[Uuid]>,
    ) -> Result<u64, StoreError> {
        let filter = folder_filter(owner_id, parent_id, kind, buckets);
        let cmd = Command::Count {
            table: FileModel::TABLE.to_owned(),
            filter: Some(filter),
        };
        let reply = self.executor.execute(&cmd).await?;
        match reply {
            lacrymira::Reply::Value(Some(lacrymira::Value::I64(count))) => Ok(count as u64),
            other => Err(StoreError::Db(format!(
                "expected a count reply from the files table, got {other:?}"
            ))),
        }
    }

    async fn list_folder(
        &self,
        owner_id: Uuid,
        parent_id: Option<Uuid>,
        kind: Option<String>,
        buckets: Option<&[Uuid]>,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<FileRecord>, StoreError> {
        let filter = folder_filter(owner_id, parent_id, kind, buckets);
        let cmd = Command::Select {
            table: FileModel::TABLE.to_owned(),
            filter: Some(filter),
            order_by: vec![("created_at".to_owned(), false)],
            limit: Some(limit),
            offset: Some(offset),
            lock: None,
            distinct: false,
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
        bucket_id: Option<Uuid>,
        name: &str,
        exclude: Option<Uuid>,
    ) -> Result<bool, StoreError> {
        // Entity-layer read (the raw COALESCE SQL hit the driver's
        // binary-decoding path for UUID params); the root/NULL folding
        // that COALESCE expressed happens here in Rust. The bucket
        // dimension folds the same way (R-G: same-layer uniqueness is
        // bucket-aware — identical names in different buckets coexist).
        let mut filter = FileColumn::DeletedAt
            .is_null()
            .into_inner()
            .and(FileColumn::OwnerId.eq(owner_id).into_inner())
            .and(FileColumn::Name.eq(name).into_inner());
        filter = match bucket_id {
            Some(bid) => filter.and(FileColumn::BucketId.eq(bid).into_inner()),
            None => filter.and(FileColumn::BucketId.is_null().into_inner()),
        };
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
            distinct: false,
        };
        Ok(!self.query_all::<FileModel>(cmd).await?.is_empty())
    }

    async fn create_folder(
        &self,
        owner_id: Uuid,
        parent_id: Option<Uuid>,
        name: &str,
        bucket_id: Option<Uuid>,
    ) -> Result<FileRecord, StoreError> {
        if self
            .folder_name_taken(owner_id, parent_id, bucket_id, name, None)
            .await?
        {
            return Err(StoreError::Conflict(format!(
                "name {name:?} already exists in this folder"
            )));
        }
        // R-G: a child inherits its parent's bucket — a caller passing a
        // different bucket than the parent's is refused, not silently
        // re-seated (the same refusal `set_parent` gives a move).
        if let Some(parent) = parent_id {
            let parent_row = self.get_file(parent).await?;
            if parent_row.map(|r| r.bucket_id) != Some(bucket_id) {
                return Err(StoreError::Conflict(
                    "cannot create a folder across buckets".to_owned(),
                ));
            }
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
            bucket_id,
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
        // Same-layer uniqueness at the TARGET (the RPC pre-checks too;
        // the store layer is the final gate against a raw 23505).
        if let Some(current) = self.get_file(id).await? {
            if self
                .folder_name_taken(
                    current.owner_id,
                    parent_id,
                    current.bucket_id,
                    &current.name,
                    Some(id),
                )
                .await?
            {
                return Err(StoreError::Conflict(format!(
                    "name {:?} already exists in the target folder",
                    current.name
                )));
            }
            // R-G: the folder tree is per bucket. A move whose target
            // parent sits in another bucket is refused here — the ONE
            // guard every move path shares (the RPC lane has no second
            // bypass).
            let parent_bucket = match parent_id {
                Some(pid) => match self.get_file(pid).await? {
                    Some(parent) => parent.bucket_id,
                    None => {
                        return Err(StoreError::Conflict(
                            "the target folder does not exist".to_owned(),
                        ))
                    }
                },
                // Root: the tree is per bucket, so a row moved to its own
                // root keeps the bucket it already lives in.
                None => current.bucket_id,
            };
            if parent_bucket != current.bucket_id {
                return Err(StoreError::Conflict(
                    "cannot move a row into a folder in another bucket".to_owned(),
                ));
            }
        }
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
            false,
        )?;
        self.executor.execute(&cmd).await?;
        Ok(())
    }

    async fn ancestor_ids(&self, id: Uuid) -> Result<Vec<Uuid>, StoreError> {
        // Iterative walk through the entity layer (the raw recursive
        // CTE hits the driver's binary-decoding path for UUID columns).
        let mut chain = Vec::new();
        let mut visited = std::collections::HashSet::from([id]);
        let mut cursor = id;
        loop {
            let Some(row) = self.get_file(cursor).await? else {
                break;
            };
            let Some(parent) = row.parent_id else {
                break;
            };
            // Cycle guard (corrupted parent chains).
            if !visited.insert(parent) {
                break;
            }
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
                distinct: false,
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
                distinct: false,
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
            distinct: false,
        };
        Ok(self
            .query_one::<ShareModel>(cmd)
            .await?
            .map(share_model_to_record))
    }

    async fn find_share_by_file(
        &self,
        owner_id: Uuid,
        file_id: Uuid,
    ) -> Result<Option<ShareRecord>, StoreError> {
        let cmd = Command::Select {
            table: ShareModel::TABLE.to_owned(),
            filter: Some(
                ShareColumn::FileId
                    .eq(file_id)
                    .into_inner()
                    .and(ShareColumn::OwnerId.eq(owner_id).into_inner()),
            ),
            // Newest first: `create_share`'s "return the existing row" is
            // best-effort (the table has no unique index on `file_id`), so
            // a raced pair of legacy rows resolves to the latest.
            order_by: vec![("created_at".to_owned(), false)],
            limit: None,
            offset: None,
            lock: None,
            distinct: false,
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
            distinct: false,
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

    async fn delete_shares_for_files(&self, file_ids: &[Uuid]) -> Result<u64, StoreError> {
        if file_ids.is_empty() {
            return Ok(0);
        }
        let filter = ShareColumn::FileId.in_(file_ids.to_vec()).into_inner();
        let cmd = Command::Delete {
            table: ShareModel::TABLE.to_owned(),
            filter: Some(filter),
        };
        let reply = self.executor.execute(&cmd).await?;
        match reply {
            lacrymira::Reply::Affected(n) => Ok(n),
            other => Err(StoreError::Db(format!(
                "expected an affected-rows reply from the shares table, got {other:?}"
            ))),
        }
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
            false,
        )?;
        self.executor.execute(&cmd).await?;
        Ok(())
    }
}
