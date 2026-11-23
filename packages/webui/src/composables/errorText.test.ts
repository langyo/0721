import { describe, expect, it, vi } from "vitest";

import { makeErrorText } from "./errorText";

/**
 * The factory's shape contract — the table every consumer's falsy-gated
 * lane leans on. MUT:errortext-shapes — removing either guard (the
 * `.message` truthiness or the string-arm length check) or flipping a
 * return reddens the corresponding row here.
 */

const t = vi.fn((key: string) => (key === "common.status.error" ? "Something went wrong" : "Upload failed"));

describe("makeErrorText", () => {
  it("returns the message of an Error that has one", () => {
    expect(makeErrorText(t)(new Error("boom"))).toBe("boom");
  });

  it("falls back for an Error with an EMPTY message — never an empty string", () => {
    // MUT:errortext-shapes — dropping the `.message` guard returns ""
    // here, and an empty string renders NO alert on a falsy-gated lane.
    expect(makeErrorText(t)(new Error(""))).toBe("Something went wrong");
  });

  it("carries a bare-string rejection (the JSON-RPC lane's shape)", () => {
    expect(makeErrorText(t)("rpc timeout")).toBe("rpc timeout");
    expect(makeErrorText(t)("")).toBe("Something went wrong");
  });

  it("falls back for everything else", () => {
    expect(makeErrorText(t)(undefined)).toBe("Something went wrong");
    expect(makeErrorText(t)(null)).toBe("Something went wrong");
    expect(makeErrorText(t)({ x: 1 })).toBe("Something went wrong");
  });

  it("honours the caller's fallback key", () => {
    const upload = makeErrorText(t, "common.portal.uploadFailed");
    expect(upload(new Error(""))).toBe("Upload failed");
    expect(upload(undefined)).toBe("Upload failed");
  });
});