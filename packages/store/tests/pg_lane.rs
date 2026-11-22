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
use i0721_store::{CreateUser, PgStore, Store};

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
            .count_live_files_by_storage_key(&a.storage_key)
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
            .count_live_files_by_storage_key(&a.storage_key)
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
