import { describe, expect, it } from "vitest";

import {
  identityFromEnv,
  parseIdentityToken,
  resolveBuildIdentity,
} from "@/lib/buildIdentity";

/**
 * Build-identity parse rules (family rule: the footer shows
 * `<semver> <branch>:<hash7>`).
 *
 * The shape check guards the CONTAINER path — a malformed
 * `PANEL_BUILD_IDENTITY` build-arg must fall THROUGH the ladder rather than
 * bake a bogus identity into a shipped image. That failure is invisible in
 * any build/test gate (the bundle simply carries the wrong string), so it is
 * pinned here directly.
 *
 * MUT:identity-shape-off — making `parseIdentityToken` accept anything
 * reddens the rejection table below.
 */
describe("parseIdentityToken", () => {
  it("accepts the family shape", () => {
    expect(parseIdentityToken("master:deadbee")).toBe("master:deadbee");
    expect(parseIdentityToken("  master:deadbee\n")).toBe("master:deadbee");
    expect(parseIdentityToken("feature-x:0123abc")).toBe("feature-x:0123abc");
  });

  it("normalises the benign shapes", () => {
    // A trailing colon on the branch and an uppercase hash both collapse.
    expect(parseIdentityToken("master::deadbee")).toBe("master:deadbee");
  });

  it("rejects anything that is not <branch>:<7..40 lowercase hex>", () => {
    const rejected = [
      "master:XYZ", // not hex
      "master:abc", // too short
      "master:ABCDEF1", // uppercase
      "master:0123456789abcdef0123456789abcdef01234567a", // 41 chars, too long
      ":deadbee", // no branch
      "deadbee", // no separator
      "", // empty
      "master:", // no hash
    ];
    for (const raw of rejected) {
      expect(parseIdentityToken(raw), `should reject ${JSON.stringify(raw)}`).toBeUndefined();
    }
  });
});

describe("identityFromEnv", () => {
  it("reads the container hand-off and shape-checks it", () => {
    expect(identityFromEnv({ PANEL_BUILD_IDENTITY: "master:deadbee" })).toBe("master:deadbee");
    expect(identityFromEnv({ PANEL_BUILD_IDENTITY: "master:XYZ" })).toBeUndefined();
    expect(identityFromEnv({ PANEL_BUILD_IDENTITY: "   " })).toBeUndefined();
    expect(identityFromEnv({})).toBeUndefined();
  });
});

describe("resolveBuildIdentity ladder", () => {
  it("prefers the env hand-off over git", () => {
    // MUT:identity-env-precedes-git — swapping the ladder (git first) would
    // make this root answer undefined instead of the env identity.
    // A bogus root proves git was never consulted: the env arm won.
    const prev = process.env.PANEL_BUILD_IDENTITY;
    process.env.PANEL_BUILD_IDENTITY = "master:deadbee";
    try {
      expect(resolveBuildIdentity("/nonexistent-repo-root")).toBe("master:deadbee");
    } finally {
      if (prev === undefined) delete process.env.PANEL_BUILD_IDENTITY;
      else process.env.PANEL_BUILD_IDENTITY = prev;
    }
  });

  it("falls back to undefined when neither arm can answer", () => {
    const prev = process.env.PANEL_BUILD_IDENTITY;
    delete process.env.PANEL_BUILD_IDENTITY;
    try {
      expect(resolveBuildIdentity("/nonexistent-repo-root")).toBeUndefined();
    } finally {
      if (prev !== undefined) process.env.PANEL_BUILD_IDENTITY = prev;
    }
  });
});
