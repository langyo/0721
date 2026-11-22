<script setup lang="ts">
import { HkEmptyState, HkButton } from "@celestia-island/hikari";
import { Images } from "lucide-vue-next";
import { onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import * as files from "@/api/files";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { FileDto } from "@/types";

const { t } = useI18n();
const items = ref<FileDto[]>([]);
const total = ref(0);
const loading = ref(false);
const PAGE = 40;

async function load(reset: boolean) {
  loading.value = true;
  try {
    const page = await files.listTimeline(reset ? 0 : items.value.length, PAGE);
    items.value = reset ? page.items : [...items.value, ...page.items];
    total.value = page.total;
  } finally { loading.value = false; }
}
onMounted(() => void load(true));
usePageRefresh(() => load(true), { loading });
</script>

<template>
  <section>
    <HkEmptyState v-if="loading && items.length === 0" loading boxed :title="t('common.status.loading')" />
    <HkEmptyState v-else-if="items.length === 0" boxed :icon="Images" :title="t('common.status.empty')" />
    <template v-else>
      <p class="count">{{ t("common.status.total", { count: total }) }}</p>
      <ul class="grid">
        <li v-for="f in items" :key="f.id" class="tile">
          <a :href="f.url" target="_blank" rel="noopener">
            <img v-if="f.kind === 'image'" :src="`${f.url}?thumbnail=true`" :alt="f.original_name || f.name" loading="lazy" />
          </a>
        </li>
      </ul>
      <div v-if="items.length < total" class="more">
        <HkButton variant="ghost" :disabled="loading" @click="load(false)">{{ t("common.actions.loadMore") }}</HkButton>
      </div>
    </template>
  </section>
</template>

<style scoped>
.count { margin: 0 0 0.5rem; opacity: 0.7; font-size: 0.85rem; }
.grid { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(auto-fill, minmax(10rem, 1fr)); gap: 0.6rem; }
.tile a { display: block; aspect-ratio: 1; border-radius: 0.6rem; overflow: hidden; }
.tile img { width: 100%; height: 100%; object-fit: cover; display: block; }
.more { display: flex; justify-content: center; margin-top: 1rem; }
</style>
