<script setup lang="ts">
import {
  HK_CONNECTION_PROBE,
  HK_ENGINE_HEALTH,
  HkConnectionStatus,
  HkIconButton,
} from "@celestia-island/hikari";
import { RefreshCw } from "lucide-vue-next";
import { provide } from "vue";
import { useI18n } from "vue-i18n";

import { useConnectionProbe } from "@/composables/useConnectionProbe";
import { useEngineHealth } from "@/composables/useEngineHealth";
import { useFetchConnectionProbe } from "@/composables/useFetchConnectionProbe";

/**
 * The bottom-left connection light — the single wiring of hikari's
 * status card (the chest ConnectionStatusTag contract). The only
 * variant is the probe source:
 *
 *  - `"transport"` (default): logged-in surfaces read the RpcClient's
 *    live state (WS tier, heartbeat latency, reconnect rounds);
 *  - `"health"`: sessionless pages (the login face — no WS session
 *    yet) keep the same-origin `/api/health` fetch probe.
 *
 * Engine identity rides `/api/health` either way (HK_ENGINE_HEALTH).
 * `idle` pins the neutral gray "shell alive, no session" state for the
 * auth face.
 */
const props = withDefaults(
  defineProps<{
    compact?: boolean;
    source?: "transport" | "health";
    idle?: boolean;
  }>(),
  { compact: false, source: "transport", idle: false },
);

const { t } = useI18n();
const { health: engineHealth } = useEngineHealth();
provide(HK_ENGINE_HEALTH, engineHealth);

if (props.source === "transport") {
  provide(HK_CONNECTION_PROBE, useConnectionProbe());
} else {
  provide(HK_CONNECTION_PROBE, useFetchConnectionProbe());
}

function reload() {
  window.location.reload();
}
</script>

<template>
  <HkConnectionStatus :standalone="false" :compact="compact" :idle="idle">
    <template #actions>
      <HkIconButton
        :size="24"
        variant="ghost"
        :aria-label="t('common.actions.retry')"
        :title="t('common.actions.retry')"
        data-testid="manual-refresh"
        @click="reload"
      >
        <template #icon><RefreshCw :size="16" /></template>
      </HkIconButton>
    </template>
  </HkConnectionStatus>
</template>
