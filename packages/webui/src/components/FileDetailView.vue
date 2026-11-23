<script setup lang="ts">
import { HkBadge } from "@celestia-island/hikari";
import { FileAudio, FileText, FileVideo, Folder, ImageIcon } from "lucide-vue-next";
import { computed } from "vue";
import { useI18n } from "vue-i18n";

import type { FileDto } from "@/types";

/**
 * The embedded file detail — the pane every list surface (files, portal,
 * timeline) shows beside its roster when a row is picked. Render-only:
 * no selection state of its own (the pane's open/close is the host's),
 * no actions of its own (hosts stamp theirs through the actions slot).
 */

const props = defineProps<{ file: FileDto }>();
const { t } = useI18n();

const kindIcon = computed(() => {
  if (props.file.kind === "folder") return Folder;
  if (props.file.mime.startsWith("audio/")) return FileAudio;
  if (props.file.mime.startsWith("video/")) return FileVideo;
  if (props.file.mime.startsWith("text/")) return FileText;
  return ImageIcon;
});

function formatSize(bytes: bigint | number): string {
  const n = typeof bytes === "bigint" ? Number(bytes) : bytes;
  if (!Number.isFinite(n) || n < 0) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = n;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

function formatWhen(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}
</script>

<template>
  <div class="file-detail">
    <div class="preview">
      <img
        v-if="file.kind === 'image'"
        :src="file.url"
        :alt="file.original_name || file.name"
        loading="lazy"
      />
      <component :is="kindIcon" v-else :size="64" class="kind-icon" />
    </div>

    <div class="meta">
      <p class="name" :title="file.original_name || file.name">{{ file.original_name || file.name }}</p>
      <div class="rows">
        <span class="row">
          <span class="label">{{ t("common.files.size") }}</span>
          <span v-if="file.kind !== 'folder'" class="mono">{{ formatSize(file.size) }}</span>
          <span v-else>—</span>
        </span>
        <span class="row">
          <span class="label">{{ t("common.files.uploaded") }}</span>
          <span>{{ formatWhen(file.created_at) }}</span>
        </span>
        <span class="row">
          <span class="label">{{ t("common.files.kind") }}</span>
          <HkBadge v-if="file.kind === 'folder'" variant="info" pill>{{ t("common.files.kindFolder") }}</HkBadge>
          <HkBadge v-else variant="muted" pill mono>{{ file.mime }}</HkBadge>
        </span>
      </div>
    </div>
  </div>
</template>

<style scoped lang="scss">
.file-detail {
  display: flex;
  flex-direction: column;
  gap: 1rem;
}

.preview {
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 0.75rem;
  background: color-mix(in srgb, currentColor 5%, transparent);
  overflow: hidden;

  img {
    display: block;
    max-width: 100%;
    max-height: 18rem;
    object-fit: contain;
  }

  .kind-icon {
    opacity: 0.5;
    padding: 2rem;
  }
}

.name {
  margin: 0;
  font-weight: 600;
  word-break: break-all;
}

.rows {
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
  font-size: 0.875rem;
}

.row {
  display: flex;
  justify-content: space-between;
  gap: 1rem;
}

.label {
  opacity: 0.6;
}

.mono {
  font-family: var(--hk-font-mono, ui-monospace, monospace);
}
</style>