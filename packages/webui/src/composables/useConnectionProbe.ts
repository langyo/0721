import type { ConnectionStateEvent } from "@celestia-island/plana-rpc-client";
import { RpcClient } from "@celestia-island/plana-rpc-client";
import { onMounted, onUnmounted, ref, type Ref } from "vue";

/**
 * The RpcClient-driven connection probe (the chest shape, web-only).
 * The transport singleton's state events feed the bottom-left status
 * light; `retryNow` is the light's tap action (forceReconnect).
 */
export interface ProbeResult {
  connected: boolean;
  state: "connected" | "disconnected" | "connecting" | "reconnecting" | "failed";
  /** Canonical transport tier ("ws" | "poll" | …). */
  transportTier: string;
  /** @deprecated Alias of `transportTier` (hikari's shape keeps it). */
  tier: string;
  latencyMs: number | null;
  /** Canonical 1-based connect-attempt counter. */
  attemptNumber: number;
  /** @deprecated Alias of `attemptNumber` (hikari's shape keeps it). */
  retryCount: number;
  retryTotal: number;
  countdown: number;
}

let sharedClient: RpcClient | null = null;

/** Install the transport singleton the probes read (main.ts, after the
 *  client exists — the login flow reconnects it per session). */
export function setProbeClient(client: RpcClient): void {
  sharedClient = client;
}

export function useConnectionProbe(): {
  result: Ref<ProbeResult>;
  retryNow: () => void;
} {
  const result = ref<ProbeResult>({
    connected: false,
    state: "disconnected",
    transportTier: "ws",
    tier: "ws",
    latencyMs: null,
    attemptNumber: 0,
    retryCount: 0,
    retryTotal: 3,
    countdown: 0,
  });

  let unsub: (() => void) | null = null;

  function updateState(e: ConnectionStateEvent): void {
    const prev = result.value;
    const tier = e.transportTier ?? sharedClient?.transportTier ?? prev.transportTier;
    const attempt = e.attemptNumber ?? e.retryCount ?? prev.attemptNumber;
    // Heartbeat RTT arrives as a partial event; a lost connection has
    // no meaningful latency, otherwise keep the last measurement.
    const latencyMs =
      e.latencyMs !== undefined
        ? e.latencyMs
        : e.state === "disconnected" || e.state === "failed"
          ? null
          : prev.latencyMs;
    result.value = {
      ...prev,
      connected: e.state === "connected",
      state: e.state as ProbeResult["state"],
      transportTier: tier,
      tier,
      attemptNumber: attempt,
      retryCount: attempt,
      retryTotal: e.maxRetries ?? prev.retryTotal,
      latencyMs,
      countdown: e.countdown ?? 0,
    };
  }

  function retryNow(): void {
    sharedClient?.forceReconnect();
  }

  onMounted(() => {
    if (sharedClient) {
      updateState({
        state: sharedClient.state,
        retryCount: sharedClient.retryCount,
        transportTier: sharedClient.transportTier,
        latencyMs: sharedClient.latencyMs ?? undefined,
      });
      unsub = sharedClient.on("state", updateState);
    }
  });

  onUnmounted(() => {
    unsub?.();
  });

  return { result, retryNow };
}
