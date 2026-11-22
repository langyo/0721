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

const { t } = useI18n();
const shares = ref<ShareDto[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);

/** Failure text for the roster banner: never empty. A JSON-RPC/WS lane
 * commonly rejects with a bare string, and an empty message renders no
 * alert at all (the scaffold's message is falsy-checked), which would
 * show a silent "no rows yet" over a real failure. */
function errorText(e: unknown): string {
  if (e instanceof Error && e.message) return e.message;
  if (typeof e === "string" && e.length > 0) return e;
  return t("common.status.error");
}

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
  try {
    await files.shareDelete(token);
    revoked.value = true;
    void load();
  } catch { /* inline */ }
}
</script>

<template>
  <section>
    <p v-if="revoked" role="status" class="ok">{{ t("common.files.revoked") }}</p>
    <HkAdminTablePage
      :title="t('common.files.myShares')"
      :subtitle="t('common.status.total', { count: shares.length })"
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
