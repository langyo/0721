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

/// The baseline schema plus the settings seed.
///
/// The seed row uses `ON CONFLICT DO NOTHING`, so a database whose
/// operator deleted the row keeps that choice on re-run.
#[must_use]
pub fn migrations() -> Vec<Migration> {
    vec![
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
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_version_sorted_and_unique() {
        let migrations = migrations();
        let mut versions: Vec<_> = migrations.iter().map(|m| m.version.as_str()).collect();
        let mut sorted = versions.clone();
        sorted.sort_unstable();
        assert_eq!(versions, sorted, "runner sorts anyway; keep it true here");
        versions.dedup();
        assert_eq!(versions.len(), migrations.len());
        assert!(migrations.iter().all(|m| !m.sql.trim().is_empty()));
    }

    #[test]
    fn seed_json_is_dollar_quoted_and_matches_the_default_config() {
        let seed = migrations()
            .into_iter()
            .find(|m| m.version == "20261122000002")
            .expect("seed migration present");
        let json_str = seed
            .sql
            .split("$i0721$")
            .nth(1)
            .and_then(|rest| rest.split("$i0721$").next())
            .expect("seed SQL carries dollar-quoted JSON");
        assert!(
            !json_str.contains('$'),
            "the default config payload must stay inside the dollar-quoted literal"
        );
        let json: serde_json::Value = serde_json::from_str(json_str).expect("seed JSON parses");
        assert_eq!(
            json,
            serde_json::to_value(i0721_proto::default_site_config()).unwrap()
        );
    }
}
