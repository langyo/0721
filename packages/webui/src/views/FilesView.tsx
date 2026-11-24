import {
  HkAdaptiveDialog,
  HkButton,
  HkEmptyState,
  HkIconButton,
  HkInput,
  HkMasterDetail,
  HkSelect,
  HkSwitch,
  useChromeActionSize,
  useToast,
} from "@celestia-island/hikari";
import {
  Copy,
  FileArchive,
  FileAudio,
  FileText,
  FileVideo,
  Folder,
  Images,
  Link2,
  Move,
  Pencil,
  Trash2,
  Upload,
} from "lucide-vue-next";
import { computed, defineComponent, h, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { RouterLink, useRoute, useRouter } from "vue-router";

import * as files from "@/api/files";
import type { ChainEntry } from "@/api/files";
import { useActionBar } from "@/composables/useActionBar";
import { usePageCreateAction } from "@/composables/usePageCreateAction";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { FileDto } from "@/types";
import { makeErrorText } from "@/composables/errorText";
import FileDetailView from "@/components/FileDetailView";
import { formatSize } from "@/composables/formatSize";

import "./FilesView.css";

/**
 * The netdisk roster: breadcrumb + quota bar + the master/detail file
 * grid, with the upload, new-folder, rename, move, share and delete
 * affordances around it.
 *
 * Failure feedback is a TOAST (the family feedback contract): a failed
 * load/create/rename/move/share/delete raises one, and nothing inserts a
 * paragraph beside the roster. The share dialog's own result — the link
 * it just minted plus its copy state — stays where it is: that is the
 * dialog's content, not a transient notice.
 */
export default defineComponent({
  name: "FilesView",
  setup() {
    const { t } = useI18n();

    const errorText = makeErrorText(t);
    const toast = useToast();
    const route = useRoute();
    const router = useRouter();

    const items = ref<FileDto[]>([]);
    const loading = ref(false);
    const folderId = computed(() => {
      const id = route.params.folderId;
      return typeof id === "string" && id ? id : null;
    });

    // Quota display.
    const quotaUsed = ref(0);
    const quotaLimit = ref<number | null>(null);

    async function load() {
      loading.value = true;
      try {
        items.value = (await files.listFolder(folderId.value)).items;
        // The pane holds an OBJECT: a reload that drops its row (deleted
        // elsewhere, moved by another tab) must not keep rendering it.
        if (selected.value && !items.value.some((f) => f.id === selected.value?.id)) {
          selected.value = null;
        }
        const q = await files.quotaGet();
        quotaUsed.value = Number(q.used_bytes);
        quotaLimit.value = q.quota_bytes != null ? Number(q.quota_bytes) : null;
      } catch (e) {
        // Transient load feedback: a toast, never DOM beside the roster.
        toast.error(errorText(e));
      } finally {
        loading.value = false;
      }
    }

    onMounted(() => void load());
    watch(folderId, () => {
      // A new folder is a new roster; the pane must not keep the parent's row.
      selected.value = null;
      void load();
    });
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

    const selected = ref<FileDto | null>(null);

    function openFolder(f: FileDto) {
      if (f.kind === "folder") {
        void router.push(`/files/${f.id}`);
      } else {
        selected.value = f;
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
        toast.error(errorText(e));
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
      const renamedId = renameTarget.value.id;
      try {
        await files.renameFile(renamedId, renameName.value.trim());
        renameTarget.value = null;
        await load();
        // The pane holds an OBJECT, not an id: a reload replaces the array,
        // so re-point it or the detail keeps showing the pre-rename row.
        if (selected.value?.id === renamedId) {
          selected.value = items.value.find((f) => f.id === renamedId) ?? null;
        }
      } catch (e) {
        toast.error(errorText(e));
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
        const movedId = moveTarget.value.id;
        if (selected.value?.id === movedId) selected.value = null;
        await files.moveFile(movedId, moveParent.value || null);
        moveTarget.value = null;
        void load();
      } catch (e) {
        toast.error(errorText(e));
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
        toast.error(errorText(e));
      } finally {
        shareBusy.value = false;
      }
    }

    // ── delete ──
    /** Move targets: the folder roster minus the row being moved (a folder
     * cannot move into itself), with the root first. */
    const moveTargetOptions = computed(() => [
      { value: "", label: t("common.files.breadcrumbRoot") },
      ...items.value
        .filter((f) => f.kind === "folder" && f.id !== moveTarget.value?.id)
        .map((f) => ({ value: f.id, label: f.name })),
    ]);

    const deleteTarget = ref<FileDto | null>(null);
    const deleteBusy = ref(false);

    async function doDelete() {
      if (!deleteTarget.value || deleteBusy.value) return;
      deleteBusy.value = true;
      try {
        // The pane must not keep showing the row that just vanished.
        if (selected.value?.id === deleteTarget.value.id) selected.value = null;
        await files.deleteFile(deleteTarget.value.id);
        deleteTarget.value = null;
        void load();
      } catch (e) {
        toast.error(errorText(e));
      } finally {
        deleteBusy.value = false;
      }
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

    return () => (
      <section class="files-page">
        <input
          ref={fileInput}
          type="file"
          multiple
          hidden
          onChange={(e: Event) => void onFilesPicked(e)}
        />

        {/* Breadcrumb */}
        <nav class="breadcrumb" aria-label={t("common.files.breadcrumbRoot")}>
          {crumbs.value.map((crumb) => (
            <RouterLink key={crumb.to} to={crumb.to} class="crumb">
              {crumb.name}
            </RouterLink>
          ))}
        </nav>

        {/* Quota bar */}
        {quotaLimit.value ? (
          <div class="quota-bar" aria-label={t("common.files.quota")}>
            <div class="quota-track">
              <div
                class="quota-fill"
                style={{ width: `${quotaPercent.value ?? 0}%` }}
              />
            </div>
            <span class="quota-text">
              {quotaLimit.value
                ? t("common.files.quotaOf", {
                  used: formatSize(quotaUsed.value),
                  quota: formatSize(quotaLimit.value),
                })
                : t("common.files.unlimited")}
            </span>
          </div>
        ) : null}

        {loading.value && items.value.length === 0
          ? <HkEmptyState loading boxed title={t("common.status.loading")} />
          : items.value.length === 0
            ? <HkEmptyState boxed icon={Folder} title={t("common.files.empty")} />
            : (
              <HkMasterDetail
                detailOpen={selected.value !== null}
                detailTitle={selected.value?.original_name || selected.value?.name || ""}
                emptyTitle={t("common.files.details")}
                emptyDescription={t("common.files.pickHint")}
                closeLabel={t("common.actions.cancel")}
                onClose={() => { selected.value = null; }}
              >
                {{
                  list: () => (
                    <ul class="file-grid">
                      {items.value.map((f) => (
                        <li
                          key={f.id}
                          class={["file-tile", { folder: f.kind === "folder", picked: selected.value?.id === f.id }]}
                        >
                          <button
                            type="button"
                            class="tile-body"
                            onClick={() => openFolder(f)}
                            onDblclick={() => openFolder(f)}
                          >
                            {f.kind !== "image"
                              ? h(iconFor(f), { size: 32, class: "tile-icon" })
                              : <img src={`${f.url}?thumbnail=true`} alt={f.original_name || f.name} loading="lazy" />}
                          </button>
                          <div class="tile-meta">
                            <span class="tile-name" title={f.original_name || f.name}>{f.original_name || f.name}</span>
                            {f.kind !== "folder" ? <span class="tile-size">{formatSize(f.size)}</span> : null}
                          </div>
                          <div class="tile-actions">
                            <button
                              type="button"
                              class="t-action"
                              title={t("common.files.rename")}
                              onClick={() => { renameTarget.value = f; renameName.value = f.name; }}
                            >
                              <Pencil size={14} />
                            </button>
                            <button
                              type="button"
                              class="t-action"
                              title={t("common.files.move")}
                              onClick={() => { moveTarget.value = f; moveParent.value = ""; }}
                            >
                              <Move size={14} />
                            </button>
                            {f.kind !== "folder" ? (
                              <button
                                type="button"
                                class="t-action"
                                title={t("common.files.share")}
                                onClick={() => { shareTarget.value = f; shareLink.value = null; }}
                              >
                                <Link2 size={14} />
                              </button>
                            ) : null}
                            <button
                              type="button"
                              class="t-action danger"
                              title={t("common.actions.delete")}
                              onClick={() => { deleteTarget.value = f; }}
                            >
                              <Trash2 size={14} />
                            </button>
                          </div>
                        </li>
                      ))}
                    </ul>
                  ),
                  detail: () => (selected.value ? <FileDetailView file={selected.value} /> : null),
                }}
              </HkMasterDetail>
            )}

        {/* Upload progress overlay */}
        {uploadProgress.value.size > 0 ? (
          <div class="upload-overlay" role="status">
            {Array.from(uploadProgress.value).map(([name, frac]) => (
              <div key={name} class="upload-row">
                <span>{name}</span>
                <progress value={frac} max={1} />
              </div>
            ))}
          </div>
        ) : null}

        {/* New folder dialog */}
        <HkAdaptiveDialog
          modelValue={newFolderOpen.value}
          title={t("common.files.newFolder")}
          width="26rem"
          onUpdate:modelValue={(v: boolean) => { newFolderOpen.value = v; }}
        >
          <div class="dialog-body">
            <HkInput
              modelValue={newFolderName.value}
              onUpdate:modelValue={(v: string) => { newFolderName.value = v; }}
              placeholder={t("common.files.folderName")}
              data-testid="new-folder-name"
            />
            <HkButton
              variant="primary"
              disabled={!newFolderName.value.trim() || newFolderBusy.value}
              onClick={createFolder}
            >
              {t("common.actions.create")}
            </HkButton>
          </div>
        </HkAdaptiveDialog>

        {/* Rename dialog */}
        <HkAdaptiveDialog
          modelValue={renameTarget.value !== null}
          title={t("common.files.rename")}
          width="26rem"
          onUpdate:modelValue={(v: boolean) => { if (!v) renameTarget.value = null; }}
        >
          <div class="dialog-body">
            <HkInput
              modelValue={renameName.value}
              onUpdate:modelValue={(v: string) => { renameName.value = v; }}
              data-testid="rename-input"
            />
            <HkButton
              variant="primary"
              disabled={!renameName.value.trim() || renameBusy.value}
              onClick={doRename}
            >
              {t("common.actions.save")}
            </HkButton>
          </div>
        </HkAdaptiveDialog>

        {/* Move dialog */}
        <HkAdaptiveDialog
          modelValue={moveTarget.value !== null}
          title={t("common.files.move")}
          width="26rem"
          onUpdate:modelValue={(v: boolean) => { if (!v) moveTarget.value = null; }}
        >
          <div class="dialog-body">
            <label class="field-label">
              <span>{t("common.files.moveTarget")}</span>
              <HkSelect
                modelValue={moveParent.value}
                onUpdate:modelValue={(v: string) => { moveParent.value = v; }}
                options={moveTargetOptions.value}
                panelTitle={t("common.files.moveTarget")}
              />
            </label>
            <HkButton variant="primary" disabled={moveBusy.value} onClick={doMove}>
              {t("common.actions.confirm")}
            </HkButton>
          </div>
        </HkAdaptiveDialog>

        {/* Share dialog */}
        <HkAdaptiveDialog
          modelValue={shareTarget.value !== null}
          title={t("common.files.share")}
          width="28rem"
          onUpdate:modelValue={(v: boolean) => { if (!v) shareTarget.value = null; }}
        >
          <div class="dialog-body">
            <label class="field-label">
              <span>{t("common.files.shareExpires")}</span>
              <input
                value={shareExpires.value}
                onInput={(e: Event) => { shareExpires.value = (e.target as HTMLInputElement).value; }}
                type="datetime-local"
                class="native-input"
              />
            </label>
            <label class="switch-row">
              <span>{t("common.files.sharePreview")}</span>
              <HkSwitch
                modelValue={sharePreview.value}
                onUpdate:modelValue={(v: boolean) => { sharePreview.value = v; }}
              />
            </label>
            <HkButton variant="primary" disabled={shareBusy.value} onClick={doShare}>
              {t("common.actions.confirm")}
            </HkButton>
            {shareLink.value ? (
              <div class="share-result">
                <span class="share-label">{t("common.files.shareLink")}</span>
                <code>{shareLink.value}</code>
                {copyOk.value === true
                  ? <span class="share-copied">{t("common.files.shareCreated")}</span>
                  : copyOk.value === false
                    ? <span class="share-failed">{t("common.actions.copyLink")} ↓</span>
                    : null}
                <HkButton variant="ghost" onClick={() => copyShareLink()}>
                  <Copy size={14} />
                  {t("common.actions.copyLink")}
                </HkButton>
              </div>
            ) : null}
          </div>
        </HkAdaptiveDialog>

        {/* Delete confirm */}
        <HkAdaptiveDialog
          modelValue={deleteTarget.value !== null}
          title={t("common.actions.delete")}
          width="28rem"
          onUpdate:modelValue={(v: boolean) => { if (!v) deleteTarget.value = null; }}
        >
          {deleteTarget.value?.kind === "folder"
            ? <p class="warn-text">{t("common.files.folderDeleteWarning")}</p>
            : null}
          <div class="dialog-body">
            <HkButton variant="danger" disabled={deleteBusy.value} onClick={doDelete}>
              {t("common.actions.delete")}
            </HkButton>
            <HkButton variant="secondary" onClick={() => { deleteTarget.value = null; }}>
              {t("common.actions.cancel")}
            </HkButton>
          </div>
        </HkAdaptiveDialog>
      </section>
    );
  },
});