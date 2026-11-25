import type { KeyOp } from "@/types";

/**
 * The API-key form's two conversion lanes, shared by the create and edit
 * dialogs (they are the SAME form — only their title, their submit and
 * whether a plaintext key can appear afterwards differ).
 *
 * Why a module: the canonical op order must hold in the PAYLOAD as well
 * as in the list response, and the `datetime-local` ↔ RFC 3339 round
 * trip is a two-way street with a timezone trap on each end. Copying
 * either into a second dialog is how the two dialogs drift apart.
 */

/**
 * The permission vocabulary in its CANONICAL order (read, upload,
 * share) — the order the generated `KeyOp` type and the server's
 * `KeyOp::ALL` declare, and the order the list column renders.
 *
 * The literal array is necessary rather than derived: a union type has
 * no runtime members, and this wave must not invent a parallel enum.
 */
export const KEY_OPS: readonly KeyOp[] = ["read", "upload", "share"];

/** Order a permission set canonically, dropping duplicates. */
export function canonicalOps(ops: readonly KeyOp[]): KeyOp[] {
  return KEY_OPS.filter((op) => ops.includes(op));
}

/**
 * The `scopes.buckets` canonical form (Q7): slug array, ≤ 64 items,
 * DEDUPED, lexicographically sorted — the same canonicalization the
 * server's `encode_scopes_with_buckets` applies. MUT:p2a-key-buckets-canonical —
 * dropping the sort or the dedupe reddens the payload test.
 */
export function canonicalBuckets(buckets: readonly string[]): string[] {
  return [...new Set(buckets)].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0)).slice(0, 64);
}

/**
 * The key's bucket reference as the LIST row shows it. `buckets: null`
 * means the key's scopes carry NO `buckets` array at all (a K-era row
 * that R-T has not materialized — it renders the「无桶」marker); `[]` is
 * an EXPLICIT empty array (the caller picked "no buckets") and renders
 * as the plain empty state, not the marker.
 */
export function keyBucketsOf(
  buckets: readonly string[] | undefined | null,
): string[] | null {
  return Array.isArray(buckets) ? buckets : null;
}

/**
 * A `Date`'s local wall-clock reading as a `datetime-local` field value
 * (`YYYY-MM-DDTHH:mm`).
 *
 * The FIELD is timezone-naive — its value is the user's local time —
 * while every instant the server sends or stores is RFC 3339 (UTC). Both
 * directions therefore go through `Date` (which already carries the
 * zone), never through string slicing: slicing an ISO string prints UTC
 * into a local field and shifts the instant by the browser's offset.
 */
export function toDateTimeLocal(instant: Date): string {
  const pad = (n: number): string => String(n).padStart(2, "0");
  return (
    `${instant.getFullYear()}-${pad(instant.getMonth() + 1)}-${pad(instant.getDate())}` +
    `T${pad(instant.getHours())}:${pad(instant.getMinutes())}`
  );
}

/**
 * An RFC 3339 timestamp (the `ApiKeyPublic.expires_at` wire form) as the
 * edit dialog's field value. An unparseable or absent instant yields the
 * EMPTY field — "no expiry" — never an exception: the list row is not
 * this dialog's input to validate.
 */
export function instantToField(expiresAt: string | undefined | null): string {
  if (!expiresAt) return "";
  const instant = new Date(expiresAt);
  if (Number.isNaN(instant.getTime())) return "";
  return toDateTimeLocal(instant);
}

/**
 * A `datetime-local` field value as the RFC 3339 instant the server
 * parses. The EMPTY value is `undefined` — the field's "no expiry" — and
 * so is an unparseable one (the server rejects that with `-32602`, whose
 * message the toast then carries).
 *
 * The browser hands back either `YYYY-MM-DDTHH:mm` (modern) or the same
 * with seconds; both are local wall-clock, so the parse must stay
 * zone-aware. `new Date("2026-01-01T10:00")` is spec'd as LOCAL time;
 * the seconds form is not uniformly treated that way across engines, so
 * the parts are read explicitly and handed to the `Date` constructor.
 */
export function fieldToInstant(value: string): string | undefined {
  const trimmed = value.trim();
  if (!trimmed) return undefined;
  const parts = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2}))?$/.exec(trimmed);
  if (!parts) return undefined;
  const [, y, mo, d, h, mi, s] = parts;
  const [year, month, day, hour, minute, second] = [y, mo, d, h, mi, s ?? "0"].map(Number);
  // The `Date` constructor ROLLS OVER out-of-range parts instead of
  // rejecting them (`new Date(2027, 2, 4, 99, 99)` is a real date a few
  // days later), so the ranges are checked before it is built — a typo
  // must not silently become a different expiry the user never chose.
  if (minute > 59 || second > 59 || hour > 23 || day < 1 || day > 31 || month < 1 || month > 12) {
    return undefined;
  }
  const instant = new Date(year, month - 1, day, hour, minute, second);
  if (Number.isNaN(instant.getTime())) return undefined;
  // ... and the day-of-month is re-checked for the same reason (`02-31`
  // rolls into March).
  if (instant.getMonth() !== month - 1 || instant.getDate() !== day) return undefined;
  return instant.toISOString();
}

/**
 * Whether a local-instant field value is STRICTLY in the future.
 *
 * The client-side mirror of the server's rule: it exists so an obviously
 * past expiry is refused before a round trip, NOT as the authority — the
 * server still answers `-32602` `expires_at must be in the future` for
 * anything that slips through (clock skew, a value typed between the
 * check and the send), and that answer is surfaced too.
 */
export function isFutureField(value: string, now: Date = new Date()): boolean {
  const instant = fieldToInstant(value);
  if (instant === undefined) return false;
  return new Date(instant).getTime() > now.getTime();
}
