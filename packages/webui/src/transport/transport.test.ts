import { describe, expect, it } from "vitest";

import { rpcGetToken } from "./index";

/**
 * The session-layer contract (Round B S0), pinned where it lives:
 *
 * The credential is the httpOnly cookie — no script on the page owns a
 * token. The RPC client's token source must therefore answer `null`
 * ALWAYS: a non-null answer would make the client fall back to its
 * `?token=` URL channel, putting the JWT into URLs (server logs,
 * browser history, proxy buffers). If this test is red, the session
 * layer regressed to URL-carried credentials.
 */
describe("rpcGetToken (the session-layer contract)", () => {
  it("answers null — the credential never exists client-side", () => {
    expect(rpcGetToken()).toBeNull();
  });

  it("keeps answering null after anything a page could do to storage", () => {
    // Simulate a stale pre-migration world: a token sitting in every
    // storage hole. The getter must STILL refuse to see it.
    localStorage.setItem("i0721-token", "stale-jwt");
    localStorage.setItem("i0721-session", JSON.stringify({ token: "stale-jwt" }));
    expect(rpcGetToken()).toBeNull();
    localStorage.removeItem("i0721-token");
    localStorage.removeItem("i0721-session");
  });
});
