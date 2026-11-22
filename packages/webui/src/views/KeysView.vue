<script setup lang="ts">
import { HkAdaptiveDialog, HkButton, HkEmptyState, HkInput } from "@celestia-island/hikari";
import { Copy, KeyRound, Trash2 } from "lucide-vue-next";
import { onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import * as api from "@/api";
import { usePageCreateAction } from "@/composables/usePageCreateAction";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { ApiKeyCreated, ApiKeyPublic } from "@/api";

const { t } = useI18n();
const keys = ref<ApiKeyPublic[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);
const created = ref<ApiKeyCreated | null>(null);
const createOpen = ref(false);
const newName = ref("");
const createBusy = ref(false);
/// Which copy button most recently succeeded (the tri-state: a target,
/// or nothing). A FAILED copy is its own flag so the failure is visible.
type CopyTarget = "key" | "config" | "prompt";
const copied = ref<CopyTarget | null>(null);
const copyFailed = ref(false);
const pendingRevoke = ref<ApiKeyPublic | null>(null);
const revokeBusy = ref(false);

async function load() {
  loading.value = true;
  try { keys.value = await api.listKeys(); } catch (e) { error.value = e instanceof Error ? e.message : ""; } finally { loading.value = false; }
}
onMounted(() => void load());
usePageRefresh(() => load(), { loading });
usePageCreateAction({
  label: () => t("common.keys.create"),
  onCreate: () => { newName.value = ""; created.value = null; createOpen.value = true; },
});

async function doCreate() {
  if (!newName.value.trim() || createBusy.value) return;
  createBusy.value = true;
  try {
    created.value = await api.createKey(newName.value.trim());
    createOpen.value = false;
    await load();
  } catch (e) { error.value = e instanceof Error ? e.message : ""; } finally { createBusy.value = false; }
}

/// The MCP endpoint an agent connects to (the SSE lane).
function mcpEndpoint(): string {
  return `${window.location.origin}/api/mcp/sse`;
}

/// Copy one of the copy-once card's payloads. Success is echoed on the
/// button that copied; a FAILURE is never silent (a denied clipboard
/// permission used to leave the button unchanged).
async function copyText(target: CopyTarget, text: string) {
  copyFailed.value = false;
  try {
    await navigator.clipboard.writeText(text);
    copied.value = target;
    setTimeout(() => { if (copied.value === target) copied.value = null; }, 3000);
  } catch {
    copied.value = null;
    copyFailed.value = true;
  }
}

function copyKey() {
  if (created.value) void copyText("key", created.value.key);
}

function copyConfig() {
  void copyText("config", t("common.keys.configTemplate", { endpoint: mcpEndpoint(), key: created.value?.key ?? "" }));
}

function copyPrompt() {
  void copyText("prompt", t("common.keys.promptTemplate", { endpoint: mcpEndpoint(), key: created.value?.key ?? "" }));
}

async function doRevoke() {
  if (!pendingRevoke.value || revokeBusy.value) return;
  revokeBusy.value = true;
  try { await api.revokeKey(pendingRevoke.value.id); pendingRevoke.value = null; void load(); }
  catch (e) { error.value = e instanceof Error ? e.message : ""; } finally { revokeBusy.value = false; }
}
</script>

<template>
  <section class="keys-page">
    <p v-if="error" role="alert" class="err">{{ error }}</p>
    <HkEmptyState v-if="!loading && keys.length === 0 && !created" boxed :icon="KeyRound" :title="t('common.keys.empty')" />

    <!-- Copy-once card: shown immediately after creation -->
    <div v-if="created" class="copy-once" data-testid="copy-once-card">
      <p class="warning">{{ t("common.keys.copyOnceWarning") }}</p>
      <code class="key-text" data-testid="plaintext-key">{{ created.key }}</code>
      <div class="actions">
        <HkButton variant="primary" data-testid="copy-key" @click="copyKey">
          <template #icon><Copy :size="14" /></template>
          {{ copied === "key" ? t("common.keys.copied") : t("common.actions.copyLink") }}
        </HkButton>
        <HkButton variant="ghost" data-testid="copy-config" @click="copyConfig">
          {{ copied === "config" ? t("common.keys.copied") : t("common.keys.copyConfig") }}
        </HkButton>
        <HkButton variant="ghost" data-testid="copy-prompt" @click="copyPrompt">
          {{ copied === "prompt" ? t("common.keys.copied") : t("common.keys.copyPrompt") }}
        </HkButton>
      </div>
      <p v-if="copyFailed" role="alert" data-testid="copy-failed" class="copy-failed">{{ t("common.keys.copyFailed") }}</p>
    </div>

    <ul v-if="keys.length > 0" class="key-list">
      <li v-for="k in keys" :key="k.id" class="key-row" :class="{ revoked: !!k.revoked_at }">
        <KeyRound :size="16" class="icon" />
        <span class="name">{{ k.name }}</span>
        <code class="prefix">{{ k.key_prefix }}</code>
        <span class="meta">{{ k.revoked_at ? t("common.keys.revoked") : "" }}</span>
        <button v-if="!k.revoked_at" type="button" class="revoke" :aria-label="t('common.keys.revoke')" @click="pendingRevoke = k">
          <Trash2 :size="14" />
        </button>
      </li>
    </ul>

    <HkAdaptiveDialog :model-value="createOpen" :title="t('common.keys.create')" width="26rem" @update:model-value="(v: boolean) => (createOpen = v)">
      <div class="dialog-body">
        <HkInput v-model="newName" :placeholder="t('common.keys.nameLabel')" data-testid="key-name" />
        <HkButton variant="primary" data-testid="new-key" :disabled="!newName.trim() || createBusy" @click="doCreate">{{ t("common.actions.create") }}</HkButton>
      </div>
    </HkAdaptiveDialog>

    <HkAdaptiveDialog :model-value="pendingRevoke !== null" :title="t('common.keys.revoke')" width="26rem" @update:model-value="(v: boolean) => !v && (pendingRevoke = null)">
      <p>{{ t("common.keys.revokeConfirm", { name: pendingRevoke?.name ?? "" }) }}</p>
      <div class="dialog-body">
        <HkButton variant="danger" :disabled="revokeBusy" @click="doRevoke">{{ t("common.keys.revoke") }}</HkButton>
        <HkButton variant="secondary" @click="pendingRevoke = null">{{ t("common.actions.cancel") }}</HkButton>
      </div>
    </HkAdaptiveDialog>
  </section>
</template>

<style scoped>
.keys-page { display: flex; flex-direction: column; gap: 1rem; }
.err { color: rgb(220 38 38); }
.copy-once { padding: 1rem; border: 2px solid rgb(217 138 43); border-radius: 0.75rem; display: flex; flex-direction: column; gap: 0.75rem; }
.warning { color: rgb(217 138 43); font-size: 0.85rem; margin: 0; }
.copy-failed { color: rgb(220 38 38); font-size: 0.8rem; margin: 0; }
.key-text { font-size: 0.9rem; padding: 0.5rem; border-radius: 0.4rem; background: color-mix(in srgb, currentColor 6%, transparent); word-break: break-all; }
.actions { display: flex; gap: 0.5rem; flex-wrap: wrap; }
.key-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.5rem; }
.key-row { display: flex; align-items: center; gap: 0.75rem; padding: 0.5rem 0.75rem; border: 1px solid color-mix(in srgb, currentColor 16%, transparent); border-radius: 0.6rem; }
.key-row.revoked { opacity: 0.5; }
.icon { opacity: 0.5; }
.name { font-weight: 600; font-size: 0.85rem; flex: 1; }
.prefix { font-size: 0.78rem; opacity: 0.6; font-family: monospace; }
.meta { font-size: 0.75rem; color: rgb(220 38 38); }
.revoke { border: none; background: transparent; color: inherit; opacity: 0.5; cursor: pointer; padding: 0.25rem; border-radius: 0.35rem; }
.revoke:hover { opacity: 1; color: rgb(220 38 38); }
.dialog-body { display: flex; flex-direction: column; gap: 0.75rem; }
</style>
