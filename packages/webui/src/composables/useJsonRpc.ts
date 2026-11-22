import { RpcError } from "@celestia-island/plana-rpc-client";

import { rpcClient } from "@/transport";

/**
 * The JSON-RPC call surface (the chest wsCall contract).
 *
 * Error policy (the user-facing split): a connection-level failure —
 * `RpcError` of kind `transport`/`timeout` (and the forbidden family) —
 * is NOT toasted anywhere: it surfaces in the bottom-left status light,
 * which is the single communication-context indicator. Only genuine
 * application-level errors (kind `rpc` — the server answered with a
 * JSON-RPC error object) are reported through the toast hook; callers
 * additionally receive the rejection for their inline error surfaces.
 */

type ErrorReporter = (message: string) => void;

let toastReporter: ErrorReporter | null = null;

/** Registers the app's rpc-error toast (main wires hikari's useToast
 *  publisher here; without one, rpc errors pass through silently to the
 *  callers' inline surfaces). */
export function setRpcErrorReporter(reporter: ErrorReporter): void {
  toastReporter = reporter;
}

export async function wsCall<T>(
  method: string,
  params?: Record<string, unknown>,
  opts?: { timeoutMs?: number },
): Promise<T> {
  try {
    return await rpcClient.call<T>(method, params, opts?.timeoutMs);
  } catch (err: unknown) {
    if (err instanceof RpcError) {
      if (err.kind === "rpc") {
        // Application-level error: actionable, worth a toast.
        toastReporter?.(err.message);
      }
      // transport/timeout/forbidden: the status light owns these.
      throw err;
    }
    throw err;
  }
}

export { RpcError };
