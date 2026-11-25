//! The migration set, executed by [`lacrymira_pg::MigrationRunner`].
//!
//! Bookkeeping is the `seaql_migrations(version TEXT PK, applied_at
//! BIGINT)` table the legacy sea-orm engine wrote; the runner reads
//! recorded versions as "already applied" — including pre-history rows
//! with no file here — so it can adopt a database sea-orm has been
//! migrating and pick up exactly where sea-orm left off.
//!
//! The dedupe key is `(owner_id, sha256)` as a *partial* unique index
//! over live rows: dedupe is per owner (never leak one user's file to
//! another), and a soft-deleted file frees its hash for a re-upload.
//! `storage_key` stays content-addressed (hex SHA-256), so rows from
//! different owners can share one blob — its index is deliberately
//! non-unique; blob reclamation when the last live row goes away is the
//! caller's job (GC hardening lands with the netdisk waves).

use lacrymira_pg::Migration;
use uuid::Uuid;

/// The migration-0004 legacy-dedup UPDATE (extracted so the PG lane can
/// re-run it against a hand-planted pre-migration shape — the migration
/// itself is a one-shot on a fresh database, but the cleansing logic
/// must be provable on real PostgreSQL).
pub const MIGRATION_0004_DEDUPE_SQL: &str = r#"WITH ranked AS (
    SELECT id, owner_id, name,
           COALESCE(parent_id, '00000000-0000-0000-0000-000000000000'::uuid) AS layer_key,
           ROW_NUMBER() OVER (
               PARTITION BY owner_id,
                            COALESCE(parent_id, '00000000-0000-0000-0000-000000000000'::uuid),
                            name
               ORDER BY created_at, id
           ) AS rn
    FROM files WHERE deleted_at IS NULL
),
existing AS (
    SELECT DISTINCT f.owner_id, f.name,
           COALESCE(f.parent_id, '00000000-0000-0000-0000-000000000000'::uuid) AS layer_key
    FROM files f WHERE f.deleted_at IS NULL
),
renamers AS (
    -- One row per renaming file: the k-th duplicate of a name (by rn)
    -- carries rename_rank = k. Computed here, BEFORE the candidate
    -- cross join, so the window counts distinct files rather than
    -- (file x candidate) pairs.
    SELECT r.id, r.name, r.rn, r.owner_id, r.layer_key,
           ROW_NUMBER() OVER (PARTITION BY r.owner_id, r.layer_key, r.name
                              ORDER BY r.rn) AS rename_rank
    FROM ranked r
    WHERE r.rn > 1
),
free AS (
    -- Collision-safe pairing: each renaming row ranks ITS OWN free
    -- candidates (free_rank, partitioned per row), and the k-th
    -- renaming row of a group (by rn) takes ITS k-th free candidate,
    -- where "free" means no live row (pre-update snapshot in `existing`)
    -- already carries "name (n)". Because free_rank and rename_rank are
    -- both distinct within a group, the (k, k) pairing can never hand
    -- the same candidate to two rows, and cross-group renames carry
    -- different base names so they cannot collide either. A group whose
    -- renaming rows outnumber the free candidates (a name holding
    -- 10'000 suffixed variants) simply leaves the tail unrenamed, which
    -- the CREATE UNIQUE INDEX below then rejects loudly -- fail-closed
    -- with a precise error rather than a silent bad name.
    SELECT r.id,
           r.name || ' (' || gs.n || ')' AS new_name,
           ROW_NUMBER() OVER (PARTITION BY r.owner_id, r.layer_key, r.name, r.id
                              ORDER BY gs.n) AS free_rank,
           r.rename_rank
    FROM renamers r
    CROSS JOIN LATERAL generate_series(2, 10000) AS gs(n)
    WHERE NOT EXISTS (
          SELECT 1 FROM existing e
          WHERE e.owner_id = r.owner_id
            AND e.layer_key = r.layer_key
            AND e.name = r.name || ' (' || gs.n || ')'
      )
)
UPDATE files f
SET name = fr.new_name
FROM free fr
WHERE fr.id = f.id
  AND fr.free_rank = fr.rename_rank"#;

/// The API-key scope shape every row must carry from the permission
/// wave on: an OBJECT with an `ops` array, e.g. `{"ops":["read"]}`.
///
/// Before that wave the column held a bare `[]` nobody executed, which
/// every reader treated as "all permissions". Permissions ARE executed
/// now, so that shape must land as the EMPTY set instead — a key whose
/// owner never picked permissions cannot act until `keys.update` grants
/// them (fail-closed: the normalization can only ever narrow).
pub const API_KEY_SCOPES_SHAPE_SQL: &str = r#"'{"ops":[]}'::jsonb"#;

/// The migration-0006 legacy normalization, extracted so the PG lane can
/// re-run it against a hand-planted legacy row (the migration itself is
/// a one-shot on a fresh database). Idempotent: a row that already has an
/// object `scopes` is untouched, so the test can re-run it at will.
pub const MIGRATION_0006_NORMALIZE_SQL: &str = r#"UPDATE api_keys
SET scopes = '{"ops":[]}'::jsonb
WHERE scopes IS NULL
   OR jsonb_typeof(scopes) IS DISTINCT FROM 'object'
   OR jsonb_typeof(scopes -> 'ops') IS DISTINCT FROM 'array'"#;

/// The zero UUID — the COALESCE stand-in for NULL `bucket_id` / NULL
/// `parent_id` inside the unique-index expressions (same convention as
/// the existing `parent_id` folding).
pub const ZERO_UUID_SQL: &str = "'00000000-0000-0000-0000-000000000000'::uuid";

/// The old same-layer name index 0007 replaces (referenced by name in
/// the DROP and by the PG lane's rebuild statements — the lane drops and
/// recreates it around hand-planted rows).
pub const FILES_OWNER_FOLDER_NAME_LIVE_KEY_SQL: &str = "files_owner_folder_name_live_key";

/// The bucket-aware same-layer name index 0007 installs (statement 5).
pub const FILES_OWNER_BUCKET_FOLDER_NAME_LIVE_KEY_SQL: &str =
    "CREATE UNIQUE INDEX files_owner_bucket_folder_name_live_key \
     ON files (owner_id, COALESCE(bucket_id, '00000000-0000-0000-0000-000000000000'::uuid), \
     COALESCE(parent_id, '00000000-0000-0000-0000-000000000000'::uuid), name) \
     WHERE deleted_at IS NULL";

/// The bucket-dimension dedupe index 0007 installs (statement 6a). The
/// legacy `(owner_id, sha256)` index is deliberately KEPT (spec Q1).
pub const FILES_OWNER_BUCKET_SHA256_LIVE_KEY_SQL: &str =
    "CREATE UNIQUE INDEX files_owner_bucket_sha256_live_key \
     ON files (owner_id, COALESCE(bucket_id, '00000000-0000-0000-0000-000000000000'::uuid), sha256) \
     WHERE deleted_at IS NULL";

/// The `(bucket_id, storage_key)` lookup index the unregistered-object
/// lane's set difference uses (draft §7.2).
pub const FILES_BUCKET_STORAGE_KEY_INDEX_SQL: &str =
    "CREATE INDEX files_bucket_storage_key_idx ON files (bucket_id, storage_key)";

/// The two new tables plus the nullable `files.bucket_id` column
/// (statements 1–2; parameterless, so the schema half can be applied
/// without knowing the deployment's backend).
pub const MIGRATION_0007_SCHEMA_SQL: &str = r#"
CREATE TABLE storage_buckets (
    id UUID PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    backend TEXT NOT NULL CHECK (backend IN ('s3','local','ipfs')),
    location TEXT NOT NULL,
    endpoint_ref TEXT NOT NULL,
    anonymous_read BOOLEAN NOT NULL DEFAULT FALSE,
    grant_on_signup BOOLEAN NOT NULL DEFAULT FALSE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    is_bootstrap BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE bucket_grants (
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    bucket_id UUID NOT NULL REFERENCES storage_buckets (id) ON DELETE CASCADE,
    can_read BOOLEAN NOT NULL DEFAULT FALSE,
    can_write BOOLEAN NOT NULL DEFAULT FALSE,
    can_share BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, bucket_id)
);
CREATE UNIQUE INDEX storage_buckets_single_ipfs ON storage_buckets ((true)) WHERE backend = 'ipfs';
ALTER TABLE files ADD COLUMN bucket_id UUID REFERENCES storage_buckets (id);
"#;

/// The legacy-other-backend bootstrap row INSERT (statement 3b): one
/// disabled bucket per legacy label found in `files.storage_backend`.
/// The lane formats the label in ([`legacy_bucket_statement`]); this
/// constant documents the shape the PG lane asserts against.
pub const MIGRATION_0007_LEGACY_BUCKET_SQL: &str = r#"
INSERT INTO storage_buckets (id, slug, display_name, backend, location, endpoint_ref,
                             anonymous_read, grant_on_signup, enabled, is_bootstrap)
SELECT $1, 'bootstrap-' || $2, 'bootstrap (' || $2 || ') — legacy, not registered', $2,
       'legacy:' || $2 || ' (unknown — register its bucket to serve)', 'legacy_unknown',
       TRUE, FALSE, FALSE, TRUE
WHERE $2 IN ('s3', 'local', 'ipfs')
  AND NOT EXISTS (SELECT 1 FROM storage_buckets WHERE slug = 'bootstrap-' || $2)
ON CONFLICT (slug) DO NOTHING"#;

/// The bootstrap bucket upsert + the full backfill + the zero-NULL
/// assertion (statements 3a, 3c, 3d) — the migration's one stateful
/// half, extracted so the PG lane can re-run it against hand-planted
/// pre-migration shapes.
///
/// `$1` = the bootstrap bucket's id (uuid; consumed only when the row is
/// first created), `$2` = its `location` (text, from the deployment env),
/// `$3` = the current backend label (text).
///
/// The upsert keeps the FIRST id stable across re-runs
/// (`ON CONFLICT (slug) DO UPDATE SET slug = storage_buckets.slug
/// RETURNING id`); the backfill seats every row by its own
/// `storage_backend` label, folding unknown/unlabelled rows onto the
/// current backend's bootstrap bucket; the `DO` block fails the whole
/// statement (and, inside the migration, the whole transaction) when any
/// row is still left without a bucket.
pub const MIGRATION_0007_BOOTSTRAP_SQL: &str = r#"WITH ensure AS (
    INSERT INTO storage_buckets (id, slug, display_name, backend, location, endpoint_ref,
                                 anonymous_read, grant_on_signup, enabled, is_bootstrap)
    VALUES ($1, 'bootstrap-' || $3, 'bootstrap (' || $3 || ')', $3, $2, 'legacy_' || $3 || '_env',
            TRUE, TRUE, TRUE, TRUE)
    ON CONFLICT (slug) DO UPDATE SET slug = storage_buckets.slug
    RETURNING id
),
current_bucket AS (
    SELECT id FROM ensure
    UNION ALL
    SELECT id FROM storage_buckets WHERE slug = 'bootstrap-' || $3
    LIMIT 1
),
legacy_labels AS (
    SELECT DISTINCT storage_backend AS label FROM files
    WHERE storage_backend IN ('s3', 'local', 'ipfs') AND storage_backend <> $3
),
planted AS (
    INSERT INTO storage_buckets (id, slug, display_name, backend, location, endpoint_ref,
                                 anonymous_read, grant_on_signup, enabled, is_bootstrap)
    SELECT gen_random_uuid(), 'bootstrap-' || label,
           'bootstrap (' || label || ') — legacy, not registered', label,
           'legacy:' || label || ' (unknown — register its bucket to serve)', 'legacy_unknown',
           TRUE, FALSE, FALSE, TRUE
    FROM legacy_labels
    ON CONFLICT (slug) DO NOTHING
    RETURNING id, slug
)
UPDATE files
SET bucket_id = COALESCE(
        (SELECT b.id FROM storage_buckets b
          WHERE b.slug = 'bootstrap-' || files.storage_backend
            AND files.storage_backend IN ('s3', 'local', 'ipfs') LIMIT 1),
        (SELECT id FROM current_bucket))
WHERE bucket_id IS NULL;
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM files WHERE bucket_id IS NULL) THEN
        RAISE EXCEPTION 'migration 0007 backfill left rows without a bucket';
    END IF;
END $$;"#;

/// One legacy bootstrap bucket row, minted by the `legacy_labels` CTE
/// inside [`MIGRATION_0007_BOOTSTRAP_SQL`] from a distinct
/// `files.storage_backend` label (R-H statement 3b): `enabled = FALSE`,
/// `anonymous_read = TRUE`, `grant_on_signup = FALSE` (satisfies R-A's
/// "default-open buckets are anonymous-readable" rule), `location`
/// spelled out as unknown, `endpoint_ref = legacy_unknown`.
///
/// The labels are derived IN SQL from the live rows — no Rust-side
/// pre-scan can be authoritative once the migration starts.

/// The deployment facts 0007's bootstrap half needs, resolved by the
/// caller (fail-closed: no values here, no migration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapBucketParams {
    /// The bootstrap bucket's id (minted once per process start).
    pub id: Uuid,
    /// The bucket's `location` (env-provided).
    pub location: String,
    /// The current backend label.
    pub backend: String,
}

/// Distinct legacy backend labels found in the `files` table (statement
/// 3b's row source). Pure function so the startup lane can test it.
#[must_use]
pub fn legacy_backend_labels(present: &[&str], current: &str) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for label in present {
        if matches!(*label, "s3" | "local" | "ipfs") && *label != current && seen.insert(*label) {
            out.push((*label).to_owned());
        }
    }
    out
}

/// The baseline schema plus the settings seed.
///
/// The seed row uses `ON CONFLICT DO NOTHING`, so a database whose
/// operator deleted the row keeps that choice on re-run.
///
/// `bootstrap` carries the deployment facts 0007's bootstrap half needs;
/// `None` omits 0007 entirely — the caller must have already decided
/// (fail-closed, R-U) that the bucket migration is not pending, and the
/// runner then simply never sees it.
#[must_use]
pub fn migrations(bootstrap: Option<&BootstrapBucketParams>) -> Vec<Migration> {
    let mut out = vec![
        Migration::new(
            "20261122000001",
            "baseline schema (users, oauth_identities, files, shares, api_keys, settings)",
            r#"
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                username TEXT NOT NULL UNIQUE,
                display_name TEXT NOT NULL,
                role TEXT NOT NULL CHECK (role IN ('admin', 'member')),
                password_hash TEXT,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now()
            );

            CREATE TABLE oauth_identities (
                provider TEXT NOT NULL,
                subject TEXT NOT NULL,
                user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
                PRIMARY KEY (provider, subject)
            );

            CREATE TABLE files (
                id UUID PRIMARY KEY,
                owner_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
                parent_id UUID REFERENCES files (id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                kind TEXT NOT NULL CHECK (kind IN ('image', 'document', 'media', 'archive', 'other')),
                mime TEXT NOT NULL,
                size BIGINT NOT NULL,
                sha256 TEXT NOT NULL,
                storage_backend TEXT NOT NULL DEFAULT 'local',
                storage_key TEXT NOT NULL,
                thumbnail_key TEXT,
                original_name TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                deleted_at TEXT
            );

            CREATE INDEX files_created_at_idx ON files (created_at DESC);

            CREATE UNIQUE INDEX files_sha256_live_key
                ON files (sha256) WHERE deleted_at IS NULL;
            CREATE UNIQUE INDEX files_storage_key_live_key
                ON files (storage_key) WHERE deleted_at IS NULL;

            CREATE TABLE shares (
                token TEXT PRIMARY KEY,
                file_id UUID NOT NULL REFERENCES files (id) ON DELETE CASCADE,
                owner_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
                expires_at TEXT,
                allow_preview BOOLEAN NOT NULL DEFAULT FALSE,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now()
            );

            CREATE TABLE api_keys (
                id UUID PRIMARY KEY,
                user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                key_prefix TEXT NOT NULL,
                key_hash TEXT NOT NULL,
                scopes JSONB NOT NULL DEFAULT '[]'::jsonb,
                last_used_at TEXT,
                revoked_at TEXT,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now()
            );

            CREATE TABLE settings (
                key TEXT PRIMARY KEY,
                value JSONB NOT NULL
            );
            "#,
        ),
        Migration::new(
            "20261122000002",
            "seed the default site settings row",
            // Dollar-quoting: the JSON payload is data, not SQL text — a
            // `'` inside a future default value must never be able to
            // escape the literal (string interpolation would break there).
            format!(
                "INSERT INTO settings (key, value) VALUES ('site', $i0721${json}$i0721$::jsonb) \
                 ON CONFLICT (key) DO NOTHING;",
                json = serde_json::to_string(&i0721_proto::default_site_config())
                    .expect("the default site config is always serializable")
            ),
        ),
        Migration::new(
            "20261122000003",
            "scope file dedupe per owner and share storage keys",
            // Dedupe is per owner: two users uploading the same bytes both
            // get a row (one DTO must never leak another user's file).
            // storage_key stays content-addressed, so their rows share one
            // blob — the index is therefore deliberately NOT unique.
            r#"
            DROP INDEX IF EXISTS files_sha256_live_key;
            DROP INDEX IF EXISTS files_storage_key_live_key;

            CREATE UNIQUE INDEX files_owner_sha256_live_key
                ON files (owner_id, sha256) WHERE deleted_at IS NULL;
            CREATE INDEX files_storage_key_live_key
                ON files (storage_key) WHERE deleted_at IS NULL;
            "#,
        ),
        Migration::new(
            "20261122000004",
            "netdisk: folders, per-user quota, folder-name uniqueness (reentrant + legacy dedupe)",
            format!(
                r#"ALTER TABLE users ADD COLUMN IF NOT EXISTS quota_bytes BIGINT;
            ALTER TABLE files DROP CONSTRAINT IF EXISTS files_kind_check;
            ALTER TABLE files ADD CONSTRAINT files_kind_check
                CHECK (kind IN ('folder', 'image', 'document', 'media', 'archive', 'other'));
            {};
            CREATE UNIQUE INDEX IF NOT EXISTS files_owner_folder_name_live_key
                ON files (owner_id, COALESCE(parent_id, '00000000-0000-0000-0000-000000000000'::uuid), name)
                WHERE deleted_at IS NULL;"#,
                MIGRATION_0004_DEDUPE_SQL
            ),
        ),
        Migration::new(
            "20261122000005",
            "api_keys: unique index on key_hash for the MCP verify path",
            // Unique is correct: the same key (same bytes) produces the
            // same SHA-256 hash; two rows with the same hash would be
            // the same key, which find_api_key_by_hash would resolve
            // ambiguously. Idempotent via IF NOT EXISTS.
            r#"
            CREATE UNIQUE INDEX IF NOT EXISTS api_keys_key_hash_key
                ON api_keys (key_hash);
            "#,
        ),
        Migration::new(
            "20261122000006",
            "api_keys: optional expires_at + fail-closed legacy scopes normalization",
            // The expiry column is nullable TEXT holding RFC 3339, the
            // same shape as `revoked_at` / `last_used_at` (nullable time
            // columns stay text; see `entity.rs`).
            //
            // The normalization is the OTHER half: from this wave on the
            // permission set is ENFORCED, so a legacy row's bare `[]`
            // (read by nobody, meaning nothing) must not widen into
            // "everything". Any `scopes` that is not an object with an
            // `ops` array becomes the empty permission set.
            format!(
                r#"ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS expires_at TEXT;
            {};
            ALTER TABLE api_keys ALTER COLUMN scopes SET DEFAULT {shape};"#,
                MIGRATION_0006_NORMALIZE_SQL,
                shape = API_KEY_SCOPES_SHAPE_SQL,
            ),
        ),
    ];

    // 0007 (the bucket wave) is appended ONLY when the caller resolved
    // the bootstrap facts — the R-U wiring: a missing
    // `I0721_STORAGE_BACKEND` while 0007 is pending must fail the start
    // BEFORE any migration runs, never run the migration half-built.
    let Some(bootstrap) = bootstrap else {
        return out;
    };
    out.push(Migration::new(
        "20261122000007",
        "bucket registry: storage_buckets + bucket_grants, files.bucket_id, bootstrap buckets, bucket-aware indexes",
        format!(
            r#"{schema};
            {};
            DROP INDEX IF EXISTS {old_layer};
            {layer_index};
            {dedupe_index};
            {bucket_key_index};"#,
            bootstrap_sql(bootstrap),
            schema = MIGRATION_0007_SCHEMA_SQL.trim(),
            old_layer = FILES_OWNER_FOLDER_NAME_LIVE_KEY_SQL,
            layer_index = FILES_OWNER_BUCKET_FOLDER_NAME_LIVE_KEY_SQL,
            dedupe_index = FILES_OWNER_BUCKET_SHA256_LIVE_KEY_SQL,
            bucket_key_index = FILES_BUCKET_STORAGE_KEY_INDEX_SQL,
        ),
    ));
    out
}

/// Format the bootstrap half of 0007. The runner's SQL lane cannot bind
/// parameters, so the deployment facts (backend label, location, minted
/// id) are formatted in from Rust — same precedent as migration 0002's
/// `format!` JSON. The backend label is a CHECK-constrained tag with a
/// fixed shape (validated by [`bootstrap_backend_for_startup`]'s caller),
/// and the location is dollar-quoted as data so an arbitrary env value
/// cannot break out of the literal.
fn bootstrap_sql(bootstrap: &BootstrapBucketParams) -> String {
    MIGRATION_0007_BOOTSTRAP_SQL
        .replace("$1", &format!("'{}'::uuid", bootstrap.id))
        .replace("$2", &format!("$loc${}$loc$", bootstrap.location))
        .replace("$3", &format!("'{}'", bootstrap.backend))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_version_sorted_and_unique() {
        let all = super::migrations(Some(&bootstrap_params()));
        let mut versions: Vec<_> = all.iter().map(|m| m.version.as_str()).collect();
        let mut sorted = versions.clone();
        sorted.sort_unstable();
        assert_eq!(versions, sorted, "runner sorts anyway; keep it true here");
        versions.dedup();
        assert_eq!(versions.len(), all.len());
        assert!(all.iter().all(|m| !m.sql.trim().is_empty()));
        // Without the bootstrap facts 0007 is absent: the R-U wiring's
        // "no values, no migration" shape.
        let without = super::migrations(None);
        assert!(
            without.iter().all(|m| m.version != "20261122000007"),
            "0007 is only appended when the bootstrap facts are known"
        );
        assert_eq!(without.len(), 6);
        assert_eq!(all.len(), 7);
    }

    /// A fixed bootstrap parameter set for the static tests.
    fn bootstrap_params() -> BootstrapBucketParams {
        BootstrapBucketParams {
            id: Uuid::nil(),
            location: "i0721-media".to_owned(),
            backend: "local".to_owned(),
        }
    }

    #[test]
    fn seed_json_is_dollar_quoted_and_matches_the_default_config() {
        let seed = migrations(Some(&bootstrap_params()))
            .into_iter()
            .find(|m| m.version == "20261122000002")
            .expect("seed migration present");
        let json_str = seed
            .sql
            .split("$i0721$")
            .nth(1)
            .and_then(|rest| rest.split("$i0721$").next())
            .expect("seed SQL carries dollar-quoted JSON");
        assert_eq!(
            seed.sql.matches("$i0721$").count(),
            2,
            "the statement must carry exactly the two dollar-quote delimiters"
        );
        // serde_json keeps `$` verbatim (only control characters are
        // escaped), so barring the raw dollar is sufficient and can really
        // fire: a future default value containing `$` fails here.
        assert!(
            !json_str.contains('$'),
            "the payload carries no dollar sign, so no dollar-quote tag can be formed between the payload and the closing delimiter"
        );
    }

    #[test]
    fn migration_0006_normalizes_legacy_scopes_and_adds_the_expiry_column() {
        let migration = migrations(Some(&bootstrap_params()))
            .into_iter()
            .find(|m| m.version == "20261122000006")
            .expect("the permission wave's migration is present");
        assert!(
            migration
                .sql
                .contains("ADD COLUMN IF NOT EXISTS expires_at TEXT"),
            "the expiry column is nullable TEXT (RFC 3339), added reentrantly"
        );
        assert!(
            migration
                .sql
                .contains(MIGRATION_0006_NORMALIZE_SQL.trim_end_matches(';')),
            "the migration runs the SHARED normalization the PG lane re-runs"
        );
        // The shape constant is the one the normalization writes and the
        // column default carries — one source of truth, no drift.
        assert!(
            MIGRATION_0006_NORMALIZE_SQL.contains(API_KEY_SCOPES_SHAPE_SQL),
            "the normalization writes exactly the documented shape"
        );
        assert!(
            migration
                .sql
                .contains(&format!("SET DEFAULT {API_KEY_SCOPES_SHAPE_SQL}")),
            "the column default is the normalized shape, not the legacy []"
        );
        // Fail-closed: the normalization may only ever NARROW a row into
        // the empty-ops object, so the replacement text must be the
        // empty array (never a populated one).
        assert!(
            API_KEY_SCOPES_SHAPE_SQL.contains(r#""ops":[]"#),
            "normalization never grants an op"
        );
    }
}
