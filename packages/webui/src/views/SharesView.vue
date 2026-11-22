<script setup lang="ts">
import { HkEmptyState, HkButton } from "@celestia-island/hikari";
import { Link2Off } from "lucide-vue-next";
import { onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import * as files from "@/api/files";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { ShareDto } from "@/types";

const { t } = useI18n();
const shares = ref<ShareDto[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);

async function load() {
  loading.value = true;
  try { shares.value = await files.shareList(); } catch (e) { error.value = e instanceof Error ? e.message : ""; } finally { loading.value = false; }
}
onMounted(() => void load());
usePageRefresh(() => load(), { loading });

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
    <p v-if="error" role="alert" class="err">{{ error }}</p>
    <p v-if="revoked" role="status" class="ok">{{ t("common.files.revoked") }}</p>
    <HkEmptyState v-if="!loading && shares.length === 0" boxed :icon="Link2Off" :title="t('common.files.noShares')" />
    <ul v-else class="share-list">
      <li v-for="s in shares" :key="s.token" class="share-row">
        <span class="name">{{ s.file_name }}</span>
        <span class="size">{{ s.file_size }} B</span>
        <a :href="`/s/${s.token}`" class="link">/s/{{ s.token.slice(0, 12) }}…</a>
        <HkButton variant="danger" size="sm" @click="revoke(s.token)">{{ t("common.files.revoke") }}</HkButton>
      </li>
    </ul>
  </section>
</template>

<style scoped>
.share-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.5rem; }
.share-row { display: flex; align-items: center; gap: 0.75rem; padding: 0.5rem 0.75rem; border: 1px solid color-mix(in srgb, currentColor 16%, transparent); border-radius: 0.6rem; }
.name { flex: 1; font-size: 0.85rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.size { font-size: 0.75rem; opacity: 0.6; }
.link { font-size: 0.75rem; opacity: 0.7; color: inherit; }
.err { color: rgb(220 38 38); }
.ok { color: rgb(22 128 61); font-size: 0.85rem; margin: 0; }
</style>
