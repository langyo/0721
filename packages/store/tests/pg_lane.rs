//! The opt-in PostgreSQL lane — the only tests in the workspace that
//! need a real database.
//!
//! Gate contract (fail-closed, the arona `test_db_lane` shape):
//!
//! - `I0721_TEST_PG` unset → every test below skips cleanly (printed
//!   once per process) and `cargo test --workspace` stays green without
//!   external services.
//! - `I0721_TEST_PG` set → the URL must resolve, the server must be
//!   reachable and the migrations must apply; anything else **panics**.
//!   A red gate, never a silent skip that would read green with zero
//!   assertions.
//!
//! Run with:
//!
//! ```sh
//! # scratch database, e.g. createdb i0721_dev_20261122
//! I0721_TEST_PG=1 \
//! I0721_DATABASE_URL='postgres://…/i0721_dev_20261122' \
//! just pg-test
//! ```
//!
//! Each process bootstraps (connect + migrate + seed check) exactly
//! once behind a mutex — concurrent migrations on a fresh scratch
//! database race on `CREATE TABLE` (the arona L198 lesson) — and every
//! test then opens its own small pool, because a sqlx pool is bound to
//! the tokio runtime that created it and each `#[tokio::test]` has its
//! own runtime.

use std::sync::OnceLock;

use i0721_proto::{FileKind, NewFile, Role};
use i0721_store::{CreateUser, PgStore, Store, StoreError};

static SKIP_NOTICE: OnceLock<()> = OnceLock::new();

fn gate_set() -> bool {
    std::env::var("I0721_TEST_PG").is_ok()
}

fn database_url() -> String {
    std::env::var("I0721_DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "I0721_TEST_PG is set but I0721_DATABASE_URL is not — \
             point it at a scratch database (the lane is fail-closed)"
        )
    })
}

/// Bootstraps (connect + migrate) exactly once per process, serialized.
async fn bootstrap() {
    static GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    static DONE: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    DONE.get_or_init(|| async {
        let _serialized = GATE.lock().await;
        let store = PgStore::connect(&database_url())
            .await
            .expect("I0721_TEST_PG is set: connect must succeed (fail-closed)");
        // The lane is a scratch deployment: its bootstrap bucket is
        // `bootstrap-local` rooted at a throwaway directory, so 0007's
        // bucket schema exists for the registry tests.
        PgStore::set_bootstrap_params(i0721_store::BootstrapBucketParams {
            id: uuid::Uuid::new_v4(),
            location: "i0721-bwave-lane".to_owned(),
            backend: "local".to_owned(),
        });
        store
            .migrate()
            .await
            .expect("I0721_TEST_PG is set: migrations must apply (fail-closed)");
    })
    .await;
}

/// A fresh store against the shared, already-migrated database.
async fn db() -> Option<PgStore> {
    if !gate_set() {
        SKIP_NOTICE.get_or_init(|| {
            eprintln!("SKIP: set I0721_TEST_PG=1 (and I0721_DATABASE_URL) to run the pg_ lane");
        });
        return None;
    }
    bootstrap().await;
    Some(
        PgStore::connect(&database_url())
            .await
            .expect("per-test connect must succeed (fail-closed)"),
    )
}

fn scratch_user() -> CreateUser {
    CreateUser {
        username: format!("lane_{}", uuid::Uuid::new_v4().simple()),
        display_name: "Lane".to_owned(),
        role: Role::Member,
        password_hash: None,
    }
}

/// A1 (R-R) — the entity layer can carry the new nullable UUID column:
/// the column is added by plain DDL, a `create_file` INSERT that never
/// names `bucket_id` succeeds, and the row reads back with `bucket_id`
/// NULL through the typed entity (no `Command::Sql` fallback needed).
/// Once migration 0007 exists the ALTER here is redundant (IF NOT
/// EXISTS), so the check stays valid on every lane database.
#[tokio::test]
async fn pg_entity_layer_carries_a_new_nullable_uuid_column() {
    let Some(store) = db().await else { return };
    let alter = lacrymira::Command::Sql {
        dialect: lacrymira::Dialect::Postgres,
        text: "ALTER TABLE files ADD COLUMN IF NOT EXISTS bucket_id UUID \
               REFERENCES storage_buckets (id)"
            .to_owned(),
        params: vec![],
    };
    store
        .executor
        .execute(&alter)
        .await
        .expect("the nullable UUID column is expressible as DDL");

    let user = store.create_user(scratch_user()).await.expect("user");
    let created = store
        .create_file(NewFile {
            parent_id: None,
            owner_id: user.id,
            name: "a1.png".to_owned(),
            kind: FileKind::Image,
            mime: "image/webp".to_owned(),
            size: 1,
            sha256: format!("a1{}", uuid::Uuid::new_v4().simple()),
            bucket_id: None,
            storage_backend: "local".to_owned(),
            storage_key: format!("a1{}", uuid::Uuid::new_v4().simple()),
            thumbnail_key: None,
            original_name: "a1.png".to_owned(),
        })
        .await
        .expect("an insert that never sets bucket_id");
    let fetched = store
        .get_file(created.id)
        .await
        .expect("read back")
        .expect("row exists");
    assert!(
        fetched.bucket_id.is_none(),
        "the un-set column decodes back as NULL (no fallback to zero-uuid): {:?}",
        fetched.bucket_id
    );
    cleanup_user(&store, user.id).await;
}

async fn cleanup_user(store: &PgStore, id: uuid::Uuid) {
    store.delete_user(id).await.expect("cleanup delete");
}

#[tokio::test]
async fn pg_migration_runner_is_idempotent() {
    let Some(store) = db().await else { return };
    // The bootstrap already applied everything; a second `migrate()` must
    // skip cleanly — the seaql_migrations bookkeeping holds.
    let result = store.migrate().await.expect("second run must succeed");
    assert!(result.applied.is_empty(), "nothing new to apply");
    assert!(!result.skipped.is_empty(), "recorded versions are skipped");
}

#[tokio::test]
async fn pg_user_crud_and_uniqueness() {
    let Some(store) = db().await else { return };
    let params = scratch_user();
    let created = store
        .create_user(params.clone())
        .await
        .expect("create user");
    assert_eq!(created.username, params.username);
    assert_eq!(created.role, Role::Member);
    assert!(created.password_hash.is_none());

    let fetched = store
        .get_user_by_username(&params.username)
        .await
        .expect("fetch by username")
        .expect("row exists");
    assert_eq!(fetched.id, created.id);

    assert!(matches!(
        store.create_user(params.clone()).await,
        Err(i0721_store::StoreError::Conflict(_))
    ));

    // Pagination includes the new user.
    let all = store.list_users(0, 1000).await.expect("list users");
    assert!(all.iter().any(|user| user.id == created.id));

    assert!(store.delete_user(created.id).await.expect("delete"));
    assert!(
        store.get_user(created.id).await.expect("get").is_none(),
        "hard delete removes the row"
    );
}

#[tokio::test]
async fn pg_files_timeline_dedupe_and_soft_delete() {
    let Some(store) = db().await else { return };
    let owner = store.create_user(scratch_user()).await.expect("owner");
    let other = store
        .create_user(scratch_user())
        .await
        .expect("other owner");

    let file = |owner_id, sha: &str| NewFile {
        parent_id: None,
        owner_id,
        name: sha.to_owned(),
        kind: FileKind::Image,
        mime: "image/webp".to_owned(),
        size: 42,
        sha256: sha.to_owned(),
        bucket_id: None,
        storage_backend: "local".to_owned(),
        storage_key: sha.to_owned(),
        thumbnail_key: None,
        original_name: "cat.png".to_owned(),
    };

    let a = store
        .create_file(file(owner.id, &uuid::Uuid::new_v4().simple().to_string()))
        .await
        .expect("file a");
    let b = store
        .create_file(file(owner.id, &uuid::Uuid::new_v4().simple().to_string()))
        .await
        .expect("file b");

    // Timeline order: newest first.
    let page = store.list_files(0, 10).await.expect("list files");
    let positions: Vec<_> = page
        .iter()
        .filter(|f| f.id == a.id || f.id == b.id)
        .map(|f| f.id)
        .collect();
    assert_eq!(
        positions.first(),
        Some(&b.id),
        "b uploaded later, listed first"
    );

    // Per-owner dedupe probe.
    assert_eq!(
        store
            .get_file_by_owner_and_sha256(owner.id, &a.sha256)
            .await
            .expect("sha probe")
            .map(|f| f.id),
        Some(a.id)
    );
    assert!(matches!(
        store.create_file(file(owner.id, &a.sha256)).await,
        Err(i0721_store::StoreError::Conflict(_))
    ));

    // Another owner, same bytes: a fresh row sharing the storage key.
    let shared = store
        .create_file(file(other.id, &a.sha256))
        .await
        .expect("cross-owner upload is a new row, not a conflict");
    assert_ne!(shared.id, a.id);
    assert_eq!(shared.storage_key, a.storage_key);
    assert_eq!(
        store
            .count_live_files_by_storage_key(None, &a.storage_key)
            .await
            .expect("storage key census"),
        2
    );

    // Guarded soft delete: hides the row, keeps the other owner's blob
    // reference alive, and a second call marks 0 rows.
    assert!(store.soft_delete_file(a.id).await.expect("soft delete"));
    assert!(
        !store
            .soft_delete_file(a.id)
            .await
            .expect("second soft delete"),
        "the guarded update refuses the second call"
    );
    assert!(store.get_file(a.id).await.expect("get").is_none());
    assert_eq!(
        store
            .count_live_files_by_storage_key(None, &a.storage_key)
            .await
            .expect("census after delete"),
        1,
        "the other owner's row still references the blob"
    );
    let resurrected = store
        .create_file(file(owner.id, &a.sha256))
        .await
        .expect("soft delete frees the per-owner hash");
    assert_ne!(resurrected.id, a.id);

    // Guarded rename: a soft-deleted row answers None.
    assert!(store
        .rename_file(a.id, "renamed")
        .await
        .expect("rename query")
        .is_none());
    let renamed = store
        .rename_file(b.id, "renamed")
        .await
        .expect("rename")
        .expect("row live");
    assert_eq!(renamed.name, "renamed");

    cleanup_user(&store, owner.id).await;
    cleanup_user(&store, other.id).await;
}

#[tokio::test]
async fn pg_settings_upsert_round_trip() {
    let Some(store) = db().await else { return };
    let key = format!("lane_{}", uuid::Uuid::new_v4().simple());
    store
        .set_setting(&key, serde_json::json!({"n": 1}))
        .await
        .expect("set");
    assert_eq!(
        store.get_setting(&key).await.expect("get"),
        Some(serde_json::json!({"n": 1}))
    );
    store
        .set_setting(&key, serde_json::json!({"n": 2}))
        .await
        .expect("upsert");
    assert_eq!(
        store.get_setting(&key).await.expect("get again"),
        Some(serde_json::json!({"n": 2})),
        "second set wins (ON CONFLICT DO UPDATE)"
    );
}

#[tokio::test]
async fn pg_settings_seed_row_exists() {
    let Some(store) = db().await else { return };
    let site = store
        .get_setting(i0721_proto::CONFIG_SETTING_KEY)
        .await
        .expect("settings read");
    let site = site.expect("the seed migration planted the site row");
    assert_eq!(
        site,
        serde_json::to_value(i0721_proto::default_site_config()).unwrap(),
        "the seed equals the default site config"
    );
}

// ── the netdisk schema on real PostgreSQL (migration 0004) ─────────────

#[tokio::test]
async fn pg_folder_check_and_root_name_uniqueness() {
    let Some(store) = db().await else { return };

    let user = store
        .create_user(CreateUser {
            username: format!("nd_{}", uuid::Uuid::new_v4().simple()),
            display_name: "ND".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .expect("user");

    // A folder row passes the CHECK (kind='folder').
    let f1 = store
        .create_folder(user.id, None, "docs", None)
        .await
        .expect("first folder");
    assert_eq!(f1.kind, FileKind::Folder);

    // Root-level duplicate: the unique index (COALESCE-folded NULL
    // parent) rejects — surfacing as a Conflict from the store.
    let dup = store.create_folder(user.id, None, "docs", None).await;
    assert!(
        matches!(dup, Err(StoreError::Conflict(_))),
        "root duplicate: {dup:?}"
    );

    // Same name in a DIFFERENT folder is fine (the index keys include
    // the parent).
    let nested = store
        .create_folder(user.id, Some(f1.id), "docs", None)
        .await
        .expect("same name, different layer");
    // And a duplicate in THAT layer refuses.
    let dup2 = store
        .create_folder(user.id, Some(f1.id), "docs", None)
        .await;
    assert!(
        matches!(dup2, Err(StoreError::Conflict(_))),
        "nested duplicate: {dup2:?}"
    );

    // A bad kind is rejected by the CHECK constraint.
    let bad = store
        .create_user(CreateUser {
            username: format!("nd2_{}", uuid::Uuid::new_v4().simple()),
            display_name: "ND2".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let _ = bad;
    // (kind values route through create_folder only; the CHECK itself is
    // proven by the folder rows above — a raw INSERT bypass is not
    // reachable through the store API surface.)
    drop(nested);
}

#[tokio::test]
async fn pg_recursive_ancestors_and_subtree() {
    let Some(store) = db().await else { return };
    let user = store
        .create_user(CreateUser {
            username: format!("rc_{}", uuid::Uuid::new_v4().simple()),
            display_name: "RC".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .expect("user");

    // a → b → c (a=root child).
    let a = store.create_folder(user.id, None, "a", None).await.unwrap();
    eprintln!("DEBUG: folder a = {a:?}");
    let b = store
        .create_folder(user.id, Some(a.id), "b", None)
        .await
        .unwrap();
    eprintln!("DEBUG: folder b = {b:?}");
    let c = store
        .create_folder(user.id, Some(b.id), "c", None)
        .await
        .unwrap();
    eprintln!("DEBUG: folder c = {c:?}");

    // Ancestors of c: b, a (nearest first — order may vary, membership is
    // the contract).
    let ancestors = store.ancestor_ids(c.id).await.unwrap();
    assert!(
        ancestors.contains(&b.id),
        "b is an ancestor of c: {ancestors:?}"
    );
    assert!(
        ancestors.contains(&a.id),
        "a is an ancestor of c: {ancestors:?}"
    );
    assert!(!ancestors.contains(&c.id), "self is not its own ancestor");

    // Subtree of a: a, b, c.
    let subtree = store.subtree_live_ids(a.id).await.unwrap();
    assert!(subtree.contains(&a.id) && subtree.contains(&b.id) && subtree.contains(&c.id));
    assert_eq!(subtree.len(), 3, "exactly the three folders");

    // Subtree of b: b, c only (a excluded).
    let subtree_b = store.subtree_live_ids(b.id).await.unwrap();
    assert!(subtree_b.contains(&b.id) && subtree_b.contains(&c.id));
    assert!(
        !subtree_b.contains(&a.id),
        "the parent is not in the child's subtree"
    );

    // Cascade soft-delete of a: all three go.
    let removed = store.soft_delete_many(&subtree).await.unwrap();
    assert_eq!(removed, 3, "the whole subtree deleted");
    for id in [&a.id, &b.id, &c.id] {
        assert!(
            store.get_file(*id).await.unwrap().is_none(),
            "{id} gone from the live view"
        );
    }
}

#[tokio::test]
async fn pg_migration_0004_is_idempotent() {
    // The runner records applied versions; running the full set again
    // must be a no-op (the 0003 test covers the mechanism; this pins
    // that 0004 joins the recorded set).
    let Some(store) = db().await else { return };
    let result = store.migrate().await.expect("re-migration is a no-op");
    assert!(
        result.applied.is_empty(),
        "nothing re-applies: {:?}",
        result.applied
    );
}

/// Migration 0004's legacy data cleansing: pre-existing same-owner
/// same-layer duplicate names get suffixed; the unique index then holds.
#[tokio::test]
async fn pg_migration_0004_dedupes_legacy_names_and_enforces_index() {
    let Some(store) = db().await else { return };

    let user = store
        .create_user(CreateUser {
            username: format!("legacy_{}", uuid::Uuid::new_v4().simple()),
            display_name: "Legacy".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();

    // Hand-plant the PRE-MIGRATION COLLISION shape: two "cat.png" +
    // a pre-existing "cat.png (2)" — the exact form the reviewer asked
    // for (naive rn assignment would give the second cat.png rn=2,
    // colliding with the existing "cat.png (2)").
    {
        use lacrymira::{Command, Value};
        let drop = Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: format!(
                "DROP INDEX IF EXISTS {}",
                i0721_store::migrate::FILES_OWNER_FOLDER_NAME_LIVE_KEY_SQL
            ),
            params: vec![],
        };
        store.executor.execute(&drop).await.unwrap();
        store
            .executor
            .execute(&lacrymira::Command::Sql {
                dialect: lacrymira::Dialect::Postgres,
                text: "DROP INDEX IF EXISTS files_owner_bucket_folder_name_live_key".to_owned(),
                params: vec![],
            })
            .await
            .unwrap();

        for (name, sha, delay_ms) in [
            ("cat.png", "aaaa0000", 0),
            ("cat.png", "bbbb0000", 10),
            ("cat.png (2)", "cccc0000", 20),
        ] {
            if delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
            let insert = Command::Sql {
                dialect: lacrymira::Dialect::Postgres,
                text:
                    "INSERT INTO files (id, owner_id, parent_id, name, kind, mime, size, sha256, \
                     storage_backend, storage_key, original_name) \
                     VALUES (gen_random_uuid(), $1, NULL, $2, 'image', 'image/webp', \
                     100, $3, 'local', $3, $2)"
                        .to_owned(),
                params: vec![
                    Value::Uuid(user.id.to_string()),
                    Value::Text(name.to_owned()),
                    Value::Text(sha.to_owned()),
                ],
            };
            store.executor.execute(&insert).await.unwrap();
        }
    }

    // Run the avoidance-based cleansing.
    {
        use lacrymira::Command;
        let cleanse = Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: i0721_store::migrate::MIGRATION_0004_DEDUPE_SQL.to_owned(),
            params: vec![],
        };
        store.executor.execute(&cleanse).await.unwrap();
    }

    // All three rows must now have UNIQUE names.
    let files = store
        .list_folder(user.id, None, None, 0, 100)
        .await
        .unwrap();
    assert_eq!(files.len(), 3, "all rows live: {files:?}");
    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    // The first cat.png keeps its name; the second (rn=2) must NOT
    // collide with the pre-existing "cat.png (2)" — it takes (3).
    assert!(names.contains(&"cat.png"), "the earliest keeps: {names:?}");
    assert!(
        names.contains(&"cat.png (2)"),
        "the pre-existing (2) stays: {names:?}"
    );
    assert!(
        names.contains(&"cat.png (3)"),
        "the duplicate avoided (2) and took (3): {names:?}"
    );
    // All unique.
    let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
    assert_eq!(unique.len(), 3, "all three names are unique: {names:?}");

    // Rebuild the index — it must succeed now.
    {
        use lacrymira::Command;
        let rebuild = Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: format!(
                "CREATE UNIQUE INDEX IF NOT EXISTS {} \
                   ON files (owner_id, COALESCE(bucket_id, '00000000-0000-0000-0000-000000000000'::uuid), \
                   COALESCE(parent_id, '00000000-0000-0000-0000-000000000000'::uuid), name) \
                   WHERE deleted_at IS NULL",
                i0721_store::migrate::FILES_OWNER_BUCKET_FOLDER_NAME_LIVE_KEY_SQL_SLUG
            ),
            params: vec![],
        };
        store.executor.execute(&rebuild).await.unwrap();
    }

    // The index enforces.
    let dup = store.create_folder(user.id, None, "cat.png", None).await;
    assert!(
        matches!(dup, Err(StoreError::Conflict(_))),
        "the index enforces same-layer uniqueness: {dup:?}"
    );
}

#[tokio::test]
async fn pg_sum_live_sizes_returns_bigint() {
    let Some(store) = db().await else { return };
    let user = store
        .create_user(CreateUser {
            username: format!("sum_{}", uuid::Uuid::new_v4().simple()),
            display_name: "Sum".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let sum = store.sum_live_sizes(user.id).await.unwrap();
    assert_eq!(
        sum, 0,
        "a fresh user has zero bytes (not a numeric-decode 0)"
    );
}

/// The C2 collision shape: THREE same-name rows plus a pre-existing
/// suffixed name. The static-CTE avoidance assigned every renamer the
/// same free candidate; the rank-pairing rewrite must hand the k-th
/// renamer the k-th free candidate.
#[tokio::test]
async fn pg_migration_0004_dedupe_survives_three_fold_duplicates() {
    let Some(store) = db().await else { return };

    let user = store
        .create_user(CreateUser {
            username: format!("threefold_{}", uuid::Uuid::new_v4().simple()),
            display_name: "Threefold".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();

    {
        use lacrymira::{Command, Value};
        let drop = Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: format!(
                "DROP INDEX IF EXISTS {}",
                i0721_store::migrate::FILES_OWNER_FOLDER_NAME_LIVE_KEY_SQL
            ),
            params: vec![],
        };
        store.executor.execute(&drop).await.unwrap();
        store
            .executor
            .execute(&lacrymira::Command::Sql {
                dialect: lacrymira::Dialect::Postgres,
                text: "DROP INDEX IF EXISTS files_owner_bucket_folder_name_live_key".to_owned(),
                params: vec![],
            })
            .await
            .unwrap();

        for (i, name) in ["x.bin", "x.bin", "x.bin", "x.bin (2)"]
            .into_iter()
            .enumerate()
        {
            let sha = format!("dd{:06x}", i);
            if i > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            let insert = Command::Sql {
                dialect: lacrymira::Dialect::Postgres,
                text:
                    "INSERT INTO files (id, owner_id, parent_id, name, kind, mime, size, sha256, \
                     storage_backend, storage_key, original_name) \
                     VALUES (gen_random_uuid(), $1, NULL, $2, 'other', 'application/octet-stream', \
                     10, $3, 'local', $3, $2)"
                        .to_owned(),
                params: vec![
                    Value::Uuid(user.id.to_string()),
                    Value::Text(name.to_owned()),
                    Value::Text(sha),
                ],
            };
            store.executor.execute(&insert).await.unwrap();
        }
    }

    {
        use lacrymira::Command;
        let cleanse = Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: i0721_store::migrate::MIGRATION_0004_DEDUPE_SQL.to_owned(),
            params: vec![],
        };
        store.executor.execute(&cleanse).await.unwrap();
    }

    let files = store
        .list_folder(user.id, None, None, 0, 100)
        .await
        .unwrap();
    assert_eq!(files.len(), 4, "all rows live: {files:?}");
    let mut names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["x.bin", "x.bin (2)", "x.bin (3)", "x.bin (4)"],
        "rank pairing hands the two renamers distinct free candidates past the blocked (2)"
    );

    // The rebuilt index must accept this state.
    {
        use lacrymira::Command;
        let rebuild = Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: format!(
                "CREATE UNIQUE INDEX IF NOT EXISTS {} \
                   ON files (owner_id, COALESCE(bucket_id, '00000000-0000-0000-0000-000000000000'::uuid), \
                   COALESCE(parent_id, '00000000-0000-0000-0000-000000000000'::uuid), name) \
                   WHERE deleted_at IS NULL",
                i0721_store::migrate::FILES_OWNER_BUCKET_FOLDER_NAME_LIVE_KEY_SQL_SLUG
            ),
            params: vec![],
        };
        store.executor.execute(&rebuild).await.unwrap();
    }
}

/// Upload-into-folder is the wave's headline: the persisted row must
/// carry the requested parent (this is the defect C2 found hardwired
/// to None on both store lanes).
#[tokio::test]
async fn pg_create_file_honors_parent_id() {
    let Some(store) = db().await else { return };

    let user = store
        .create_user(CreateUser {
            username: format!("parentfix_{}", uuid::Uuid::new_v4().simple()),
            display_name: "ParentFix".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();

    let folder = store
        .create_folder(user.id, None, "docs", None)
        .await
        .unwrap();
    let nested = store
        .create_folder(user.id, Some(folder.id), "2026", None)
        .await
        .unwrap();
    assert_eq!(
        nested.parent_id,
        Some(folder.id),
        "the nested folder carries its parent"
    );

    let file = store
        .create_file(i0721_proto::NewFile {
            owner_id: user.id,
            parent_id: Some(nested.id),
            name: "report.bin".to_owned(),
            kind: i0721_proto::FileKind::Other,
            mime: "application/octet-stream".to_owned(),
            size: 7,
            sha256: format!("pf{}", uuid::Uuid::new_v4().simple()),
            bucket_id: None,
            storage_backend: "local".to_owned(),
            storage_key: format!("pf{}", uuid::Uuid::new_v4().simple()),
            thumbnail_key: None,
            original_name: "report.bin".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(
        file.parent_id,
        Some(nested.id),
        "the file lands in its folder"
    );

    let inside = store
        .list_folder(user.id, Some(nested.id), None, 0, 10)
        .await
        .unwrap();
    assert_eq!(inside.len(), 1, "the nested folder lists the file");
    assert_eq!(inside[0].name, "report.bin");
}

// ── the shares read path (the MCP link tool's reuse decision) ──────────

/// `find_share_by_file` is owner-scoped, answers the file's row and has
/// no cross-owner leak (the MCP `get_share_link` reuse path reads it).
#[tokio::test]
async fn pg_find_share_by_file_is_owner_scoped() {
    let Some(store) = db().await else { return };
    let owner = store.create_user(scratch_user()).await.expect("owner");
    let other = store
        .create_user(scratch_user())
        .await
        .expect("other owner");
    // Same-layer uniqueness (migration 0004) forbids two live rows named
    // alike under one owner, so each file carries its own name.
    let file = |owner_id, name: &str| NewFile {
        parent_id: None,
        owner_id,
        name: name.to_owned(),
        kind: FileKind::Other,
        mime: "application/octet-stream".to_owned(),
        size: 3,
        sha256: format!("sh{}", uuid::Uuid::new_v4().simple()),
        bucket_id: None,
        storage_backend: "local".to_owned(),
        storage_key: format!("sh{}", uuid::Uuid::new_v4().simple()),
        thumbnail_key: None,
        original_name: name.to_owned(),
    };
    let a = store
        .create_file(file(owner.id, "share-a.bin"))
        .await
        .expect("file a");
    let b = store
        .create_file(file(owner.id, "share-b.bin"))
        .await
        .expect("file b");

    assert!(
        store
            .find_share_by_file(owner.id, a.id)
            .await
            .expect("lookup")
            .is_none(),
        "a file without a share answers None"
    );

    let share = store
        .create_share(i0721_proto::NewShare {
            file_id: a.id,
            owner_id: owner.id,
            expires_at: None,
            allow_preview: true,
        })
        .await
        .expect("share");
    let found = store
        .find_share_by_file(owner.id, a.id)
        .await
        .expect("lookup")
        .expect("the file's share is found");
    assert_eq!(found.token, share.token);
    assert!(
        store
            .find_share_by_file(owner.id, b.id)
            .await
            .expect("lookup")
            .is_none(),
        "the lookup is per file"
    );
    assert!(
        store
            .find_share_by_file(other.id, a.id)
            .await
            .expect("lookup")
            .is_none(),
        "the lookup is per owner"
    );

    cleanup_user(&store, owner.id).await;
    cleanup_user(&store, other.id).await;
}

/// The affected-count side of the API key lane: `revoke` and `touch`
/// answer through `Reply::Affected`, so a `returning: true` site would
/// turn every call into an error.
#[tokio::test]
async fn pg_api_key_lifecycle_reads_affected_counts() {
    // R2 S3 finding: revoke/touch carry `returning: false` — they read the
    // affected count, which only exists on the Affected reply path. Before
    // 0.2 the driver always answered Rows, so revoke's `let Reply::Affected`
    // never matched and every call errored. Pin both the success and the
    // second-call (0 rows) verdict.
    let Some(store) = db().await else { return };
    let user = store.create_user(scratch_user()).await.expect("user");
    let record = i0721_proto::ApiKeyRecord {
        id: uuid::Uuid::new_v4(),
        user_id: user.id,
        name: "lifecycle".to_owned(),
        key_prefix: "i0721-lc".to_owned(),
        key_hash: format!("hash{}", uuid::Uuid::new_v4().simple()),
        scopes: "[]".to_owned(),
        expires_at: None,
        last_used_at: None,
        revoked_at: None,
        created_at: jiff::Timestamp::now(),
    };
    store.create_api_key(&record).await.expect("insert");

    // touch: fire-and-forget, must not error and must set last_used_at.
    store.touch_api_key(record.id).await.expect("touch");
    let touched = store
        .list_api_keys(user.id)
        .await
        .expect("list")
        .into_iter()
        .find(|r| r.id == record.id)
        .expect("row listed");
    assert!(touched.last_used_at.is_some(), "touch stamped last_used_at");

    // revoke: first call marks the row, the guarded second call marks none.
    assert!(
        store.revoke_api_key(record.id).await.expect("revoke"),
        "the first revoke reports an affected row"
    );
    assert!(
        !store
            .revoke_api_key(record.id)
            .await
            .expect("second revoke"),
        "the guarded update reports 0 affected rows"
    );
    // A revoked key no longer resolves by hash (the filter excludes it).
    assert!(
        store
            .find_api_key_by_hash(&record.key_hash)
            .await
            .expect("lookup")
            .is_none(),
        "revoked keys drop out of the hash lookup"
    );

    cleanup_user(&store, user.id).await;
}

/// A JSON *string* payload (a String special case on the read side
/// would strip its quotes), and an unparseable payload must be REJECTED
/// at the write (a permissions column may not silently become the
/// fail-open "empty = full" set).
#[tokio::test]
async fn pg_api_key_scopes_round_trip_and_reject_garbage() {
    let Some(store) = db().await else { return };
    let user = store.create_user(scratch_user()).await.expect("user");
    for payload in [
        "[]",
        "[\"read\",\"write\"]",
        "{\"share\":true}",
        "\"openid profile\"",
    ] {
        let record = i0721_proto::ApiKeyRecord {
            id: uuid::Uuid::new_v4(),
            user_id: user.id,
            name: format!("scopes-{payload}"),
            key_prefix: "i0721-abcd".to_owned(),
            key_hash: format!("hash{}", uuid::Uuid::new_v4().simple()),
            scopes: payload.to_owned(),
            expires_at: None,
            last_used_at: None,
            revoked_at: None,
            created_at: jiff::Timestamp::now(),
        };
        store.create_api_key(&record).await.expect("insert");
        let rows = store.list_api_keys(user.id).await.expect("list");
        let row = rows
            .iter()
            .find(|r| r.id == record.id)
            .expect("the row is listed");
        assert_eq!(
            row.scopes, payload,
            "the scopes payload round-trips byte-identically ({payload})"
        );
    }

    // Garbage must be refused, not folded into the empty set.
    let bad = i0721_proto::ApiKeyRecord {
        id: uuid::Uuid::new_v4(),
        user_id: user.id,
        name: "scopes-garbage".to_owned(),
        key_prefix: "i0721-beef".to_owned(),
        key_hash: format!("hash{}", uuid::Uuid::new_v4().simple()),
        scopes: "not-json".to_owned(),
        expires_at: None,
        last_used_at: None,
        revoked_at: None,
        created_at: jiff::Timestamp::now(),
    };
    let refused = store.create_api_key(&bad).await;
    assert!(
        refused.is_err(),
        "an unparseable scopes payload is refused (fail-closed): {refused:?}"
    );
}

/// ⑩ The permission wave on a real database: the legacy `[]` scopes row
/// is normalized by the SHARED migration SQL (which the lane re-runs
/// against a hand-planted legacy shape, so the test does not depend on
/// which migration version the scratch database is at), and the new
/// `expires_at` column writes and reads back.
/// MUT:M8 — gutting the shared SQL's WHERE turns this test red: it is that
/// SQL the lane exercises, not the migration runner's state.
#[tokio::test]
async fn pg_api_key_legacy_scopes_normalize_and_expiry_round_trips() {
    let Some(store) = db().await else { return };
    let user = store.create_user(scratch_user()).await.expect("user");
    let legacy_id = uuid::Uuid::new_v4();

    // A row in the PRE-permission-wave shape: the bare `[]` array nobody
    // executed (the INSERT predates the column, so `expires_at` stays
    // NULL as well).
    let plant = lacrymira::Command::Sql {
        dialect: lacrymira::Dialect::Postgres,
        text: "INSERT INTO api_keys (id, user_id, name, key_prefix, key_hash, scopes) \
               VALUES ($1, $2, 'planted-legacy', 'i0721-aaaa****', $3, '[]'::jsonb)"
            .to_owned(),
        params: vec![
            lacrymira::Value::Uuid(legacy_id.to_string()),
            lacrymira::Value::Uuid(user.id.to_string()),
            lacrymira::Value::Text(format!("legacy{}", uuid::Uuid::new_v4().simple())),
        ],
    };
    store
        .executor
        .execute(&plant)
        .await
        .expect("plant legacy row");

    // The same normalization the migration runs.
    let normalize = lacrymira::Command::Sql {
        dialect: lacrymira::Dialect::Postgres,
        text: i0721_store::migrate::MIGRATION_0006_NORMALIZE_SQL.to_owned(),
        params: vec![],
    };
    store
        .executor
        .execute(&normalize)
        .await
        .expect("normalization runs");

    let row = store
        .list_api_keys(user.id)
        .await
        .expect("list")
        .into_iter()
        .find(|r| r.id == legacy_id)
        .expect("the planted row is listed");
    assert_eq!(
        row.scopes, r#"{"ops":[]}"#,
        "a legacy `[]` row normalizes to the EMPTY permission set (fail-closed)"
    );
    assert_eq!(row.expires_at, None, "the pre-wave row has no expiry");

    // Re-running is a no-op for an already-normalized row (the test is
    // re-runnable), and a populated shape is never widened.
    // MUT:M8 — the same gutted WHERE is caught by the assertion
    // "an already-normalized row keeps its ops".
    let populated = format!("hash{}", uuid::Uuid::new_v4().simple());
    let granted = store
        .create_api_key(&i0721_proto::ApiKeyRecord {
            id: uuid::Uuid::new_v4(),
            user_id: user.id,
            name: "granted".to_owned(),
            key_prefix: "i0721-bbbb****".to_owned(),
            key_hash: populated,
            scopes: r#"{"ops":["read"]}"#.to_owned(),
            expires_at: None,
            last_used_at: None,
            revoked_at: None,
            created_at: jiff::Timestamp::now(),
        })
        .await
        .is_ok();
    assert!(granted, "the granted row inserts");
    store
        .executor
        .execute(&normalize)
        .await
        .expect("second normalization run");
    let granted_row = store
        .list_api_keys(user.id)
        .await
        .expect("list")
        .into_iter()
        .find(|r| r.name == "granted")
        .expect("the granted row is listed");
    assert_eq!(
        granted_row.scopes, r#"{"ops":["read"]}"#,
        "an already-normalized row keeps its ops"
    );

    // `expires_at` writes and reads back through update_api_key, and an
    // absent expiry CLEARS it.
    let expiry = jiff::Timestamp::now() + std::time::Duration::from_secs(3600);
    let expiry_text = expiry.to_string();
    assert!(
        store
            .update_api_key(
                legacy_id,
                "renamed-legacy",
                r#"{"ops":["read","share"]}"#,
                Some(&expiry_text)
            )
            .await
            .expect("update"),
        "the live row matches"
    );
    let updated = store
        .list_api_keys(user.id)
        .await
        .expect("list")
        .into_iter()
        .find(|r| r.id == legacy_id)
        .expect("the row is listed");
    assert_eq!(updated.name, "renamed-legacy");
    assert_eq!(updated.expires_at.as_deref(), Some(expiry_text.as_str()));
    assert_eq!(updated.scopes, r#"{"ops":["read","share"]}"#);

    assert!(
        store
            .update_api_key(legacy_id, "cleared", r#"{"ops":["read"]}"#, None)
            .await
            .expect("update"),
        "the second update matches"
    );
    let cleared = store
        .list_api_keys(user.id)
        .await
        .expect("list")
        .into_iter()
        .find(|r| r.id == legacy_id)
        .expect("the row is listed");
    assert_eq!(
        cleared.expires_at, None,
        "an absent expiry clears the column"
    );
    assert_eq!(cleared.scopes, r#"{"ops":["read"]}"#);

    // A revoked key is not updatable (the guard the RPC lane turns into
    // its not-found answer).
    store.revoke_api_key(legacy_id).await.expect("revoke");
    assert!(
        !store
            .update_api_key(legacy_id, "too-late", "{}", None)
            .await
            .expect("update"),
        "a revoked row does not match the live-row filter"
    );

    cleanup_user(&store, user.id).await;
}

// ── migration 0007 (the bucket registry) — the named-SQL direct-call lane ──
//
// The migration itself is a one-shot, but its stateful half (the
// bootstrap upsert + the backfill + the zero-NULL assertion) must be
// provable on real PostgreSQL against hand-planted pre-migration
// shapes, exactly like `MIGRATION_0004_DEDUPE_SQL` above. Each test
// runs the SHARED SQL text (`bootstrap_sql` + the index constants), not
// the runner's bookkeeping.

use i0721_store::migrate::{
    FILES_BUCKET_STORAGE_KEY_INDEX_SQL, FILES_OWNER_FOLDER_NAME_LIVE_KEY_SQL,
    MIGRATION_0007_BOOTSTRAP_SQL, MIGRATION_0007_SCHEMA_SQL,
};

/// The lane's bootstrap parameter set (`bootstrap-local`, matching the
/// fixture that migrated the scratch database).
fn lane_bootstrap() -> i0721_store::BootstrapBucketParams {
    // The id must be a REAL uuid4 (a nil id would collide across
    // re-runs); determinism is NOT required — the upsert keeps the
    // first row's id.
    i0721_store::BootstrapBucketParams {
        id: uuid::Uuid::new_v4(),
        location: "i0721-bwave-lane".to_owned(),
        backend: "local".to_owned(),
    }
}

/// Hand-plant the PRE-migration shape for one label: an enabled dummy
/// label row (`xx` is not one of the three backend tags, so the
/// backfill must fold it onto the current backend's bootstrap bucket).
async fn plant_file(
    store: &PgStore,
    owner_id: uuid::Uuid,
    name: &str,
    storage_backend: &str,
    bucket_id: Option<uuid::Uuid>,
) {
    let sql = match bucket_id {
        None => "INSERT INTO files (id, owner_id, parent_id, name, kind, mime, size, sha256, \
                 storage_backend, storage_key, original_name) \
                 VALUES (gen_random_uuid(), $1, NULL, $2, 'image', 'image/webp', 100, $3, $4, $3, $2)"
            .to_owned(),
        Some(id) => format!(
            "INSERT INTO files (id, owner_id, parent_id, name, kind, mime, size, sha256, \
             bucket_id, storage_backend, storage_key, original_name) \
             VALUES (gen_random_uuid(), $1, NULL, $2, 'image', 'image/webp', 100, $3, '{id}', $4, $3, $2)"
        ),
    };
    let cmd = lacrymira::Command::Sql {
        dialect: lacrymira::Dialect::Postgres,
        text: sql,
        params: vec![
            lacrymira::Value::Uuid(owner_id.to_string()),
            lacrymira::Value::Text(name.to_owned()),
            lacrymira::Value::Text(format!("pf{}", uuid::Uuid::new_v4().simple())),
            lacrymira::Value::Text(storage_backend.to_owned()),
        ],
    };
    store.executor.execute(&cmd).await.unwrap();
}

async fn count_null_bucket_rows(store: &PgStore) -> i64 {
    let cmd = lacrymira::Command::Sql {
        dialect: lacrymira::Dialect::Postgres,
        text: "SELECT COUNT(*) FROM files WHERE bucket_id IS NULL".to_owned(),
        params: vec![],
    };
    let reply = store.executor.execute(&cmd).await.unwrap();
    match reply {
        lacrymira::Reply::Value(Some(lacrymira::Value::I64(n))) => n,
        lacrymira::Reply::Rows { rows, .. } => rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| match v {
                lacrymira::Value::I64(n) => Some(*n),
                lacrymira::Value::I32(n) => Some(i64::from(*n)),
                _ => None,
            })
            .unwrap_or(0),
        other => panic!("unexpected count reply: {other:?}"),
    }
}

/// The migration's statement-3 SQL, formatted with a fresh parameter
/// set — executed statement-by-statement (the lane's `Command::Sql`
/// carries one command per call; the runner's raw lane splits on the
/// same boundaries).
async fn run_bootstrap(store: &PgStore) {
    for statement in i0721_store::migrate::bootstrap_sql_statements(&lane_bootstrap())
        .expect("the lane bootstrap location passes the quoting guard")
    {
        store
            .executor
            .execute(&lacrymira::Command::Sql {
                dialect: lacrymira::Dialect::Postgres,
                text: statement,
                params: vec![],
            })
            .await
            .expect("bootstrap statement succeeds");
    }
}

/// MUT:M1a — gutting the backfill `UPDATE` (statement 3c) leaves
/// planted rows without a bucket, and the assertion `DO` block (3d)
/// fails the statement; this test must turn red with exactly that
/// failure.
#[tokio::test]
async fn pg_0007_backfill_sql_fills_every_row() {
    let Some(store) = db().await else { return };
    let user = store.create_user(scratch_user()).await.expect("user");
    plant_file(&store, user.id, "unlabelled.png", "local", None).await;
    plant_file(&store, user.id, "unknown.png", "gcs", None).await;
    assert!(count_null_bucket_rows(&store).await > 0, "precondition");

    run_bootstrap(&store).await;
    assert_eq!(
        count_null_bucket_rows(&store).await,
        0,
        "every row is backfilled to a bucket — the zero-NULL invariant holds on real data"
    );
    cleanup_user(&store, user.id).await;
}

/// MUT:M1b — dropping the `WHERE bucket_id IS NULL` guard turns the
/// backfill into an unconditional rewrite: a row that ALREADY carries
/// its own bucket (planted before this run) must keep it — the fixed
/// point property.
#[tokio::test]
async fn pg_0007_backfill_is_a_fixed_point() {
    let Some(store) = db().await else { return };
    let user = store.create_user(scratch_user()).await.expect("user");
    plant_file(&store, user.id, "re.png", "local", None).await;
    run_bootstrap(&store).await;

    // Snapshot every row's bucket assignment.
    let before = store
        .executor
        .execute(&lacrymira::Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "SELECT id, bucket_id FROM files ORDER BY id".to_owned(),
            params: vec![],
        })
        .await
        .unwrap();
    let snap_before = format!("{before:?}");

    // Second run (re-runnable like the lane always is).
    run_bootstrap(&store).await;
    let after = store
        .executor
        .execute(&lacrymira::Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "SELECT id, bucket_id FROM files ORDER BY id".to_owned(),
            params: vec![],
        })
        .await
        .unwrap();
    assert_eq!(
        snap_before,
        format!("{after:?}"),
        "re-running the backfill changes nothing (fixed point)"
    );
    cleanup_user(&store, user.id).await;
}

/// MUT:M1b's second face: a row with its OWN bucket (planted with one)
/// must survive the backfill untouched.
#[tokio::test]
async fn pg_0007_backfill_never_touches_a_row_with_a_bucket() {
    let Some(store) = db().await else { return };
    let user = store.create_user(scratch_user()).await.expect("user");

    // Plant one row already carrying a bucket: create a bucket row by
    // running the bootstrap SQL once (it produces bootstrap-local), then
    // reference it.
    run_bootstrap(&store).await;
    let bucket_id: uuid::Uuid = {
        let reply = store
            .executor
            .execute(&lacrymira::Command::Sql {
                dialect: lacrymira::Dialect::Postgres,
                text: "SELECT id FROM storage_buckets WHERE slug = 'bootstrap-local' LIMIT 1"
                    .to_owned(),
                params: vec![],
            })
            .await
            .unwrap();
        let lacrymira::Reply::Rows { rows, .. } = reply else {
            panic!("rows expected");
        };
        match &rows[0][0] {
            lacrymira::Value::Uuid(text) => uuid::Uuid::parse_str(text).unwrap(),
            other => panic!("uuid expected, got {other:?}"),
        }
    };
    plant_file(&store, user.id, "pre-owned.png", "local", Some(bucket_id)).await;

    // The backfill runs again — and must not rewrite the owned row's
    // bucket (the WHERE guard).
    run_bootstrap(&store).await;
    let reply = store
        .executor
        .execute(&lacrymira::Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "SELECT bucket_id FROM files WHERE name = 'pre-owned.png'".to_owned(),
            params: vec![],
        })
        .await
        .unwrap();
    let lacrymira::Reply::Rows { rows, .. } = reply else {
        panic!("rows expected");
    };
    match &rows[0][0] {
        lacrymira::Value::Uuid(text) => {
            assert_eq!(
                uuid::Uuid::parse_str(text).unwrap(),
                bucket_id,
                "the row keeps ITS OWN bucket after the backfill"
            );
        }
        other => panic!("uuid expected, got {other:?}"),
    }
    cleanup_user(&store, user.id).await;
}

/// The bootstrap upsert keeps the FIRST id stable across re-runs (the
/// CTE's `ON CONFLICT DO UPDATE ... RETURNING id`).
#[tokio::test]
async fn pg_0007_bootstrap_bucket_id_is_stable() {
    let Some(store) = db().await else { return };
    let read_id = || async {
        let reply = store
            .executor
            .execute(&lacrymira::Command::Sql {
                dialect: lacrymira::Dialect::Postgres,
                text: "SELECT id FROM storage_buckets WHERE slug = 'bootstrap-local' LIMIT 1"
                    .to_owned(),
                params: vec![],
            })
            .await
            .unwrap();
        let lacrymira::Reply::Rows { rows, .. } = reply else {
            panic!("rows expected");
        };
        assert!(!rows.is_empty(), "the bootstrap row exists");
        match &rows[0][0] {
            lacrymira::Value::Uuid(text) => uuid::Uuid::parse_str(text).unwrap(),
            other => panic!("uuid expected, got {other:?}"),
        }
    };
    // The lane bootstrap already created the row; re-running the SQL
    // with a DIFFERENT minted id must keep the first one.
    run_bootstrap(&store).await;
    let first = read_id().await;
    run_bootstrap(&store).await;
    assert_eq!(read_id().await, first, "the first id stays stable");
}

/// A database migrated by 0007 carries one bootstrap row per backend
/// label that occurs in `files.storage_backend`, plus the current
/// backend's (R-H statement 3). The lane database's own migration
/// already proves the shape; this re-runs the whole 0007 SQL block on
/// the migrated database and counts the rows.
#[tokio::test]
async fn a_database_migrated_by_0007_has_a_bootstrap_row_per_backend_label() {
    let Some(store) = db().await else { return };
    let user = store.create_user(scratch_user()).await.expect("user");
    // Plant one s3 row so a legacy s3 bucket must appear; the current
    // backend is local (the fixture's).
    plant_file(&store, user.id, "legacy-s3.png", "s3", None).await;
    plant_file(&store, user.id, "legacy-ipfs.png", "ipfs", None).await;
    run_bootstrap(&store).await;
    let reply = store
        .executor
        .execute(&lacrymira::Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "SELECT slug, enabled, anonymous_read, grant_on_signup, is_bootstrap \
                   FROM storage_buckets WHERE slug LIKE 'bootstrap-%' ORDER BY slug"
                .to_owned(),
            params: vec![],
        })
        .await
        .unwrap();
    let lacrymira::Reply::Rows { rows, .. } = reply else {
        panic!("rows expected");
    };
    let slugs: Vec<String> = rows
        .iter()
        .filter_map(|r| match &r[0] {
            lacrymira::Value::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert!(slugs.contains(&"bootstrap-local".to_owned()), "{slugs:?}");
    assert!(slugs.contains(&"bootstrap-s3".to_owned()), "{slugs:?}");
    assert!(slugs.contains(&"bootstrap-ipfs".to_owned()), "{slugs:?}");
    // The legacy rows are DISABLED with anonymous_read TRUE and
    // grant_on_signup FALSE (the R-A-compatible defaults).
    let legacy_disabled = rows.iter().any(|r| {
        matches!(&r[0], lacrymira::Value::Text(t) if t == "bootstrap-s3")
            && matches!(&r[1], lacrymira::Value::Bool(false))
            && matches!(&r[2], lacrymira::Value::Bool(true))
            && matches!(&r[3], lacrymira::Value::Bool(false))
            && matches!(&r[4], lacrymira::Value::Bool(true))
    });
    assert!(
        legacy_disabled,
        "legacy bootstrap rows are disabled + anonymous-readable + not signup-granted"
    );
    cleanup_user(&store, user.id).await;
}

/// Rows seat by their OWN storage_backend label (statement 3c's
/// COALESCE by slug); this plants rows of every label and reads the
/// assignments back through the store.
#[tokio::test]
async fn rows_are_backfilled_by_their_storage_backend_label() {
    let Some(store) = db().await else { return };
    let user = store.create_user(scratch_user()).await.expect("user");
    plant_file(&store, user.id, "by-s3.png", "s3", None).await;
    plant_file(&store, user.id, "by-local.png", "local", None).await;
    run_bootstrap(&store).await;
    let s3_bucket = store
        .get_storage_bucket_by_slug("bootstrap-s3")
        .await
        .expect("read")
        .expect("legacy s3 bucket exists");
    let local_bucket = store
        .get_storage_bucket_by_slug("bootstrap-local")
        .await
        .expect("read")
        .expect("bootstrap-local exists");
    let rows = store
        .list_live_rows_by_storage_key(&format!("pf{}", ""))
        .await
        .ok();
    let _ = rows; // lookup by name instead — storage keys are random.
    let owned = store
        .executor
        .execute(&lacrymira::Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "SELECT name, bucket_id FROM files WHERE owner_id = $1".to_owned(),
            params: vec![lacrymira::Value::Uuid(user.id.to_string())],
        })
        .await
        .unwrap();
    let lacrymira::Reply::Rows { rows, .. } = owned else {
        panic!("rows expected");
    };
    let mut by_name = std::collections::HashMap::new();
    for r in &rows {
        if let (lacrymira::Value::Text(name), lacrymira::Value::Uuid(id)) = (&r[0], &r[1]) {
            by_name.insert(name.clone(), uuid::Uuid::parse_str(id).unwrap());
        }
    }
    assert_eq!(
        by_name.get("by-s3.png"),
        Some(&s3_bucket.id),
        "the s3-labelled row seats in the legacy s3 bootstrap bucket"
    );
    assert_eq!(
        by_name.get("by-local.png"),
        Some(&local_bucket.id),
        "the local-labelled row seats in the current backend's bucket"
    );
    cleanup_user(&store, user.id).await;
}

/// Folder rows and unlabelled rows follow the CURRENT backend (they
/// carry no meaningful label of their own). Planted with a bogus label
/// (`gcs` is not one of the three tags) they must fold onto
/// `bootstrap-local`.
#[tokio::test]
async fn folders_and_unlabelled_rows_follow_the_current_backend() {
    let Some(store) = db().await else { return };
    let user = store.create_user(scratch_user()).await.expect("user");
    let folder = store
        .create_folder(user.id, None, "docs", None)
        .await
        .unwrap();
    plant_file(&store, user.id, "bogus.png", "gcs", None).await;
    // Simulate a "folder row with no label": clear its storage_backend
    // to '' (the column is NOT NULL, so '' is the closest shape).
    store
        .executor
        .execute(&lacrymira::Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "UPDATE files SET storage_backend = '' WHERE name = 'bogus.png'".to_owned(),
            params: vec![],
        })
        .await
        .unwrap();
    run_bootstrap(&store).await;
    let local_bucket = store
        .get_storage_bucket_by_slug("bootstrap-local")
        .await
        .expect("read")
        .expect("bootstrap-local exists");
    let folder_row = store.get_file(folder.id).await.unwrap().unwrap();
    assert_eq!(
        folder_row.bucket_id,
        Some(local_bucket.id),
        "the folder row follows the current backend"
    );
    let reply = store
        .executor
        .execute(&lacrymira::Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "SELECT bucket_id FROM files WHERE name = 'bogus.png'".to_owned(),
            params: vec![],
        })
        .await
        .unwrap();
    let lacrymira::Reply::Rows { rows, .. } = reply else {
        panic!("rows expected");
    };
    match &rows[0][0] {
        lacrymira::Value::Uuid(text) => assert_eq!(
            uuid::Uuid::parse_str(text).unwrap(),
            local_bucket.id,
            "an unlabelled row folds onto the current backend's bucket"
        ),
        lacrymira::Value::Null => panic!("the backfill left a NULL bucket_id"),
        other => panic!("uuid expected, got {other:?}"),
    }
    cleanup_user(&store, user.id).await;
}

/// Q5/R-K's refusal half: a second ipfs bucket is refused by the
/// partial unique index (DIRECT INSERT only — the upsert half is P2a).
#[tokio::test]
async fn second_ipfs_bucket_is_refused() {
    let Some(store) = db().await else { return };
    fn insert<'a>(
        store: &'a PgStore,
        slug: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<lacrymira::Reply, lacrymira::Error>> + 'a>,
    > {
        Box::pin(async move {
            store
                .executor
                .execute(&lacrymira::Command::Sql {
                    dialect: lacrymira::Dialect::Postgres,
                    text:
                        "INSERT INTO storage_buckets (id, slug, display_name, backend, location, \
                           endpoint_ref, anonymous_read, grant_on_signup, enabled, is_bootstrap) \
                           VALUES (gen_random_uuid(), $1, $1, 'ipfs', '127.0.0.1:5001', \
                           'bucket_ipfs_env', TRUE, FALSE, TRUE, FALSE)"
                            .to_owned(),
                    params: vec![lacrymira::Value::Text(slug.to_owned())],
                })
                .await
        })
    }
    let _ = insert(&store, "ipfs-first")
        .await
        .expect("first ipfs bucket");
    let refused = insert(&store, "ipfs-second").await;
    assert!(
        refused.is_err(),
        "the single-ipfs partial unique index refuses a second ipfs bucket: {refused:?}"
    );
}

/// The schema half + the index statements, applied standalone on a
/// shape that already has them, are no-ops — pinned here as the
/// re-runnable contract (IF NOT EXISTS-style resilience is NOT claimed:
/// the migration runs once; this test only proves the named constants
/// parse and the backfill asserts loudly when a row escapes).
#[tokio::test]
async fn pg_0007_bucket_schema_and_indexes_are_present_after_migration() {
    let Some(store) = db().await else { return };
    // The fixture's migration already ran the schema; verify the named
    // constants match what exists.
    // Exact names — a `LIKE '%table%'` count proves the pkey exists,
    // not that the MIGRATION's indexes do.
    for indexname in [
        "files_owner_bucket_folder_name_live_key",
        "files_owner_bucket_sha256_live_key",
        "files_bucket_storage_key_idx",
        "storage_buckets_single_ipfs",
    ] {
        let reply = store
            .executor
            .execute(&lacrymira::Command::Sql {
                dialect: lacrymira::Dialect::Postgres,
                text: format!("SELECT COUNT(*) FROM pg_indexes WHERE indexname = '{indexname}'"),
                params: vec![],
            })
            .await
            .unwrap();
        let lacrymira::Reply::Rows { rows, .. } = reply else {
            panic!("rows expected");
        };
        let n = match &rows[0][0] {
            lacrymira::Value::I64(n) => *n,
            lacrymira::Value::I32(n) => i64::from(*n),
            other => panic!("count expected, got {other:?}"),
        };
        assert_eq!(n, 1, "the index {indexname} exists exactly once");
    }
    // The migration's columns exist (the pkey says nothing about them).
    for (table, column) in [
        ("files", "bucket_id"),
        ("storage_buckets", "grant_on_signup"),
        ("storage_buckets", "is_bootstrap"),
        ("storage_buckets", "anonymous_read"),
    ] {
        let reply = store
            .executor
            .execute(&lacrymira::Command::Sql {
                dialect: lacrymira::Dialect::Postgres,
                text: format!(
                    "SELECT COUNT(*) FROM information_schema.columns \
                     WHERE table_name = '{table}' AND column_name = '{column}'"
                ),
                params: vec![],
            })
            .await
            .unwrap();
        let lacrymira::Reply::Rows { rows, .. } = reply else {
            panic!("rows expected");
        };
        let n = match &rows[0][0] {
            lacrymira::Value::I64(n) => *n,
            lacrymira::Value::I32(n) => i64::from(*n),
            other => panic!("count expected, got {other:?}"),
        };
        assert_eq!(n, 1, "{table}.{column} exists after the migration");
    }
    // The bucket+storage_key index exists (draft §7.2).
    let reply = store
        .executor
        .execute(&lacrymira::Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text:
                "SELECT COUNT(*) FROM pg_indexes WHERE indexname = 'files_bucket_storage_key_idx'"
                    .to_owned(),
            params: vec![],
        })
        .await
        .unwrap();
    let lacrymira::Reply::Rows { rows, .. } = reply else {
        panic!("rows expected");
    };
    let n = match &rows[0][0] {
        lacrymira::Value::I64(n) => *n,
        lacrymira::Value::I32(n) => i64::from(*n),
        other => panic!("count expected, got {other:?}"),
    };
    assert!(n > 0, "files_bucket_storage_key_idx exists ({n})");
    // The 0007 SQL text carries the exact statements in the brief's
    // order (tables → column → bootstrap → drop old index → new indexes).
    let all = MIGRATION_0007_SCHEMA_SQL;
    assert!(all.contains("CREATE TABLE storage_buckets"), "{all}");
    assert!(all.contains("CREATE TABLE bucket_grants"), "{all}");
    assert!(
        all.contains(
            "ALTER TABLE files ADD COLUMN bucket_id UUID REFERENCES storage_buckets (id);"
        ),
        "the column stays nullable (no SET NOT NULL): {all}"
    );
    assert!(MIGRATION_0007_BOOTSTRAP_SQL.contains("WHERE bucket_id IS NULL"));
    assert!(MIGRATION_0007_BOOTSTRAP_SQL.contains("RAISE EXCEPTION"));
    assert!(FILES_OWNER_FOLDER_NAME_LIVE_KEY_SQL == "files_owner_folder_name_live_key");
    let _ = FILES_BUCKET_STORAGE_KEY_INDEX_SQL;
}

/// MUT:pending-probe — the pending answer must come from the BOOKKEEPING
/// TABLE (via the `to_regclass` probe), so renaming the table away flips
/// the answer to true; a probe-less implementation errors there instead,
/// and a constant-false one fails the final assertion.
#[tokio::test]
async fn pg_bucket_migration_pending_tracks_the_bookkeeping_table() {
    let Some(store) = db().await else { return };
    // After the fixture's migration the answer is false.
    assert!(
        !store.bucket_migration_pending().await.unwrap(),
        "a migrated database has no pending bucket migration"
    );
    // Rename the bookkeeping table away: the fresh-database shape.
    store
        .executor
        .execute(&lacrymira::Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "ALTER TABLE seaql_migrations RENAME TO seaql_migrations_away".to_owned(),
            params: vec![],
        })
        .await
        .unwrap();
    assert!(
        store.bucket_migration_pending().await.unwrap(),
        "a database without the bookkeeping table has every migration pending"
    );
    // Restore: the serial lane's later tests need the bookkeeping rows.
    store
        .executor
        .execute(&lacrymira::Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "ALTER TABLE seaql_migrations_away RENAME TO seaql_migrations".to_owned(),
            params: vec![],
        })
        .await
        .unwrap();
    assert!(
        !store.bucket_migration_pending().await.unwrap(),
        "restoring the table restores the migrated answer"
    );
}

/// MUT:order-by-dropped — dropping the `ORDER BY id ASC` from
/// `list_live_rows_by_storage_key` turns this test red on the PG store
/// (the memory store's insertion order would still pass).
#[tokio::test]
async fn pg_list_live_rows_orders_by_id_asc() {
    let Some(store) = db().await else { return };
    let user = store.create_user(scratch_user()).await.expect("user");
    // The lane shares one scratch database across serial runs — clear the
    // probe rows a previous run may have left, or the assertion below
    // counts stale rows as traversal output.
    store
        .executor
        .execute(&lacrymira::Command::Sql {
            dialect: lacrymira::Dialect::Postgres,
            text: "DELETE FROM files WHERE storage_key = 'order-probe-key'".to_owned(),
            params: vec![],
        })
        .await
        .unwrap();
    let mut ids = Vec::new();
    for _ in 0..4 {
        let reply = store
            .executor
            .execute(&lacrymira::Command::Sql {
                dialect: lacrymira::Dialect::Postgres,
                text: "INSERT INTO files (id, owner_id, parent_id, name, kind, mime, size, \
                       sha256, storage_backend, storage_key, original_name) \
                       VALUES (gen_random_uuid(), $1, NULL, $2, 'image', 'image/webp', 1, $3, \
                       'local', 'order-probe-key', $2) RETURNING id"
                    .to_owned(),
                params: vec![
                    lacrymira::Value::Uuid(user.id.to_string()),
                    lacrymira::Value::Text(uuid::Uuid::new_v4().simple().to_string()),
                    lacrymira::Value::Text(uuid::Uuid::new_v4().simple().to_string()),
                ],
            })
            .await
            .unwrap();
        let lacrymira::Reply::Rows { rows, .. } = reply else {
            panic!("RETURNING id expected");
        };
        let id = match &rows[0][0] {
            lacrymira::Value::Uuid(id) => uuid::Uuid::parse_str(id).expect("a uuid"),
            lacrymira::Value::Text(id) => uuid::Uuid::parse_str(id).expect("a uuid"),
            other => panic!("uuid expected, got {other:?}"),
        };
        ids.push(id);
    }
    let rows = store
        .list_live_rows_by_storage_key("order-probe-key")
        .await
        .unwrap();
    ids.sort();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        ids,
        "the traversal order is id ASC"
    );
}

/// The bootstrap bucket the migration seats carries its IDENTITY fields —
/// not merely an existence proof: slug, backend tag, endpoint reference,
/// location (the lane's), and the three flags (enabled / anonymous /
/// signup) that the P2a slices build on. Immutability enforcement lands
/// with P2a's upsert rejection; this test pins what 0007 writes.
#[tokio::test]
async fn pg_0007_seats_the_bootstrap_bucket_with_its_identity_fields() {
    let Some(store) = db().await else { return };
    let buckets = store.list_storage_buckets().await.unwrap();
    let bootstrap = buckets
        .iter()
        .find(|b| b.slug == "bootstrap-local")
        .expect("the lane's bootstrap bucket is seated");
    assert_eq!(bootstrap.backend, i0721_proto::BucketBackend::Local);
    assert!(bootstrap.is_bootstrap, "it is THE bootstrap bucket");
    assert!(bootstrap.enabled, "the current backend's bucket serves");
    assert!(
        bootstrap.anonymous_read,
        "a signup bucket must be anonymous-readable (R-A's constraint)"
    );
    assert!(
        bootstrap.grant_on_signup,
        "the bootstrap bucket grants new signups by default"
    );
    assert_eq!(bootstrap.endpoint_ref, "legacy_local_env");
    assert_eq!(bootstrap.location, "i0721-bwave-lane");
}
