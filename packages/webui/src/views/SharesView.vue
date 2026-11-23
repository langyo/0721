<script setup lang="ts">
import {
  HkAdminTablePage,
  HkIconButton,
  type HTableColumn,
} from "@celestia-island/hikari";
import { Link2, Link2Off } from "lucide-vue-next";
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import * as files from "@/api/files";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { ShareDto } from "@/types";
import { makeErrorText } from "@/composables/errorText";

const { t } = useI18n();
const shares = ref<ShareDto[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);

// The never-empty failure text (shared factory; the contract and
// its pins live in composables/errorText).
const errorText = makeErrorText(t);

async function load() {
  error.value = null;
  loading.value = true;
  try { shares.value = await files.shareList(); } catch (e) { error.value = errorText(e); } finally { loading.value = false; }
}
onMounted(() => void load());
usePageRefresh(() => load(), { loading });

/** The roster's column set (the family table anatomy); `key` doubles as
 * the cell-slot name. The token column is copy-exempt (its rendered value
 * is truncated — the full token rides the link href). */
const columns = computed<HTableColumn[]>(() => [
  { key: "fileName", title: t("common.files.fileName") },
  { key: "fileSize", title: t("common.files.size"), align: "right" },
  { key: "token", title: t("common.files.shareLink"), copyable: false },
  { key: "actions", title: "", align: "right", width: "3.5rem", copyable: false },
]);

const revoked = ref(false);
async function revoke(token: string) {
  // Each attempt starts from a clean slate: a stale "revoked" note over
  // a failed revoke claims a success that did not happen.
  revoked.value = false;
  try {
    await files.shareDelete(token);
    revoked.value = true;
    void load();
  } catch (e) {
    // The roster's banner is the failure lane — a silent catch leaves
    // the row on screen with no explanation and no retry.
    error.value = errorText(e);
  }
}
</script>

<template>
  <section>
    <p v-if="revoked" role="status" class="ok">{{ t("common.files.revoked") }}</p>
    <HkAdminTablePage
      :title="t('common.files.myShares')"
      :subtitle="t('common.status.total', { count: shares.length })"
      :count-readout="false"
      :icon="Link2"
      :rows="shares"
      :columns="columns"
      row-key="token"
      :loading="loading"
      :error="shares.length === 0 ? (error ?? undefined) : undefined"
      :stale-error="shares.length > 0 ? (error ?? undefined) : undefined"
      :empty-title="t('common.files.noShares')"
    >
      <template #cell-fileName="{ row }">
        <span class="share-name">{{ row.file_name }}</span>
      </template>

      <template #cell-fileSize="{ row }">
        <span class="share-size">{{ row.file_size }} B</span>
      </template>

      <template #cell-token="{ row }">
        <a :href="`/s/${row.token}`" class="share-link">/s/{{ row.token.slice(0, 12) }}…</a>
      </template>

      <template #cell-actions="{ row }">
        <HkIconButton
          :size="24"
          variant="ghost"
          :aria-label="t('common.files.revoke')"
          :title="t('common.files.revoke')"
          @click="revoke(row.token)"
        >
          <template #icon><Link2Off :size="14" /></template>
        </HkIconButton>
      </template>
    </HkAdminTablePage>
  </section>
</template>

<style scoped>
.ok { color: rgb(22 128 61); font-size: 0.85rem; margin: 0; }
.share-link { font-family: var(--hk-font-mono, ui-monospace, monospace); }
</style>
