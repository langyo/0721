<script setup lang="ts">
import { HkEmptyState, HkIconButton, HkMasterDetail } from "@celestia-island/hikari";
import { ExternalLink, Images } from "lucide-vue-next";
import { onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import * as files from "@/api/files";
import FileDetailView from "@/components/FileDetailView.vue";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { FileDto } from "@/types";

const { t } = useI18n();
const items = ref<FileDto[]>([]);
const total = ref(0);
const loading = ref(false);
const PAGE = 40;

/** The pane's selection is THIS page's state — the master-detail
 * component hands out no selection of its own (its contract). */
const selected = ref<FileDto | null>(null);

async function load(reset: boolean) {
  loading.value = true;
  try {
    const page = await files.listTimeline(reset ? 0 : items.value.length, PAGE);
    items.value = reset ? page.items : [...items.value, ...page.items];
    total.value = page.total;
    if (reset && selected.value && !page.items.some((f) => f.id === selected.value?.id)) {
      selected.value = null;
    }
  } finally { loading.value = false; }
}
onMounted(() => void load(true));
usePageRefresh(() => load(true), { loading });

function openExternal(url: string): void {
  window.open(url, "_blank", "noopener");
}
</script>

<template>
  <section>
    <HkEmptyState v-if="loading && items.length === 0" loading boxed :title="t('common.status.loading')" />
    <HkEmptyState v-else-if="items.length === 0" boxed :icon="Images" :title="t('common.status.empty')" />

    <HkMasterDetail
      v-else
      :detail-open="selected !== null"
      :detail-title="selected?.original_name || selected?.name || ''"
      :empty-title="t('common.files.details')"
      :empty-description="t('common.files.pickHint')"
      :close-label="t('common.actions.cancel')"
      @close="selected = null"
    >
      <template #list>
        <p class="count">{{ t("common.status.total", { count: total }) }}</p>
        <ul class="grid">
          <!-- The tile picks the row; the pane's actions carry the
               open-in-new-tab the tile's old anchor used to do. -->
          <li v-for="f in items" :key="f.id" class="tile" :class="{ active: selected?.id === f.id }">
            <button type="button" class="tile-btn" :aria-label="f.original_name || f.name" @click="selected = f">
              <img v-if="f.kind === 'image'" :src="`${f.url}?thumbnail=true`" :alt="f.original_name || f.name" loading="lazy" />
            </button>
          </li>
        </ul>
        <div v-if="items.length < total" class="more">
          <HkIconButton :size="28" variant="ghost" :disabled="loading" :aria-label="t('common.actions.loadMore')" @click="load(false)">
            <template #icon><Images :size="16" /></template>
          </HkIconButton>
        </div>
      </template>

      <template #detail>
        <FileDetailView v-if="selected" :file="selected" />
      </template>

      <template v-if="selected" #actions>
        <HkIconButton
          :size="28"
          variant="ghost"
          :aria-label="t('common.portal.openOriginal')"
          :title="t('common.portal.openOriginal')"
          @click="openExternal(selected.url)"
        >
          <template #icon><ExternalLink :size="16" /></template>
        </HkIconButton>
      </template>
    </HkMasterDetail>
  </section>
</template>

<style scoped>
.count { margin: 0 0 0.5rem; opacity: 0.7; font-size: 0.85rem; }
.grid { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(auto-fill, minmax(10rem, 1fr)); gap: 0.6rem; }
.tile-btn { display: block; width: 100%; padding: 0; border: 2px solid transparent; border-radius: 0.6rem; background: none; cursor: pointer; overflow: hidden; }
.tile-btn img { display: block; width: 100%; aspect-ratio: 1; object-fit: cover; }
.tile.active .tile-btn { border-color: color-mix(in srgb, currentColor 45%, transparent); }
.more { display: flex; justify-content: center; margin-top: 1rem; }
</style>