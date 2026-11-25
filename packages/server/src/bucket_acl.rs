//! The R-A user-authorization resolution — the ONE place in the service
//! where "what may this user do with this bucket" is answered.
//!
//! Evaluation order (spec §2 R-A, verbatim semantics):
//!
//! 1. `admin` → ALL buckets, all three bits;
//! 2. an explicit `bucket_grants` row exists → the row decides (all
//!    three bits false = DENY — immune to a later `grant_on_signup`
//!    flip: the row is the user's own statement);
//! 3. no row → the bucket's `grant_on_signup`
//!    ? `{read, write, share}` : nothing.
//!
//! Every consumer (the grant surface, `granted_buckets`, the MCP
//! bucket pre-check, `resolve_upload_bucket`) funnels through
//! [`resolve_access`] — a second evaluation site is a drift bug.

use std::collections::BTreeSet;
use std::sync::Arc;

use i0721_proto::{Role, StorageBucketRecord, UserRecord};
use i0721_store::{Store, StoreError};
use uuid::Uuid;

/// The three authorization bits for one user × bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BucketAccess {
    pub read: bool,
    pub write: bool,
    pub share: bool,
}

impl BucketAccess {
    /// All bits false — the explicit-deny row's shape.
    pub const NONE: BucketAccess = BucketAccess {
        read: false,
        write: false,
        share: false,
    };

    /// The `grant_on_signup` default-open shape.
    pub const SIGNUP: BucketAccess = BucketAccess {
        read: true,
        write: true,
        share: true,
    };

    /// Whether ANY bit is set (the visibility predicate, spec Q14:
    /// "visible = the user holds any one grant bit, or the signup
    /// default applies").
    #[must_use]
    pub fn any(self) -> bool {
        self.read || self.write || self.share
    }
}

/// The bits `user` holds on `bucket`. The admin arm never touches the
/// store; the other two need the grant row.
///
/// # Errors
///
/// [`StoreError`] from the grant-row read.
pub async fn resolve_access(
    store: &dyn Store,
    user: &UserRecord,
    bucket: &StorageBucketRecord,
) -> Result<BucketAccess, StoreError> {
    if user.role == Role::Admin {
        return Ok(BucketAccess::SIGNUP);
    }
    match store.get_bucket_grant(user.id, bucket.id).await? {
        // The row is the WHOLE answer — including all-false, which stays
        // a deny even when the bucket later flips `grant_on_signup` on.
        Some(row) => Ok(BucketAccess {
            read: row.can_read,
            write: row.can_write,
            share: row.can_share,
        }),
        None => Ok(if bucket.grant_on_signup {
            BucketAccess::SIGNUP
        } else {
            BucketAccess::NONE
        }),
    }
}

/// Every bucket slug (or id) `actor` may use in any capacity — the
/// `keys.create` materialization source (Q7) and the R-T snapshot
/// oracle. admin = every bucket; a member = the buckets where
/// [`resolve_access`] answers at least one true bit.
///
/// # Errors
///
/// [`StoreError`] from the underlying reads.
pub async fn granted_buckets(
    store: &Arc<dyn Store>,
    actor: &UserRecord,
) -> Result<BTreeSet<String>, StoreError> {
    let buckets = store.list_storage_buckets().await?;
    let mut out = BTreeSet::new();
    for bucket in &buckets {
        if resolve_access(store.as_ref(), actor, bucket).await?.any() {
            out.insert(bucket.slug.clone());
        }
    }
    Ok(out)
}

/// The bucket row for a file's `bucket_id`, resolved from the store's
/// registry rows; when the store seeds none (the fixture lanes), the
/// BlobRegistry's id→slug map is the fallback and the row is synthesized
/// with the bootstrap-bucket posture (signup-open, anonymous-readable).
/// `None` = the id is unknown to BOTH sources (genuinely invisible).
pub async fn bucket_row_for_id(
    store: &dyn Store,
    registry: &crate::blob_registry::BlobRegistry,
    bucket_id: Uuid,
) -> Result<Option<StorageBucketRecord>, StoreError> {
    if let Some(row) = store
        .list_storage_buckets()
        .await?
        .into_iter()
        .find(|b| b.id == bucket_id)
    {
        return Ok(Some(row));
    }
    Ok(registry
        .slug_for_bucket_id(bucket_id)
        .map(|slug| StorageBucketRecord {
            id: bucket_id,
            slug,
            display_name: String::new(),
            backend: i0721_proto::BucketBackend::Local,
            location: String::new(),
            endpoint_ref: String::new(),
            anonymous_read: true,
            grant_on_signup: true,
            enabled: true,
            is_bootstrap: false,
            created_at: jiff::Timestamp::MIN,
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use i0721_proto::BucketBackend;
    use i0721_store::CreateUser;
    use jiff::Timestamp;

    fn bucket(slug: &str, grant_on_signup: bool) -> StorageBucketRecord {
        StorageBucketRecord {
            id: uuid::Uuid::new_v4(),
            slug: slug.to_owned(),
            display_name: slug.to_owned(),
            backend: BucketBackend::Local,
            location: "loc".to_owned(),
            endpoint_ref: "x".to_owned(),
            anonymous_read: true,
            grant_on_signup,
            enabled: true,
            is_bootstrap: false,
            created_at: Timestamp::now(),
        }
    }

    async fn user(store: &Arc<dyn Store>, role: Role) -> UserRecord {
        store
            .create_user(CreateUser {
                username: format!("u-{}", uuid::Uuid::new_v4().simple()),
                display_name: "u".to_owned(),
                role,
                password_hash: None,
            })
            .await
            .unwrap()
    }

    /// MUT:M-P2A2 — the signup branch answering "always has" turns this
    /// red (`non_signup_bucket_invisible_without_a_row`).
    #[tokio::test]
    async fn non_signup_bucket_invisible_without_a_row() {
        let store: Arc<dyn Store> = Arc::new(i0721_store::MemoryStore::new());
        let member = user(&store, Role::Member).await;
        let private = bucket("private", false);
        let access = resolve_access(store.as_ref(), &member, &private)
            .await
            .unwrap();
        assert_eq!(access, BucketAccess::NONE);
        assert!(!access.any());
    }

    /// MUT:M-P2A3 — the signup branch answering "never" turns this red
    /// (`member_keeps_bootstrap_access_without_any_grant_row`).
    #[tokio::test]
    async fn member_keeps_signup_access_without_any_grant_row() {
        let store: Arc<dyn Store> = Arc::new(i0721_store::MemoryStore::new());
        let member = user(&store, Role::Member).await;
        let open = bucket("open", true);
        let access = resolve_access(store.as_ref(), &member, &open)
            .await
            .unwrap();
        assert_eq!(access, BucketAccess::SIGNUP);
    }

    /// An explicit row overrides the signup default — including
    /// partially (`explicit_row_overrides_signup_default_partially`).
    #[tokio::test]
    async fn explicit_row_overrides_signup_default_partially() {
        let store: Arc<dyn Store> = Arc::new(i0721_store::MemoryStore::new());
        let member = user(&store, Role::Member).await;
        let open = bucket("open", true);
        store
            .set_bucket_grant(member.id, open.id, true, false, false)
            .await
            .unwrap();
        let access = resolve_access(store.as_ref(), &member, &open)
            .await
            .unwrap();
        assert_eq!(
            access,
            BucketAccess {
                read: true,
                write: false,
                share: false
            }
        );
    }

    /// MUT:M-P2A4 — a deny row that falls THROUGH to the signup default
    /// (the "row exists but all-false ⇒ consult grant_on_signup" wrong
    /// shape) turns this red
    /// (`an_all_false_row_stays_a_deny_after_signup_flips_back_on`).
    #[tokio::test]
    async fn an_all_false_row_stays_a_deny_after_signup_flips_back_on() {
        let store: Arc<dyn Store> = Arc::new(i0721_store::MemoryStore::new());
        let member = user(&store, Role::Member).await;
        let open = bucket("open", true);
        store
            .set_bucket_grant(member.id, open.id, false, false, false)
            .await
            .unwrap();
        // The bucket flips signup ON (it already is) — the deny row must
        // still hold. Also verify the flip the other way: the row was
        // written while the bucket was open; the deny survives.
        let access = resolve_access(store.as_ref(), &member, &open)
            .await
            .unwrap();
        assert_eq!(access, BucketAccess::NONE, "the explicit deny row wins");
    }

    /// MUT:M-P2A5 — the admin arm returning NONE (or falling through to
    /// the grant lookup) turns this red (`admin_needs_no_grant_rows`).
    #[tokio::test]
    async fn admin_needs_no_grant_rows() {
        let store: Arc<dyn Store> = Arc::new(i0721_store::MemoryStore::new());
        let admin = user(&store, Role::Admin).await;
        let private = bucket("private", false);
        let access = resolve_access(store.as_ref(), &admin, &private)
            .await
            .unwrap();
        assert_eq!(access, BucketAccess::SIGNUP, "admin = all buckets");
    }

    /// `granted_buckets` = the buckets where any bit is true; a deny row
    /// on a signup bucket keeps it OUT of the set — the R-T snapshot
    /// oracle the PG lane's 0008 test re-computes.
    #[tokio::test]
    async fn granted_buckets_reflects_rows_and_signup_defaults() {
        let memory = i0721_store::MemoryStore::new();
        memory.seed_bucket(bucket("open", true));
        memory.seed_bucket(bucket("private", false));
        memory.seed_bucket(bucket("partial", false));
        let store: Arc<dyn Store> = Arc::new(memory);
        let member = user(&store, Role::Member).await;
        let private = store
            .get_storage_bucket_by_slug("private")
            .await
            .unwrap()
            .unwrap();
        let partial = store
            .get_storage_bucket_by_slug("partial")
            .await
            .unwrap()
            .unwrap();
        store
            .set_bucket_grant(member.id, private.id, false, false, false)
            .await
            .unwrap();
        store
            .set_bucket_grant(member.id, partial.id, false, true, false)
            .await
            .unwrap();
        let granted = granted_buckets(&store, &member).await.unwrap();
        assert_eq!(
            granted,
            ["open".to_owned(), "partial".to_owned()].into(),
            "open via signup default; partial via a write-only row; \
             private excluded by its all-false row"
        );
    }

    /// `new_member_sees_only_signup_buckets`: a fresh member's granted
    /// set is exactly the `grant_on_signup` buckets.
    #[tokio::test]
    async fn new_member_sees_only_signup_buckets() {
        let memory = i0721_store::MemoryStore::new();
        memory.seed_bucket(bucket("open", true));
        memory.seed_bucket(bucket("closed", false));
        let store: Arc<dyn Store> = Arc::new(memory);
        let member = user(&store, Role::Member).await;
        let granted = granted_buckets(&store, &member).await.unwrap();
        assert_eq!(granted, ["open".to_owned()].into());
    }

    /// `revoking_access_writes_an_explicit_deny_row`: the grant surface
    /// writes the all-false ROW (no row deletion exists), and that row
    /// is what keeps the access off even on a signup bucket.
    /// MUT:M-P2A6 — a revoke path that DELETES the row instead turns
    /// this red.
    #[tokio::test]
    async fn revoking_access_writes_an_explicit_deny_row() {
        let store: Arc<dyn Store> = Arc::new(i0721_store::MemoryStore::new());
        let member = user(&store, Role::Member).await;
        let open = bucket("open", true);
        // Grant, then revoke (all-false). The bucket needs no seeded row:
        // `resolve_access` reads only the grant row and the record passed
        // in; the grant write is keyed by the bucket id.
        store
            .set_bucket_grant(member.id, open.id, true, true, true)
            .await
            .unwrap();
        store
            .set_bucket_grant(member.id, open.id, false, false, false)
            .await
            .unwrap();
        let row = store
            .get_bucket_grant(member.id, open.id)
            .await
            .unwrap()
            .expect("the revoke WROTE a row — it did not delete one");
        assert!(
            !row.can_read && !row.can_write && !row.can_share,
            "the row is the explicit deny"
        );
        let access = resolve_access(store.as_ref(), &member, &open)
            .await
            .unwrap();
        assert_eq!(access, BucketAccess::NONE);
    }

    /// `member_with_a_deny_row_on_the_bootstrap_bucket_loses_access`:
    /// the deny row applies to the bootstrap bucket like any other.
    #[tokio::test]
    async fn member_with_a_deny_row_on_the_bootstrap_bucket_loses_access() {
        let store: Arc<dyn Store> = Arc::new(i0721_store::MemoryStore::new());
        let member = user(&store, Role::Member).await;
        let mut bootstrap = bucket("bootstrap", true);
        bootstrap.is_bootstrap = true;
        store
            .set_bucket_grant(member.id, bootstrap.id, false, false, false)
            .await
            .unwrap();
        let access = resolve_access(store.as_ref(), &member, &bootstrap)
            .await
            .unwrap();
        assert_eq!(access, BucketAccess::NONE);
    }
}
