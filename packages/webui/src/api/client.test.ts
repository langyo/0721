import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// R-M①'s two-state ladder, pinned at the client's seams: the FIRST 401
// of a wave triggers exactly ONE `POST /api/v1/auth/refresh` (concurrent
// 401s JOIN the same attempt — no per-request refresh storm) and the
// original request is re-issued ONCE; a failed refresh surfaces the
// login state through the registered auth-lost reaction (never-signed-in
// and expired read identically — both only need a re-login).
//
// MUT:p2a-refresh-retry — dropping the replay (`if (await refreshAndRetry())`
// to an unconditional fall-through) or the refresh call itself reddens
// the success case; MUT:p2a-refresh-merge — giving each 401 its own
// refresh (no `??=`) reddens the concurrency case.

import { ApiError, apiFetch, onUnauthorized, resetUnauthorizedState } from "./client";

const fetchMock = vi.fn();
const refreshMock = vi.fn();

/** Route the client's raw `fetch` through the test double: the path
 *  distinguishes the lanes (the refresh endpoint vs anything else). */
function route(
  status: (path: string) => number,
  body: (path: string) => unknown = () => ({}),
): void {
  fetchMock.mockImplementation(async (input: RequestInfo | URL) => {
    const path = typeof input === "string" ? input : input.toString();
    const code = status(path);
    const payload = body(path);
    return {
      ok: code >= 200 && code < 300,
      status: code,
      statusText: `HTTP ${code}`,
      json: async () => payload,
      text: async () => (code === 204 ? "" : JSON.stringify(payload)),
    } as Response;
  });
}

beforeEach(() => {
  resetUnauthorizedState();
  fetchMock.mockReset();
  refreshMock.mockReset();
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
  resetUnauthorizedState();
  onUnauthorized(() => {});
});

describe("the 401 ladder (R-M①)", () => {
  it("retries the original request once after a successful refresh", async () => {
    let firstData = true;
    route((path) => {
      if (path === "/api/v1/auth/refresh") return 200;
      if (firstData) {
        firstData = false;
        return 401;
      }
      return 200;
    }, (path) => (path === "/api/v1/things" ? { ok: true } : {}));
    const data = await apiFetch<{ ok: boolean }>("/api/v1/things");
    expect(data).toEqual({ ok: true });
    // The endpoint was hit exactly once, then the original re-issued.
    const refreshCalls = fetchMock.mock.calls.filter(
      ([input]) => String(input).includes("/api/v1/auth/refresh"),
    );
    expect(refreshCalls).toHaveLength(1);
    const dataCalls = fetchMock.mock.calls.filter(
      ([input]) => String(input) === "/api/v1/things",
    );
    expect(dataCalls).toHaveLength(2);
  });

  it("does NOT retry when the refresh itself is refused — the auth-lost reaction owns the login state", async () => {
    route((path) => (path === "/api/v1/auth/refresh" ? 401 : 401));
    const lost = vi.fn();
    onUnauthorized(lost);
    await expect(apiFetch("/api/v1/things")).rejects.toMatchObject({
      status: 401,
    });
    // Exactly ONE refresh attempt (no storm); the reaction fires per 401
    // (both the refresh refusal and the final replay's 401 land there —
    // the two-state shape: this state means "sign in again").
    const refreshCalls = fetchMock.mock.calls.filter(
      ([input]) => String(input).includes("/api/v1/auth/refresh"),
    );
    expect(refreshCalls).toHaveLength(1);
    expect(lost).toHaveBeenCalledTimes(2);
  });

  it("merges concurrent 401s into ONE refresh attempt", async () => {
    // The response is DELAYED so all three 401s land inside the merge
    // window (a resolved promise would serialize the ladder before the
    // others even start). The refresh flips a switch: the data paths
    // answer 200 from then on (the replay succeeds).
    let refreshes = 0;
    fetchMock.mockImplementation(
      (input: RequestInfo | URL) =>
        new Promise((resolve) => {
          const path = String(input);
          setTimeout(() => {
            if (path === "/api/v1/auth/refresh") {
              refreshes += 1;
              resolve({ ok: true, status: 200, statusText: "HTTP 200", json: async () => ({}), text: async () => "{}" } as Response);
            } else if (refreshes > 0) {
              resolve({ ok: true, status: 200, statusText: "HTTP 200", json: async () => ({}), text: async () => "{}" } as Response);
            } else {
              resolve({ ok: false, status: 401, statusText: "HTTP 401", json: async () => ({}), text: async () => "{}" } as Response);
            }
          }, 10);
        }),
    );
    const [a, b, c] = await Promise.all([
      apiFetch("/api/v1/a"),
      apiFetch("/api/v1/b"),
      apiFetch("/api/v1/c"),
    ]);
    expect(a).toEqual({});
    expect(b).toEqual({});
    expect(c).toEqual({});
    // MUT:p2a-refresh-merge — one refresh per REQUEST (three here) means
    // the merge is gone.
    expect(refreshes).toBe(1);
  });

  it("never retries twice: a second 401 after the replay goes to the reaction", async () => {
    route(() => 401);
    const lost = vi.fn();
    onUnauthorized(lost);
    await expect(apiFetch("/api/v1/things")).rejects.toBeInstanceOf(ApiError);
    const refreshCalls = fetchMock.mock.calls.filter(
      ([input]) => String(input).includes("/api/v1/auth/refresh"),
    );
    expect(refreshCalls).toHaveLength(1);
    expect(lost).toHaveBeenCalledTimes(2);
  });
});
