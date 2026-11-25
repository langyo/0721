/**
 * The bucket registry + grant surface (P2a): the wire shapes are the
 * stage-A RPCs consumed EXACTLY as `packages/server/src/rpc.rs` declares
 * them. `storage.buckets.list/upsert/delete` and `storage.defaults.*`
 * are admin-only; `storage.grants.set` refuses admin targets itself.
 */
import type { BucketBackend } from "@/types";
import { wsCall } from "@/composables/useJsonRpc";

/**
 * One registered bucket — the JSON `storage.buckets.list` answers
 * (`StorageBucketRecord` serialized; there is no ts-rs binding for it,
 * so this is the documented wire shape, not a generated import).
 */
export interface StorageBucket {
  id: string;
  slug: string;
  display_name: string;
  backend: BucketBackend;
  location: string;
  endpoint_ref: string;
  anonymous_read: boolean;
  grant_on_signup: boolean;
  enabled: boolean;
  is_bootstrap: boolean;
  created_at: string;
}

export interface BucketListResponse {
  buckets: StorageBucket[];
}

export function listBuckets(): Promise<BucketListResponse> {
  return wsCall<BucketListResponse>("storage.buckets.list");
}

/** The strict upsert document — EVERY field present, no extras (the
 *  server's `deny_unknown_fields` refuses both shapes). */
export interface BucketUpsert {
  slug: string;
  display_name: string;
  anonymous_read: boolean;
  grant_on_signup: boolean;
  enabled: boolean;
}

export function upsertBucket(doc: BucketUpsert): Promise<BucketListResponse> {
  return wsCall<BucketListResponse>("storage.buckets.upsert", {
    ...doc,
  });
}

export function deleteBucket(slug: string): Promise<void> {
  return wsCall<void>("storage.buckets.delete", { slug });
}

export function getDefaultBucket(): Promise<{ bucket: string | null }> {
  return wsCall<{ bucket: string | null }>("storage.defaults.get");
}

export function setDefaultBucket(bucket: string): Promise<void> {
  return wsCall<void>("storage.defaults.set", { bucket });
}

/** The grant write: exactly the three toggles (R-A). Revocation = an
 *  all-false row; there is NO delete-row / reset-to-default affordance. */
export interface GrantWrite {
  read: boolean;
  write: boolean;
  share: boolean;
}

export function setGrant(user: string, bucketSlug: string, grant: GrantWrite): Promise<void> {
  return wsCall<void>("storage.grants.set", {
    user,
    bucket_slug: bucketSlug,
    read: grant.read,
    write: grant.write,
    share: grant.share,
  });
}
