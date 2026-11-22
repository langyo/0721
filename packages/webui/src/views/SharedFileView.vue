<script setup lang="ts">
import { HkButton, HkEmptyState } from "@celestia-island/hikari";
import { Download } from "lucide-vue-next";
import { onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute } from "vue-router";
import * as files from "@/api/files";

const { t } = useI18n();
const route = useRoute();
const meta = ref<files.ShareMeta | null>(null);
const error = ref(false);

onMounted(() => {
  const token = route.params.token as string;
  files.shareMeta(token).then((m) => { meta.value = m; }).catch(() => { error.value = true; });
});

const downloadUrl = ref("");
function download() { window.location.href = downloadUrl.value; }
</script>

<template>
  <div class="shared-page">
    <HkEmptyState v-if="error" boxed :icon="undefined" :title="t('common.files.fileNotAvailable')" />
    <div v-else-if="meta" class="shared-card">
      <h2>{{ meta.file_name }}</h2>
      <p v-if="meta.allow_preview" class="preview-label">{{ t("common.files.preview") }}</p>
      <p class="meta">{{ meta.file_size }} B · {{ meta.kind }}</p>
      <!-- Inline preview: image / text / pdf / media -->
      <img v-if="meta.allow_preview && meta.mime.startsWith('image/')" :src="downloadUrl" :alt="meta.file_name" class="preview-img" />
      <video v-else-if="meta.allow_preview && meta.mime.startsWith('video/')" :src="downloadUrl" controls class="preview-media" />
      <audio v-else-if="meta.allow_preview && meta.mime.startsWith('audio/')" :src="downloadUrl" controls class="preview-audio" />
      <iframe v-else-if="meta.allow_preview && meta.mime === 'application/pdf'" :src="downloadUrl" class="preview-pdf" />
      <pre v-else-if="meta.allow_preview && (meta.mime.startsWith('text/') || meta.mime === 'application/json')" class="preview-text">Loading…</pre>
      <HkButton variant="primary" :aria-label="t('common.files.download')" @click="download()">
        <template #icon><Download :size="16" /></template>
        {{ t("common.files.download") }}
      </HkButton>
    </div>
  </div>
</template>

<style scoped>
.shared-page { min-height: 100vh; display: flex; align-items: center; justify-content: center; padding: 2rem; }
.shared-card { width: min(48rem, 100%); text-align: center; display: flex; flex-direction: column; gap: 1rem; align-items: center; }
.meta { opacity: 0.6; font-size: 0.85rem; }
.preview-label { font-size: 0.75rem; opacity: 0.5; margin: 0; }
.preview-img { max-width: 100%; max-height: 60vh; border-radius: 0.75rem; }
.preview-media { max-width: 100%; max-height: 60vh; }
.preview-pdf { width: 100%; height: 60vh; border: none; border-radius: 0.5rem; }
.preview-text { width: 100%; max-height: 60vh; overflow: auto; text-align: left; padding: 1rem; border-radius: 0.5rem; background: color-mix(in srgb, currentColor 5%, transparent); font-size: 0.85rem; }
</style>
