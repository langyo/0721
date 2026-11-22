import {
  reportHkRuntime,
  scheduleInterval,
  type HkRuntimeHandle,
  type IntervalHandle,
} from "@celestia-island/hikari";
import { onMounted, onUnmounted, ref, type Ref } from "vue";

/**
 * The fetch-based connectivity probe (the chest sessionless-surface
 * shape): polls `/api/health` and derives the same ProbeResult the
 * status shell reads, without any RPC session — the login face's probe
 * (a WS upgrade there would hang red: no token yet).
 */
export interface ProbeResult {
  connected: boolean;
  state: "connected" | "disconnected" | "connecting" | "reconnecting" | "failed";
  transportTier: string;
  /** @deprecated Alias of `transportTier` (hikari's shape keeps it). */
  tier: string;
  latencyMs: number | null;
  attemptNumber: number;
  /** @deprecated Alias of `attemptNumber` (hikari's shape keeps it). */
  retryCount: number;
  retryTotal: number;
  countdown: number;
}

const POLL_MS = 15_000;
const RETRY_TOTAL = 3;

// Runtime-registry reporting (the "context of contexts"): one registry
// entry counting live probe instances.
let liveProbes = 0;
let probeRuntime: HkRuntimeHandle | null = null;
function ensureProbeRuntime(): HkRuntimeHandle {
  return (probeRuntime ??= reportHkRuntime("connectionProbe", {
    kind: "hook",
    description:
      "The fetch-based connectivity probe: per-component /api/health polling with retry and countdown.",
    read: () => ({ liveProbes }),
  }));
}

export function useFetchConnectionProbe(): {
  result: Ref<ProbeResult>;
  retryNow: () => void;
} {
  const result = ref<ProbeResult>({
    connected: false,
    state: "disconnected",
    transportTier: "poll",
    tier: "poll",
    latencyMs: null,
    attemptNumber: 0,
    retryCount: 0,
    retryTotal: RETRY_TOTAL,
    countdown: 0,
  });

  let tickHandle: IntervalHandle | null = null;
  let countdownHandle: IntervalHandle | null = null;
  let attempt = 0;

  async function probe(): Promise<void> {
    attempt += 1;
    const started = performance.now();
    try {
      const resp = await fetch("/api/health", { method: "GET" });
      result.value = {
        connected: resp.ok,
        state: resp.ok ? "connected" : "failed",
        transportTier: "poll",
        tier: "poll",
        latencyMs: Math.round(performance.now() - started),
        attemptNumber: attempt,
        retryCount: attempt,
        retryTotal: RETRY_TOTAL,
        countdown: 0,
      };
    } catch {
      result.value = {
        connected: false,
        state: attempt >= RETRY_TOTAL ? "failed" : "reconnecting",
        transportTier: "poll",
        tier: "poll",
        latencyMs: null,
        attemptNumber: attempt,
        retryCount: attempt,
        retryTotal: RETRY_TOTAL,
        countdown: attempt >= RETRY_TOTAL ? 0 : POLL_MS / 1000,
      };
    }
  }

  function retryNow(): void {
    attempt = 0;
    void probe();
  }

  onMounted(() => {
    ensureProbeRuntime();
    liveProbes += 1;
    void probe();
    tickHandle = scheduleInterval(() => void probe(), POLL_MS);
    countdownHandle = scheduleInterval(() => {
      if (result.value.state === "reconnecting" && result.value.countdown > 0) {
        result.value = { ...result.value, countdown: result.value.countdown - 1 };
      }
    }, 1000);
  });

  onUnmounted(() => {
    liveProbes -= 1;
    tickHandle?.disconnect();
    countdownHandle?.disconnect();
    tickHandle = null;
    countdownHandle = null;
  });

  return { result, retryNow };
}
