import { RpcClient } from "@celestia-island/plana-rpc-client";

import { loadSession } from "@/api/client";

/**
 * The JSON-RPC transport singleton (the chest web-transport shape,
 * minus the Tauri branch — this is a browser-only app).
 *
 * One `RpcClient` from the family's plana-rpc-client rides the whole
 * app: lazy WebSocket to `/api/rpc` with the heartbeat protocol the
 * plana server pair speaks, HTTP POST fallback on the same method map,
 * progressive reconnect, and token injection through `getToken` (the
 * credential is read per attempt, so a login/logout flips the lane
 * without rebuilding the client).
 *
 * The token rides the `Sec-WebSocket-Protocol: jwt.<t>` channel (the
 * client's built-in mechanism): no query parameter, no cookie.
 */

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
  getToken: () => loadSession()?.token ?? null,
  onAuthLost: () => unauthorizedHandler?.(),
});
