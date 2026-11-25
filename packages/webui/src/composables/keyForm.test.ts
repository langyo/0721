import { describe, expect, it } from "vitest";

import {
  KEY_OPS,
  canonicalOps,
  fieldToInstant,
  instantToField,
  isFutureField,
  toDateTimeLocal,
} from "./keyForm";

/**
 * The key form's two conversion lanes.
 *
 * Both directions cross the same boundary — the `datetime-local` field is
 * timezone-NAIVE (its text is the user's local wall clock) while the wire
 * carries RFC 3339 UTC — and a naive string passthrough passes every
 * assertion that only compares ROUND TRIPS. These cases therefore pin the
 * ABSOLUTE instant on each side, not just that `decode(encode(x)) == x`.
 */

/** A fixed instant with a deliberate non-UTC offset (02:03:04Z). */
const INSTANT = "2027-03-04T02:03:04Z";

describe("canonicalOps", () => {
  it("orders by the canonical vocabulary and drops duplicates", () => {
    expect(canonicalOps(["share", "read", "upload"])).toEqual(["read", "upload", "share"]);
    expect(canonicalOps(["share", "share"])).toEqual(["share"]);
    expect(canonicalOps([])).toEqual([]);
  });

  it("pins the vocabulary and its order to the generated union", () => {
    // The literal array is the runtime mirror of `KeyOp`; a variant added
    // to the generated type without landing here would silently never be
    // selectable in the dialog.
    expect(KEY_OPS).toEqual(["read", "upload", "share"]);
  });
});

describe("fieldToInstant", () => {
  it("reads the field as LOCAL wall-clock, not UTC", () => {
    // The trap this exists for: `new Date("2027-03-04T02:03")` is local
    // (spec'd), but a string passthrough to the wire would be read as UTC
    // by the server. The instant must carry the browser's offset.
    const local = "2027-03-04T02:03";
    const expected = new Date(2027, 2, 4, 2, 3, 0).toISOString();
    expect(fieldToInstant(local)).toBe(expected);
    expect(new Date(fieldToInstant(local)!).getTime()).toBe(
      new Date(2027, 2, 4, 2, 3, 0).getTime(),
    );
  });

  it("accepts the seconds-bearing spelling too", () => {
    const withSeconds = "2027-03-04T02:03:45";
    expect(new Date(fieldToInstant(withSeconds)!).getTime()).toBe(
      new Date(2027, 2, 4, 2, 3, 45).getTime(),
    );
  });

  it("maps an empty field to undefined (no expiry)", () => {
    // This is the whole "clear the expiry" lane: absent, never "".
    expect(fieldToInstant("")).toBeUndefined();
    expect(fieldToInstant("   ")).toBeUndefined();
  });

  it("maps garbage to undefined rather than inventing a date", () => {
    expect(fieldToInstant("not a date")).toBeUndefined();
    expect(fieldToInstant("2027-03-04")).toBeUndefined();
    // The `Date` constructor ROLLS OVER out-of-range parts; a typo must
    // not silently become a different expiry the user never chose.
    expect(fieldToInstant("2027-03-04T99:99")).toBeUndefined();
    expect(fieldToInstant("2027-02-31T10:00")).toBeUndefined();
    expect(fieldToInstant("2027-13-01T10:00")).toBeUndefined();
  });
});

describe("instantToField", () => {
  it("renders the same MOMENT as local wall clock", () => {
    const field = instantToField(INSTANT);
    // The field carries MINUTE resolution (that is what `datetime-local`
    // holds), so the round trip lands on the same minute — not on the
    // raw UTC text, which would shift the instant by the browser's offset.
    expect(field).toBe(toDateTimeLocal(new Date(INSTANT)));
    expect(new Date(fieldToInstant(field)!).getTime()).toBe(
      Math.floor(new Date(INSTANT).getTime() / 60_000) * 60_000,
    );
  });

  it("maps absent / unparseable values to the empty field", () => {
    expect(instantToField(undefined)).toBe("");
    expect(instantToField(null)).toBe("");
    expect(instantToField("garbage")).toBe("");
  });
});

describe("toDateTimeLocal", () => {
  it("zero-pads every part and drops seconds", () => {
    expect(toDateTimeLocal(new Date(2027, 0, 5, 7, 8, 9))).toBe("2027-01-05T07:08");
  });
});

describe("isFutureField", () => {
  it("is strictly future-aware", () => {
    const now = new Date(2027, 2, 4, 12, 0, 0);
    expect(isFutureField("2027-03-04T12:01", now)).toBe(true);
    expect(isFutureField("2027-03-04T11:59", now)).toBe(false);
    // Exactly now is NOT in the future (the server's rule is strict).
    expect(isFutureField("2027-03-04T12:00", now)).toBe(false);
  });

  it("treats an empty or unparseable field as not-an-expiry to check", () => {
    // The field's absent state is "no expiry", so the guard's callers
    // short-circuit on it; the predicate itself must not call it future.
    expect(isFutureField("")).toBe(false);
  });
});
