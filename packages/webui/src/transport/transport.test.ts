import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  clearWsTicket,
  connectTransport,
  refreshWsTicket,
  rpcGetToken,
} from "./index";
import { RpcClient } from "@celestia-island/plana-rpc-client";

/**
 * The session-layer contract (Round B S0 + Round D S1), pinned where it
 * lives:
 *
 * 1. The SESSION credential (the JWT behind the httpOnly cookie) never
 *    appears in `rpcGetToken` — that value rides the WS upgrade URL,
 *    and a session JWT in a URL leaks into logs, history and proxies.
 *    Only the ONE-TIME admission ticket (≤30 s, single-use) may be
 *    returned. If this test is red, the session layer regressed to
 *    URL-carried session credentials.
 * 2. The WS tier is GENUINELY attempted: with a cached ticket, a fresh
 *    RpcClient constructs a WebSocket whose URL carries the ticket. If
 *    `rpcGetToken` regresses to null, no WebSocket is constructed (the
 *    client short-circuits) and this test goes red — the exact
 *    production-all-POST failure mode Round D was about.
 */

const fetchMock = vi.fn();

beforeEach(() => {
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  clearWsTicket();
  localStorage.clear();
  fetchMock.mockReset();
  vi.unstubAllGlobals();
});

describe("rpcGetToken (the session-layer contract)", () => {
  it("returns the one-time ticket, never a session JWT from storage", async () => {
    // Seed the pre-migration world: a session JWT in every storage hole.
    localStorage.setItem("i0721-token", "stale-session-jwt");
    localStorage.setItem("i0721-session", JSON.stringify({ token: "stale-session-jwt" }));

    // A ticket mint (cookie-authenticated REST call).
    fetchMock.mockResolvedValueOnce(
      new Response(JSON.stringify({ ticket: "one-time-ticket-xyz" }), { status: 200 }),
    );
    await refreshWsTicket();

    expect(rpcGetToken()).toBe("one-time-ticket-xyz");
    expect(rpcGetToken()).not.toContain("stale-session-jwt");
  });

  it("keeps returning null before any mint (no fabrication)", () => {
    localStorage.setItem("i0721-token", "stale-session-jwt");
    expect(rpcGetToken()).toBeNull();
  });
});

describe("the WS tier is genuinely attempted", () => {
  it("constructs a WebSocket whose URL carries the ticket", async () => {
    fetchMock.mockResolvedValueOnce(
      new Response(JSON.stringify({ ticket: "ws-ticket-abc123" }), { status: 200 }),
    );
    await connectTransport();
    expect(rpcGetToken()).toBe("ws-ticket-abc123");

    // A FRESH client on the same token getter: its connect() must build
    // a WebSocket (the tier race's WS arm) with the ticket in the URL.
    const constructed: string[] = [];
    class WsSpy {
      constructor(url: string | URL) {
        constructed.push(String(url));
      }
      close() {}
    }
    vi.stubGlobal("WebSocket", WsSpy);

    const client = new RpcClient({
      baseUrl: "",
      rpcPath: "/api/rpc",
      getToken: rpcGetToken,
    });
    client.connect();
    // The tier race is asynchronous — give the microtask queue a beat.
    await new Promise((resolve) => setTimeout(resolve, 20));

    expect(constructed.length).toBeGreaterThan(0);
    expect(constructed[0]).toContain("/api/rpc");
    expect(constructed[0]).toContain("token=ws-ticket-abc123");
  });

  it("constructs NO WebSocket when the getter answers null (the regression shape)", async () => {
    // No mint happened: the getter answers null — the client must NOT
    // attempt the WS tier at all. (This documents the short-circuit the
    // ticket exists to satisfy; if the getter ever regresses to null in
    // production, this is the all-POST mode.)
    const constructed: string[] = [];
    class WsSpy {
      constructor(url: string | URL) {
        constructed.push(String(url));
      }
      close() {}
    }
    vi.stubGlobal("WebSocket", WsSpy);

    const client = new RpcClient({
      baseUrl: "",
      rpcPath: "/api/rpc",
      getToken: () => null,
    });
    client.connect();
    await new Promise((resolve) => setTimeout(resolve, 20));

    expect(constructed.length).toBe(0);
  });
});
