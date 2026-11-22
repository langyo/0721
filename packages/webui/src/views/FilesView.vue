<script setup lang="ts">
import {
  HkAdaptiveDialog,
  HkButton,
  HkEmptyState,
  HkIconButton,
  HkInput,
  HkSwitch,
  useChromeActionSize,
} from "@celestia-island/hikari";
import {
  Copy,
  FileArchive,
  FileAudio,
  FileText,
  FileVideo,
  Folder,
  FolderPlus,
  Images,
  Link2,
  MoreHorizontal,
  Move,
  Pencil,
  Trash2,
  Upload,
} from "lucide-vue-next";
import { computed, h, onMounted, onScopeDispose, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";

import * as files from "@/api/files";
import type { ChainEntry } from "@/api/files";
import { useActionBar } from "@/composables/useActionBar";
import { usePageCreateAction } from "@/composables/usePageCreateAction";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { FileDto } from "@/types";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();

const items = ref<FileDto[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);
const folderId = computed(() => {
  const id = route.params.folderId;
  return typeof id === "string" && id ? id : null;
});

// Quota display.
const quotaUsed = ref(0);
const quotaLimit = ref<number | null>(null);

async function load() {
  loading.value = true;
  error.value = null;
  try {
    items.value = (await files.listFolder(folderId.value)).items;
    const q = await files.quotaGet();
    quotaUsed.value = Number(q.used_bytes);
    quotaLimit.value = q.quota_bytes != null ? Number(q.quota_bytes) : null;
  } catch (e) {
    error.value = e instanceof Error ? e.message : t("common.status.error");
  } finally {
    loading.value = false;
  }
}

onMounted(() => void load());
watch(folderId, () => void load());
usePageRefresh(() => load(), { loading });

// Breadcrumb: root → … → current, via files.chain (deep-linkable).
const chain = ref<ChainEntry[]>([]);
watch(folderId, async (id) => {
  if (id) {
    try {
      chain.value = await files.folderChain(id);
    } catch { chain.value = []; }
  } else {
    chain.value = [];
  }
}, { immediate: true });

const crumbs = computed(() => {
  const trail: { name: string; to: string }[] = [
    { name: t("common.files.breadcrumbRoot"), to: "/files" },
  ];
  for (const entry of chain.value) {
    trail.push({ name: entry.name, to: `/files/${entry.id}` });
  }
  return trail;
});

function openFolder(f: FileDto) {
  if (f.kind === "folder") {
    void router.push(`/files/${f.id}`);
  }
}

function iconFor(f: FileDto) {
  if (f.kind === "folder") return Folder;
  if (f.mime.startsWith("audio/")) return FileAudio;
  if (f.mime.startsWith("video/")) return FileVideo;
  if (f.mime.startsWith("text/") || f.mime === "application/pdf" || f.mime === "application/json") return FileText;
  if (f.mime.includes("zip") || f.mime.includes("tar") || f.mime.includes("7z")) return FileArchive;
  return Images;
}

// ── upload ──
const fileInput = ref<HTMLInputElement | null>(null);
const uploadProgress = ref<Map<string, number>>(new Map());

async function onFilesPicked(e: Event) {
  const input = e.target as HTMLInputElement;
  const picked = Array.from(input.files ?? []);
  input.value = "";
  for (const f of picked) {
    const key = f.name;
    uploadProgress.value.set(key, 0);
    try {
      await files.uploadTo(f, folderId.value, (frac) => {
        uploadProgress.value.set(key, frac);
      });
      uploadProgress.value.delete(key);
    } catch {
      uploadProgress.value.delete(key);
    }
  }
  void load();
}

// ── new folder (Create slot) ──
const newFolderOpen = ref(false);
const newFolderName = ref("");
const newFolderBusy = ref(false);
usePageCreateAction({
  label: () => t("common.files.newFolder"),
  onCreate: () => {
    newFolderName.value = "";
    newFolderOpen.value = true;
  },
});

async function createFolder() {
  if (!newFolderName.value.trim() || newFolderBusy.value) return;
  newFolderBusy.value = true;
  try {
    await files.createFolder(newFolderName.value.trim(), folderId.value);
    newFolderOpen.value = false;
    void load();
  } catch (e) {
    error.value = e instanceof Error ? e.message : t("common.status.error");
  } finally {
    newFolderBusy.value = false;
  }
}

// ── rename ──
const renameTarget = ref<FileDto | null>(null);
const renameName = ref("");
const renameBusy = ref(false);

async function doRename() {
  if (!renameTarget.value || !renameName.value.trim()) return;
  renameBusy.value = true;
  try {
    await files.renameFile(renameTarget.value.id, renameName.value.trim());
    renameTarget.value = null;
    void load();
  } catch (e) {
    error.value = e instanceof Error ? e.message : t("common.status.error");
  } finally {
    renameBusy.value = false;
  }
}

// ── move ──
const moveTarget = ref<FileDto | null>(null);
const moveParent = ref<string>("");
const moveBusy = ref(false);

async function doMove() {
  if (!moveTarget.value || moveBusy.value) return;
  moveBusy.value = true;
  try {
    await files.moveFile(moveTarget.value.id, moveParent.value || null);
    moveTarget.value = null;
    void load();
  } catch (e) {
    error.value = e instanceof Error ? e.message : t("common.status.error");
  } finally {
    moveBusy.value = false;
  }
}

// ── share ──
const shareTarget = ref<FileDto | null>(null);
const sharePreview = ref(true);
const shareExpires = ref("");
const shareBusy = ref(false);
const shareLink = ref<string | null>(null);

const copyOk = ref<null | boolean>(null);
function copyShareLink() {
  if (shareLink.value) {
    copyOk.value = null;
    void navigator.clipboard.writeText(
      new URL(shareLink.value, window.location.origin).toString(),
    ).then(() => {
      copyOk.value = true;
      window.setTimeout(() => { copyOk.value = null; }, 3000);
    }).catch(() => { copyOk.value = false; });
  }
}

async function doShare() {
  if (!shareTarget.value || shareBusy.value) return;
  shareBusy.value = true;
  try {
    const share = await files.shareCreate(
      shareTarget.value.id,
      sharePreview.value,
      shareExpires.value || undefined,
    );
    shareLink.value = `/s/${share.token}`;
    copyOk.value = null;
    try {
      await navigator.clipboard.writeText(
        new URL(shareLink.value, window.location.origin).toString(),
      );
      copyOk.value = true;
      window.setTimeout(() => { copyOk.value = null; }, 3000);
    } catch {
      copyOk.value = false; // visible failure → manual copy prompt
    }
  } catch (e) {
    error.value = e instanceof Error ? e.message : t("common.status.error");
  } finally {
    shareBusy.value = false;
  }
}

// ── delete ──
const deleteTarget = ref<FileDto | null>(null);
const deleteBusy = ref(false);

async function doDelete() {
  if (!deleteTarget.value || deleteBusy.value) return;
  deleteBusy.value = true;
  try {
    await files.deleteFile(deleteTarget.value.id);
    deleteTarget.value = null;
    void load();
  } catch (e) {
    error.value = e instanceof Error ? e.message : t("common.status.error");
  } finally {
    deleteBusy.value = false;
  }
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KiB", "MiB", "GiB"];
  let v = bytes;
  let u = -1;
  do { v /= 1024; u++; } while (v >= 1024 && u < units.length - 1);
  return `${v.toFixed(v >= 10 ? 0 : 1)} ${units[u]}`;
}

const quotaPercent = computed(() => {
  if (!quotaLimit.value || quotaLimit.value === 0) return null;
  return Math.min(100, (quotaUsed.value / quotaLimit.value) * 100);
});

// The upload action (Refresh-adjacent custom slot — the Create slot is
// the new-folder plus; upload lives next to it per the page-chrome law).
const bar = useActionBar();
const uploadChromeSize = useChromeActionSize();
const uploadLabel = computed(() => t("common.files.uploadHere"));
bar.setActions(() => {
  return [h(HkIconButton, {
    size: uploadChromeSize.value, variant: "ghost",
    "aria-label": uploadLabel.value, title: uploadLabel.value,
    onClick: () => fileInput.value?.click(),
  }, { icon: () => h(Upload, { size: 16 }) })];
}, "custom-upload");
</script>

<template>
  <section class="files-page">
    <input ref="fileInput" type="file" multiple hidden @change="onFilesPicked" />

    <!-- Breadcrumb -->
    <nav class="breadcrumb" :aria-label="t('common.files.breadcrumbRoot')">
      <router-link v-for="crumb in crumbs" :key="crumb.to" :to="crumb.to" class="crumb">
        {{ crumb.name }}
      </router-link>
    </nav>

    <!-- Quota bar -->
    <div v-if="quotaLimit" class="quota-bar" :aria-label="t('common.files.quota')">
      <div class="quota-track">
        <div class="quota-fill" :style="{ width: `${quotaPercent ?? 0}%` }" />
      </div>
      <span class="quota-text">
        {{ quotaLimit ? t("common.files.quotaOf", {
          used: formatSize(quotaUsed),
          quota: formatSize(quotaLimit!),
        }) : t("common.files.unlimited") }}
      </span>
    </div>

    <p v-if="error" class="error" role="alert">{{ error }}</p>

    <HkEmptyState v-if="loading && items.length === 0" loading boxed :title="t('common.status.loading')" />
    <HkEmptyState v-else-if="items.length === 0" boxed :icon="Folder" :title="t('common.files.empty')" />

    <ul v-else class="file-grid">
      <li v-for="f in items" :key="f.id" class="file-tile" :class="{ folder: f.kind === 'folder' }">
        <button type="button" class="tile-body" @click="openFolder(f)" @dblclick="openFolder(f)">
          <component :is="iconFor(f)" v-if="f.kind !== 'image'" :size="32" class="tile-icon" />
          <img v-else :src="`${f.url}?thumbnail=true`" :alt="f.original_name || f.name" loading="lazy" />
        </button>
        <div class="tile-meta">
          <span class="tile-name" :title="f.original_name || f.name">{{ f.original_name || f.name }}</span>
          <span v-if="f.kind !== 'folder'" class="tile-size">{{ formatSize(f.size) }}</span>
        </div>
        <div class="tile-actions">
          <button type="button" class="t-action" :title="t('common.files.rename')" @click="((renameTarget = f), (renameName = f.name))">
            <Pencil :size="14" />
          </button>
          <button type="button" class="t-action" :title="t('common.files.move')" @click="((moveTarget = f), (moveParent = ''))">
            <Move :size="14" />
          </button>
          <button v-if="f.kind !== 'folder'" type="button" class="t-action" :title="t('common.files.share')" @click="((shareTarget = f), (shareLink = null))">
            <Link2 :size="14" />
          </button>
          <button type="button" class="t-action danger" :title="t('common.actions.delete')" @click="deleteTarget = f">
            <Trash2 :size="14" />
          </button>
        </div>
      </li>
    </ul>

    <!-- Upload progress overlay -->
    <div v-if="uploadProgress.size > 0" class="upload-overlay" role="status">
      <div v-for="[name, frac] in uploadProgress" :key="name" class="upload-row">
        <span>{{ name }}</span>
        <progress :value="frac" max="1" />
      </div>
    </div>

    <!-- New folder dialog -->
    <HkAdaptiveDialog :model-value="newFolderOpen" :title="t('common.files.newFolder')" width="26rem"
      @update:model-value="(v: boolean) => (newFolderOpen = v)">
      <div class="dialog-body">
        <HkInput v-model="newFolderName" :placeholder="t('common.files.folderName')" data-testid="new-folder-name" />
        <HkButton variant="primary" :disabled="!newFolderName.trim() || newFolderBusy" @click="createFolder">
          {{ t("common.actions.create") }}
        </HkButton>
      </div>
    </HkAdaptiveDialog>

    <!-- Rename dialog -->
    <HkAdaptiveDialog :model-value="renameTarget !== null" :title="t('common.files.rename')" width="26rem"
      @update:model-value="(v: boolean) => !v && (renameTarget = null)">
      <div class="dialog-body">
        <HkInput v-model="renameName" data-testid="rename-input" />
        <HkButton variant="primary" :disabled="!renameName.trim() || renameBusy" @click="doRename">
          {{ t("common.actions.save") }}
        </HkButton>
      </div>
    </HkAdaptiveDialog>

    <!-- Move dialog -->
    <HkAdaptiveDialog :model-value="moveTarget !== null" :title="t('common.files.move')" width="26rem"
      @update:model-value="(v: boolean) => !v && (moveTarget = null)">
      <div class="dialog-body">
        <label class="field-label">
          <span>{{ t("common.files.moveTarget") }}</span>
          <select v-model="moveParent" class="native-select">
            <option value="">{{ t("common.files.breadcrumbRoot") }}</option>
            <option v-for="f in items.filter((i) => i.kind === 'folder' && i.id !== moveTarget?.id)" :key="f.id" :value="f.id">
              {{ f.name }}
            </option>
          </select>
        </label>
        <HkButton variant="primary" :disabled="moveBusy" @click="doMove">
          {{ t("common.actions.confirm") }}
        </HkButton>
      </div>
    </HkAdaptiveDialog>

    <!-- Share dialog -->
    <HkAdaptiveDialog :model-value="shareTarget !== null" :title="t('common.files.share')" width="28rem"
      @update:model-value="(v: boolean) => !v && (shareTarget = null)">
      <div class="dialog-body">
        <label class="field-label">
          <span>{{ t("common.files.shareExpires") }}</span>
          <input v-model="shareExpires" type="datetime-local" class="native-input" />
        </label>
        <label class="switch-row">
          <span>{{ t("common.files.sharePreview") }}</span>
          <HkSwitch v-model="sharePreview" />
        </label>
        <HkButton variant="primary" :disabled="shareBusy" @click="doShare">
          {{ t("common.actions.confirm") }}
        </HkButton>
        <div v-if="shareLink" class="share-result">
          <span class="share-label">{{ t("common.files.shareLink") }}</span>
          <code>{{ shareLink }}</code>
          <span v-if="copyOk === true" class="share-copied">{{ t("common.files.shareCreated") }}</span>
          <span v-else-if="copyOk === false" class="share-failed">
            {{ t("common.actions.copyLink") }} ↓
          </span>
          <HkButton variant="ghost" @click="copyShareLink()">
            <template #icon><Copy :size="14" /></template>
            {{ t("common.actions.copyLink") }}
          </HkButton>
        </div>
      </div>
    </HkAdaptiveDialog>

    <!-- Delete confirm -->
    <HkAdaptiveDialog :model-value="deleteTarget !== null" :title="t('common.actions.delete')" width="28rem"
      @update:model-value="(v: boolean) => !v && (deleteTarget = null)">
      <p v-if="deleteTarget?.kind === 'folder'" class="warn-text">{{ t("common.files.folderDeleteWarning") }}</p>
      <div class="dialog-body">
        <HkButton variant="danger" :disabled="deleteBusy" @click="doDelete">
          {{ t("common.actions.delete") }}
        </HkButton>
        <HkButton variant="secondary" @click="deleteTarget = null">{{ t("common.actions.cancel") }}</HkButton>
      </div>
    </HkAdaptiveDialog>
  </section>
</template>

<style scoped lang="scss">
.files-page { display: flex; flex-direction: column; gap: 0.75rem; }
.breadcrumb { display: flex; gap: 0.5rem; font-size: 0.85rem; }
.crumb { color: inherit; text-decoration: none; opacity: 0.7; &:hover { opacity: 1; } }
.quota-bar { display: flex; align-items: center; gap: 0.75rem; }
.quota-track { flex: 1; height: 6px; border-radius: 3px; background: color-mix(in srgb, currentColor 12%, transparent); overflow: hidden; }
.quota-fill { height: 100%; border-radius: 3px; background: rgb(13 148 136); transition: width 0.3s; }
.quota-text { font-size: 0.78rem; opacity: 0.7; white-space: nowrap; }
.error { color: rgb(220 38 38); font-size: 0.85rem; margin: 0; }
.file-grid { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(auto-fill, minmax(9.5rem, 1fr)); gap: 0.75rem; }
.file-tile { display: flex; flex-direction: column; border: 1px solid color-mix(in srgb, currentColor 16%, transparent); border-radius: 0.75rem; overflow: hidden; background: color-mix(in srgb, currentColor 4%, transparent); }
.tile-body { display: flex; align-items: center; justify-content: center; aspect-ratio: 1; width: 100%; border: none; background: transparent; cursor: pointer; color: inherit; padding: 0;
  img { width: 100%; height: 100%; object-fit: cover; } }
.tile-icon { opacity: 0.5; }
.tile-meta { padding: 0.35rem 0.55rem; display: flex; flex-direction: column; min-width: 0; }
.tile-name { font-size: 0.78rem; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.tile-size { font-size: 0.7rem; opacity: 0.6; }
.tile-actions { display: flex; justify-content: flex-end; gap: 0.15rem; padding: 0 0.3rem 0.3rem; }
.t-action { display: inline-flex; border: none; background: transparent; color: inherit; opacity: 0.55; cursor: pointer; padding: 0.25rem; border-radius: 0.35rem;
  &:hover { opacity: 1; background: color-mix(in srgb, currentColor 10%, transparent); }
  &.danger:hover { background: color-mix(in srgb, rgb(220 38 38) 16%, transparent); color: rgb(220 38 38); } }
.upload-overlay { position: fixed; inset-inline-end: 1rem; inset-block-end: 4rem; display: flex; flex-direction: column; gap: 0.3rem; padding: 0.5rem 0.75rem; border-radius: 0.6rem; background: color-mix(in srgb, rgb(0 0 0 / 80%) 100%, transparent); color: white; font-size: 0.78rem; min-width: 14rem; }
.upload-row { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem;
  progress { width: 6rem; } }
.dialog-body { display: flex; flex-direction: column; gap: 0.75rem; }
.field-label { display: flex; flex-direction: column; gap: 0.3rem; font-size: 0.85rem; }
.switch-row { display: flex; align-items: center; justify-content: space-between; font-size: 0.85rem; }
.native-select, .native-input { padding: 0.45rem 0.6rem; border-radius: 0.5rem; border: 1px solid color-mix(in srgb, currentColor 22%, transparent); background: transparent; color: inherit; }
.share-result { display: flex; align-items: center; gap: 0.5rem; padding: 0.5rem; border-radius: 0.5rem; background: color-mix(in srgb, currentColor 6%, transparent);
  code { font-size: 0.75rem; word-break: break-all; flex: 1; } }
.warn-text { color: rgb(217 138 43); font-size: 0.85rem; margin: 0 0 0.5rem; }
</style>
