import { RpcClient } from "@celestia-island/plana-rpc-client";

/**
 * The JSON-RPC transport singleton (the chest web-transport shape,
 * minus the Tauri branch — this is a browser-only app).
 *
 * ## The two-tier transport, and where each credential lives
 *
 * - **WS is the preferred tier** (`/api/rpc` upgrade, heartbeat +
 *   latency). The family's client only ATTEMPTS the upgrade when its
 *   token getter answers non-null — and a session JWT must never ride
 *   a URL. The reconciliation is the chest ws-bridge pattern: a
 *   ONE-TIME admission ticket (minted by `POST /api/v1/auth/ws-ticket`,
 *   ≤30 s, user-bound, consumed by the opening itself) occupies the
 *   `?token=` slot. A leaked ticket is worth exactly one socket
 *   opening inside half a minute — bounded by construction.
 * - **The HTTP POST fallback** (same method map) authenticates with
 *   the httpOnly session cookie (`credentials: include`); the client
 *   also echoes the cached ticket into an Authorization header, but
 *   the server reads the cookie FIRST, so the ticket is never burned
 *   by a POST.
 *
 * The session cookie is planted by the REST login and never leaves the
 * server's Set-Cookie — no script on the page can read it.
 * `rpcGetToken` therefore NEVER returns a session credential; it
 * returns the cached one-time ticket only (pinned by
 * transport.test.ts).
 */

/**
 * The cached one-time WS admission ticket (short-lived, single-use —
 * refreshed before every connect/reconnect attempt wave).
 */
let cachedTicket: string | null = null;

/**
 * The token source handed to the RPC client: the CACHED ONE-TIME
 * TICKET, never a session credential. Pinned by transport.test.ts: a
 * session JWT must never appear here (it would ride URLs).
 */
export function rpcGetToken(): string | null {
  return cachedTicket;
}

/** Fetch a fresh admission ticket (the cookie authenticates the mint). */
export async function refreshWsTicket(): Promise<void> {
  try {
    const response = await fetch("/api/v1/auth/ws-ticket", {
      method: "POST",
      credentials: "same-origin",
    });
    if (!response.ok) return;
    const body = (await response.json()) as { ticket?: string };
    if (typeof body.ticket === "string" && body.ticket) {
      cachedTicket = body.ticket;
    }
  } catch {
    // Offline mint: keep the previous ticket; the WS attempt simply
    // falls to the POST fallback for this round.
  }
}

let unauthorizedHandler: (() => void) | null = null;

/** Registers the global auth-lost reaction (the auth store wires logout
 *  here — an expired watermark closes the socket with an auth close
 *  code and every later call would otherwise spin). */
export function onRpcAuthLost(handler: () => void): void {
  unauthorizedHandler = handler;
}

export const rpcClient = new RpcClient({
  baseUrl: "",
  rpcPath: "/api/rpc",
  getToken: rpcGetToken,
  onAuthLost: () => unauthorizedHandler?.(),
});

// Keep the ticket cache fresh across reconnect waves: whenever the
// client leaves the connected state (drop, recovery, manual retry),
// mint a new ticket so the NEXT upgrade attempt reads a live one. A
// stale or consumed ticket costs nothing — that attempt falls to the
// POST fallback and the following attempt gets the fresh mint.
rpcClient.on("state", (event) => {
  if (event.state !== "connected") {
    void refreshWsTicket();
  }
});

/** Test seam: drop the cached ticket (between sessions). */
export function clearWsTicket(): void {
  cachedTicket = null;
}

/**
 * Connect the transport with a freshly minted ticket (call after
 * login/session-restore).
 */
export async function connectTransport(): Promise<void> {
  await refreshWsTicket();
  void rpcClient.connect();
}
