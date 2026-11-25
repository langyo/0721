import { describe, expect, it } from "vitest";

// The key form's bucket lane (Q7): the canonical form is the slug array
// ≤ 64, DEDUPED, lexicographically sorted — the same canonicalization
// `encode_scopes_with_buckets` applies server-side, so the wire can
// never carry an unsorted or duplicated list.
//
// MUT:p2a-key-buckets-canonical — dropping the sort or the dedupe
// reddens the ordering case.

import { canonicalBuckets, keyBucketsOf } from "./keyForm";

describe("canonicalBuckets (Q7's canonical form)", () => {
  it("sorts and dedupes", () => {
    expect(canonicalBuckets(["zeta", "alpha", "zeta", "beta"])).toEqual([
      "alpha",
      "beta",
      "zeta",
    ]);
  });

  it("caps at 64 entries", () => {
    const many = Array.from({ length: 70 }, (_, i) => `b${String(i).padStart(2, "0")}`);
    const out = canonicalBuckets(many);
    expect(out).toHaveLength(64);
    expect(out[0]).toBe("b00");
  });

  it("keeps the explicit empty array as an empty array", () => {
    expect(canonicalBuckets([])).toEqual([]);
  });
});

describe("keyBucketsOf (the K-era detection)", () => {
  it("answers null for a scopes WITHOUT the buckets array", () => {
    expect(keyBucketsOf(undefined)).toBeNull();
    expect(keyBucketsOf(null)).toBeNull();
  });

  it("passes the explicit array through (empty included)", () => {
    expect(keyBucketsOf([])).toEqual([]);
    expect(keyBucketsOf(["a"])).toEqual(["a"]);
  });
});
