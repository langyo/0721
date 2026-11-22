import {
  scheduleInterval,
  type EngineHealth,
  type IntervalHandle,
} from "@celestia-island/hikari";
import { onMounted, onUnmounted, ref, type Ref } from "vue";

export type { EngineHealth };

/** The `/api/health` response — OUR wire shape (the ts-rs HealthReport
 *  contract: status/version/config_source). */
interface HealthResponse {
  status: string;
  version: string;
  config_source: string;
}

/** Visibility-aware refresh cadence (hikari's interval bus parks while
 *  the tab is hidden). 30s: engine identity is not latency-critical,
 *  but a transient mount-time failure must not blank the row forever. */
const REFRESH_MS = 30_000;

/**
 * Fetch `/api/health` on mount and keep it fresh on a 30s
 * visibility-aware loop; expose the engine subset the status card's
 * popover renders (the version row; our report carries no network
 * block, so that row stays null).
 */
export function useEngineHealth(): {
  health: Ref<EngineHealth | null>;
  loading: Ref<boolean>;
  error: Ref<string | null>;
  refresh: () => Promise<void>;
} {
  const health = ref<EngineHealth | null>(null);
  const loading = ref(false);
  const error = ref<string | null>(null);
  let tickHandle: IntervalHandle | null = null;

  async function refresh(): Promise<void> {
    loading.value = true;
    error.value = null;
    try {
      const resp = await fetch("/api/health", { method: "GET" });
      if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
      const body = (await resp.json()) as HealthResponse;
      health.value = {
        engineVersion: body.version || null,
        engineBuildHash: null,
        network: null,
      };
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    } finally {
      loading.value = false;
    }
  }

  onMounted(() => {
    void refresh();
    tickHandle = scheduleInterval(() => {
      // Never stack requests on a slow endpoint.
      if (!loading.value) void refresh();
    }, REFRESH_MS);
  });

  onUnmounted(() => {
    tickHandle?.disconnect();
    tickHandle = null;
  });

  return { health, loading, error, refresh };
}
