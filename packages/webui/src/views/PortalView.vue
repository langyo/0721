<script setup lang="ts">
import { HkAdaptiveDialog, HkEmptyState, HkIconButton, HkMasterDetail, HkProgressRing } from "@celestia-island/hikari";
import { ExternalLink, Copy, FileArchive, FileAudio, FileText, FileVideo, Folder, Images, Trash2 } from "lucide-vue-next";
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as api from "@/api";
import { usePageCreateAction } from "@/composables/usePageCreateAction";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { FileDto } from "@/types";
import { makeErrorText } from "@/composables/errorText";
import FileDetailView from "@/components/FileDetailView.vue";

/**
 * The timeline face: newest-first grid of everything on the instance,
 * multi-file upload with per-file progress, and the per-file inline
 * actions (copy link / open original / delete).
 *
 * The page's primary create action — the upload — lives in the header's
 * action cluster as the plus, LEFT of the refresh (the slot law); the
 * page body never carries its own upload button.
 */

const { t } = useI18n();

/** The pane's selection — this page's state, never the component's. */
const selected = ref<FileDto | null>(null);

function openExternal(url: string): void {
  window.open(url, "_blank", "noopener");
}

const errorText = makeErrorText(t);
const uploadErrorText = makeErrorText(t, "common.portal.uploadFailed");

const PAGE_SIZE = 30;

const files = ref<FileDto[]>([]);
const total = ref(0);
const loading = ref(false);
const loadError = ref<string | null>(null);

const fileInput = ref<HTMLInputElement | null>(null);

interface UploadEntry {
  id: number;
  name: string;
  progress: number;
  error: string | null;
}

const uploads = ref<UploadEntry[]>([]);
let uploadSeq = 0;

const hasMore = computed(() => files.value.length < total.value);

async function load(reset: boolean) {
  loading.value = true;
  loadError.value = null;
  try {
    const offset = reset ? 0 : files.value.length;
    const page = await api.listFiles(offset, PAGE_SIZE);
    files.value = reset ? page.items : [...files.value, ...page.items];
    // A reload can remove the row the pane is showing (deleted elsewhere).
    if (reset && selected.value && !files.value.some((f) => f.id === selected.value?.id)) {
      selected.value = null;
    }
    total.value = page.total;
  } catch (err) {
    loadError.value = errorText(err);
  } finally {
    loading.value = false;
  }
}

onMounted(() => {
  void load(true);
});

usePageRefresh(() => load(true), { loading });

usePageCreateAction({
  label: () => t("common.actions.upload"),
  onCreate: () => fileInput.value?.click(),
});

function onFilesPicked(event: Event) {
  const input = event.target as HTMLInputElement;
  const picked = Array.from(input.files ?? []);
  input.value = "";
  for (const file of picked) {
    const entry: UploadEntry = { id: uploadSeq++, name: file.name, progress: 0, error: null };
    uploads.value = [...uploads.value, entry];
    void api
      .uploadFile(file, (fraction) => {
        entry.progress = fraction;
        uploads.value = [...uploads.value];
      })
      .then(() => {
        uploads.value = uploads.value.filter((u) => u.id !== entry.id);
        void load(true);
      })
      .catch((err: unknown) => {
        entry.error = uploadErrorText(err);
        uploads.value = [...uploads.value];
      });
  }
}

// ── per-tile actions ───────────────────────────────────────────────────

const copiedId = ref<string | null>(null);
const pendingDelete = ref<FileDto | null>(null);
const deleteBusy = ref(false);
const deleteError = ref<string | null>(null);

const deleteActions = computed(() => [
  {
    label: t("common.actions.cancel"),
    variant: "secondary" as const,
    onClick: () => {
      pendingDelete.value = null;
    },
  },
  {
    label: t("common.actions.delete"),
    variant: "danger" as const,
    loading: deleteBusy.value,
    onClick: () => {
      void confirmDelete();
    },
  },
]);

async function confirmDelete() {
  const target = pendingDelete.value;
  if (!target) return;
  deleteBusy.value = true;
  deleteError.value = null;
  try {
    await api.deleteFile(target.id);
    // The pane must not keep showing the row that just vanished.
    if (selected.value?.id === target.id) selected.value = null;
    files.value = files.value.filter((f) => f.id !== target.id);
    total.value = Math.max(0, total.value - 1);
    pendingDelete.value = null;
  } catch (err) {
    deleteError.value = errorText(err);
  } finally {
    deleteBusy.value = false;
  }
}

function absoluteUrl(file: FileDto): string {
  return new URL(file.url, window.location.origin).toString();
}

async function copyLink(file: FileDto) {
  try {
    await navigator.clipboard.writeText(absoluteUrl(file));
    copiedId.value = file.id;
    window.setTimeout(() => {
      if (copiedId.value === file.id) copiedId.value = null;
    }, 1600);
  } catch {
    // Clipboard access denied (insecure context): the original link is
    // still reachable through the tile's open action.
  }
}

function iconFor(file: FileDto) {
  switch (file.kind) {
    case "image":
      return Images;
    case "document":
      return FileText;
    case "media":
      return file.mime.startsWith("audio/") ? FileAudio : FileVideo;
    case "archive":
      return FileArchive;
    default:
      return Folder;
  }
}

/** See FilesView's twin: the wire type is a bigint, a JSON-decoded value
 * is a number, and mixing them in arithmetic throws. */
function formatSize(bytes: bigint | number): string {
  const n = typeof bytes === "bigint" ? Number(bytes) : bytes;
  if (!Number.isFinite(n) || n < 0) return "—";
  if (n < 1024) return `${n} B`;
  const units = ["KiB", "MiB", "GiB"];
  let value = n;
  let unit = -1;
  do {
    value /= 1024;
    unit += 1;
  } while (value >= 1024 && unit < units.length - 1);
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[unit]}`;
}
</script>

<template>
  <section class="portal">
    <!-- The upload picker: hidden input driven by the action-bar plus. -->
    <input ref="fileInput" type="file" multiple hidden data-testid="upload-input" @change="onFilesPicked" />

    <HkEmptyState v-if="loading && files.length === 0" loading boxed :title="t('common.status.loading')" />
    <HkEmptyState
      v-else-if="loadError"
      boxed
      :title="t('common.status.error')"
      :description="loadError"
    />
    <HkEmptyState
      v-else-if="files.length === 0"
      boxed
      :icon="Images"
      :title="t('common.status.empty')"
      :description="t('common.portal.empty')"
    />

    <template v-else>
      <p class="portal-count">{{ t("common.status.total", { count: total }) }}</p>
      <HkMasterDetail
        :detail-open="selected !== null"
        :detail-title="selected?.original_name || selected?.name || ''"
        :empty-title="t('common.files.details')"
        :empty-description="t('common.files.pickHint')"
        :close-label="t('common.actions.cancel')"
        @close="selected = null"
      >
        <template #list>
      <ul class="portal-grid">
        <li v-for="file in files" :key="file.id" class="portal-tile">
          <button
            type="button"
            class="tile-media"
            :class="{ picked: selected?.id === file.id }"
            :aria-label="file.original_name || file.name"
            @click="selected = file"
          >
            <img
              v-if="file.kind === 'image'"
              :src="`${file.url}?thumbnail=true`"
              :alt="file.original_name || file.name"
              loading="lazy"
            />
            <component :is="iconFor(file)" v-else :size="32" class="tile-icon" />
          </button>
          <div class="tile-meta">
            <span class="tile-name" :title="file.original_name || file.name">
              {{ file.original_name || file.name }}
            </span>
            <span class="tile-size">{{ formatSize(file.size) }}</span>
          </div>
          <div class="tile-actions">
            <button
              type="button"
              class="tile-action"
              :aria-label="t('common.actions.copyLink')"
              :title="t('common.actions.copyLink')"
              @click="copyLink(file)"
            >
              <Copy v-if="copiedId !== file.id" :size="14" />
              <span v-else class="tile-copied">{{ t("common.actions.copied") }}</span>
            </button>
            <button
              type="button"
              class="tile-action danger"
              :aria-label="t('common.actions.delete')"
              :title="t('common.actions.delete')"
              @click="((pendingDelete = file), (deleteError = null))"
            >
              <Trash2 :size="14" />
            </button>
          </div>
        </li>
      </ul>
      <div v-if="hasMore" class="portal-more">
        <button type="button" class="portal-more-btn" :disabled="loading" @click="load(false)">
          {{ t("common.actions.loadMore") }}
        </button>
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
    </template>

    <!-- The upload lane: one row per in-flight file with its progress. -->
    <div
      v-if="uploads.length > 0"
      class="upload-lane"
      role="status"
      aria-live="polite"
      :aria-label="t('common.portal.uploading', { name: uploads[0]!.name })"
    >
      <div v-for="entry in uploads" :key="entry.id" class="upload-row">
        <HkProgressRing :value="entry.progress" :size="20" :stroke-width="3" />
        <span class="upload-name" :title="entry.name">{{ entry.name }}</span>
        <span v-if="entry.error" class="upload-error" role="alert">
          {{ entry.error }}
        </span>
      </div>
    </div>

    <HkAdaptiveDialog
      :model-value="pendingDelete !== null"
      :title="t('common.actions.delete')"
      :footer-actions="deleteActions"
      width="28rem"
      @update:model-value="(v: boolean) => !v && ((pendingDelete = null))"
    >
      <p>{{ t("common.portal.deleteConfirm") }}</p>
      <p v-if="deleteError" class="dialog-error" role="alert">{{ deleteError }}</p>
    </HkAdaptiveDialog>
  </section>
</template>

<style scoped lang="scss">
.portal {
  display: flex;
  flex-direction: column;
  gap: 1rem;
}

.portal-count {
  margin: 0;
  opacity: 0.75;
  font-size: 0.875rem;
}

.portal-grid {
  list-style: none;
  margin: 0;
  padding: 0;
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(10.5rem, 1fr));
  gap: 0.75rem;
}

.portal-tile {
  display: flex;
  flex-direction: column;
  border: 1px solid color-mix(in srgb, currentColor 18%, transparent);
  border-radius: 0.75rem;
  overflow: hidden;
  background: color-mix(in srgb, currentColor 4%, transparent);
}

.tile-media.picked {
  outline: 2px solid color-mix(in srgb, currentColor 45%, transparent);
  outline-offset: -2px;
}
.tile-media {
  display: flex;
  align-items: center;
  justify-content: center;
  aspect-ratio: 1;
  width: 100%;
  color: inherit;

  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
}

.tile-icon {
  opacity: 0.55;
}

.tile-meta {
  display: flex;
  flex-direction: column;
  gap: 0.15rem;
  padding: 0.4rem 0.6rem;
  min-width: 0;
}

.tile-name {
  font-size: 0.8rem;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tile-size {
  font-size: 0.72rem;
  opacity: 0.6;
}

.tile-actions {
  display: flex;
  justify-content: flex-end;
  gap: 0.25rem;
  padding: 0 0.4rem 0.4rem;
}

.tile-action {
  display: inline-flex;
  align-items: center;
  gap: 0.25rem;
  border: none;
  background: transparent;
  color: inherit;
  opacity: 0.7;
  cursor: pointer;
  padding: 0.3rem;
  border-radius: 0.4rem;

  &:hover {
    opacity: 1;
    background: color-mix(in srgb, currentColor 10%, transparent);
  }

  &.danger:hover {
    background: color-mix(in srgb, rgb(220 38 38) 16%, transparent);
    color: rgb(220 38 38);
  }
}

.tile-copied {
  font-size: 0.72rem;
}

.portal-more {
  display: flex;
  justify-content: center;
}

.portal-more-btn {
  border: 1px solid color-mix(in srgb, currentColor 22%, transparent);
  background: transparent;
  color: inherit;
  padding: 0.45rem 1.4rem;
  border-radius: 999px;
  cursor: pointer;

  &:hover:not(:disabled) {
    background: color-mix(in srgb, currentColor 8%, transparent);
  }

  &:disabled {
    opacity: 0.5;
    cursor: default;
  }
}

.upload-lane {
  position: fixed;
  inset-inline-end: 1rem;
  inset-block-end: 1rem;
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
  min-width: 16rem;
  padding: 0.75rem;
  border-radius: 0.9rem;
  border: 1px solid color-mix(in srgb, currentColor 18%, transparent);
  background: color-mix(in srgb, rgb(255 255 255 / 92%) 100%, transparent);
  color: #1f2937;
  box-shadow: 0 8px 30px rgb(0 0 0 / 18%);
}

:global([data-mode="dark"]) .upload-lane {
  background: color-mix(in srgb, rgb(17 24 39 / 94%) 100%, transparent);
  color: #e5e7eb;
}

.upload-row {
  display: flex;
  align-items: center;
  gap: 0.6rem;
  min-width: 0;
}

.upload-name {
  font-size: 0.8rem;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.upload-error {
  font-size: 0.72rem;
  color: rgb(220 38 38);
  margin-inline-start: auto;
}

.dialog-error {
  color: rgb(220 38 38);
  font-size: 0.85rem;
}
</style>
