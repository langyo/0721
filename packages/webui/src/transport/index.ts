import { RpcClient } from "@celestia-island/plana-rpc-client";

/**
 * The JSON-RPC transport singleton (the chest web-transport shape,
 * minus the Tauri branch — this is a browser-only app).
 *
 * ## Where the credential lives
 *
 * The session token rides the **httpOnly cookie** the REST login plants
 * (`i0721-token`): same-origin WebSocket and HTTP requests carry it
 * automatically, and no script on the page can read it. This client
 * therefore hands the RPC client a getToken that answers `null` —
 * ALWAYS — so the client never falls back to its `?token=` URL channel
 * (a JWT in a URL leaks into logs, history and proxies). The server
 * keeps `?token=` and `Sec-WebSocket-Protocol: jwt.<t>` open for
 * NON-browser clients; the webui production path is the cookie and
 * nothing else.
 *
 * One `RpcClient` from the family's plana-rpc-client rides the whole
 * app: lazy WebSocket to `/api/rpc` with the heartbeat protocol the
 * plana server pair speaks, HTTP POST fallback on the same method map,
 * and progressive reconnect.
 */

/**
 * The token source handed to the RPC client — deliberately inert.
 * Pinned by transport.test.ts: if this ever returns a stored token
 * again, the client would start putting JWTs into URLs.
 */
export function rpcGetToken(): string | null {
  return null;
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
