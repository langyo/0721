import {
  HkAdaptiveDialog,
  HkButton,
  HkEmptyState,
  HkIconButton,
  HkMasterDetail,
  HkProgressRing,
  HkSelect,
  useToast,
} from "@celestia-island/hikari";
import { ExternalLink, Copy, FileArchive, FileAudio, FileText, FileVideo, Folder, Images, Trash2 } from "lucide-vue-next";
import { computed, h, defineComponent, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as api from "@/api";
import { listBuckets, type StorageBucket } from "@/api/buckets";
import { usePageCreateAction } from "@/composables/usePageCreateAction";
import { usePageRefresh } from "@/composables/usePageRefresh";
import { useMediaPlaceholderLadder } from "@/composables/mediaPlaceholderLadder";
import type { FileDto } from "@/types";
import { makeErrorText } from "@/composables/errorText";
import FileDetailView from "@/components/FileDetailView";
import { formatSize } from "@/composables/formatSize";

import "./PortalView.scss";

/**
 * The portal face: the newest-first grid of everything on the instance,
 * with the upload entry riding the header's create slot
 * (`usePageCreateAction`) and per-tile copy-link / delete.
 *
 * Picking a tile opens the shared detail pane beside the grid; the pane's
 * own action opens the original in a new tab.
 *
 * Operation failures are TOASTS (the family feedback contract): a rejected
 * upload raises one toast at the point the item fails, and a rejected
 * delete raises one — nothing about either inserts DOM beside the row, the
 * dialog or the upload lane. The load lane keeps its page-level empty state
 * (a permanent surface, not transient feedback).
 */
export default defineComponent({
  name: "PortalView",
  setup() {
    const { t } = useI18n();

    /** The pane's selection — this page's state, never the component's. */
    const selected = ref<FileDto | null>(null);

    function openExternal(url: string): void {
      window.open(url, "_blank", "noopener");
    }

    const errorText = makeErrorText(t);
    const uploadErrorText = makeErrorText(t, "common.portal.uploadFailed");
    const toast = useToast();
    // The R-M② placeholder ladder for the grid's image tiles (the
    // detail pane's FileDetailView runs its own).
    const tileLadder = useMediaPlaceholderLadder();

    async function onTileImageError(file: FileDto): Promise<void> {
      await tileLadder.resolve(
        file.url,
        async () => {
          try {
            const response = await fetch("/api/v1/auth/refresh", {
              method: "POST",
              credentials: "same-origin",
            });
            return response.ok;
          } catch {
            return false;
          }
        },
        async (url) => {
          try {
            const retry = await fetch(url, { credentials: "same-origin" });
            return retry.status;
          } catch {
            return 0;
          }
        },
      );
    }

    const PAGE_SIZE = 30;

    const files = ref<FileDto[]>([]);
    const total = ref(0);
    const loading = ref(false);
    const loadError = ref<string | null>(null);

    const fileInput = ref<HTMLInputElement | null>(null);

    // ── the upload bucket dropdown (P2a / R-I) ─────────────────────────
    //
    // The actor's READABLE buckets from the visibility-scoped registry
    // list; the picked slug rides the upload as `?bucket=` (the server's
    // `resolve_upload_bucket` resolves/refuses — R-I's single outlet).
    // An EMPTY selection sends NO param: the server resolves the default
    // (draft §9: the default is the admin's setting, never shown as
    // "the first bucket").
    const uploadBuckets = ref<StorageBucket[]>([]);
    const uploadBucket = ref<string>("");

    const uploadBucketOptions = computed(() => [
      { value: "", label: t("common.portal.uploadDefaultBucket") },
      ...uploadBuckets.value.map((b) => ({
        value: b.slug,
        label:
          b.my_access && !b.my_access.read
            ? `${b.slug} — ${t("common.files.noReadPermission")}`
            : b.slug,
      })),
    ]);

    async function loadUploadBuckets() {
      try {
        uploadBuckets.value = (await listBuckets()).buckets;
      } catch {
        // A failed registry read degrades to the default-only dropdown —
        // the server still resolves (or refuses) on the wire.
        uploadBuckets.value = [];
      }
    }
    onMounted(() => void loadUploadBuckets());

    interface UploadEntry {
      id: number;
      name: string;
      progress: number;
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
        const entry: UploadEntry = { id: uploadSeq++, name: file.name, progress: 0 };
        uploads.value = [...uploads.value, entry];
        void api
          .uploadFile(file, (fraction) => {
            entry.progress = fraction;
            uploads.value = [...uploads.value];
          }, uploadBucket.value || undefined)
          .then(() => {
            uploads.value = uploads.value.filter((u) => u.id !== entry.id);
            void load(true);
          })
          .catch((err: unknown) => {
            // A failed upload leaves NO row behind: a frozen ring with no
            // message is a dead end the visitor cannot dismiss. The toast
            // is the whole report, and it names the file — the only place
            // that association survives the toast's auto-dismiss.
            uploads.value = uploads.value.filter((u) => u.id !== entry.id);
            toast.error(`${file.name}: ${uploadErrorText(err)}`);
          });
      }
    }

    // ── per-tile actions ───────────────────────────────────────────────

    const copiedId = ref<string | null>(null);
    const pendingDelete = ref<FileDto | null>(null);
    const deleteBusy = ref(false);

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
      try {
        await api.deleteFile(target.id);
        // The pane must not keep showing the row that just vanished.
        if (selected.value?.id === target.id) selected.value = null;
        files.value = files.value.filter((f) => f.id !== target.id);
        total.value = Math.max(0, total.value - 1);
        pendingDelete.value = null;
      } catch (err) {
        // A failed delete is a toast — the dialog must not grow a note.
        toast.error(errorText(err));
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

    return () => (
      <section class="portal">
        {/* The upload picker: hidden input driven by the action-bar plus.
            The bucket dropdown rides ABOVE the grid (P2a / R-I): the
            picked slug goes as `?bucket=`, empty = the server default. */}
        <input ref={fileInput} type="file" multiple hidden data-testid="upload-input" onChange={onFilesPicked} />

        {uploadBucketOptions.value.length > 1 ? (
          <div class="upload-bucket" data-testid="upload-bucket-picker">
            <HkSelect
              modelValue={uploadBucket.value}
              options={uploadBucketOptions.value}
              panelTitle={t("common.portal.uploadBucket")}
              placeholder={t("common.portal.uploadDefaultBucket")}
              data-testid="upload-bucket-select"
              onUpdate:modelValue={(v: string) => (uploadBucket.value = v)}
            />
          </div>
        ) : null}

        {loading.value && files.value.length === 0 ? (
          <HkEmptyState loading boxed title={t("common.status.loading")} />
        ) : loadError.value ? (
          <HkEmptyState
            boxed
            {...{ title: t("common.status.error") }}
            description={loadError.value}
          />
        ) : files.value.length === 0 ? (
          <HkEmptyState
            boxed
            icon={Images}
            {...{ title: t("common.status.empty") }}
            description={t("common.portal.empty")}
          />
        ) : (
          <>
            <p class="portal-count">{t("common.status.total", { count: total.value })}</p>
            <HkMasterDetail
              detailOpen={selected.value !== null}
              detailTitle={selected.value?.original_name || selected.value?.name || ""}
              emptyTitle={t("common.files.details")}
              emptyDescription={t("common.files.pickHint")}
              closeLabel={t("common.actions.cancel")}
              onClose={() => (selected.value = null)}
            >
              {{
                list: () => (
                  <>
                    <ul class="portal-grid">
                      {files.value.map((file) => (
                        <li key={file.id} class="portal-tile">
                          <button
                            type="button"
                            class={["tile-media", { picked: selected.value?.id === file.id }]}
                            aria-label={file.original_name || file.name}
                            onClick={() => (selected.value = file)}
                          >
                            {file.kind === "image" && !tileLadder.faceOf(file.url) ? (
                              <img
                                src={`${file.url}?thumbnail=true`}
                                alt={file.original_name || file.name}
                                loading="lazy"
                                onError={() => void onTileImageError(file)}
                                onLoad={() => tileLadder.clear(file.url)}
                              />
                            ) : file.kind === "image" && tileLadder.faceOf(file.url) ? (
                              <span
                                class="tile-media-placeholder"
                                data-testid={`media-placeholder-${tileLadder.faceOf(file.url)!.kind}`}
                              >
                                {t(
                                  tileLadder.faceOf(file.url)!.kind === "sign-in"
                                    ? "common.media.signin"
                                    : tileLadder.faceOf(file.url)!.kind === "forbidden"
                                      ? "common.media.forbidden"
                                      : tileLadder.faceOf(file.url)!.kind === "unavailable"
                                        ? "common.media.unavailable"
                                        : "common.media.missing",
                                )}
                              </span>
                            ) : (
                              h(iconFor(file), { size: 32, class: "tile-icon" })
                            )}
                          </button>
                          <div class="tile-meta">
                            <span class="tile-name" title={file.original_name || file.name}>
                              {file.original_name || file.name}
                            </span>
                            <span class="tile-size">{formatSize(file.size)}</span>
                          </div>
                          <div class="tile-actions">
                            <button
                              type="button"
                              class="tile-action"
                              aria-label={t("common.actions.copyLink")}
                              title={t("common.actions.copyLink")}
                              onClick={() => copyLink(file)}
                            >
                              {copiedId.value !== file.id ? <Copy size={14} /> : <span class="tile-copied">{t("common.actions.copied")}</span>}
                            </button>
                            <button
                              type="button"
                              class="tile-action danger"
                              aria-label={t("common.actions.delete")}
                              title={t("common.actions.delete")}
                              onClick={() => { pendingDelete.value = file; }}
                            >
                              <Trash2 size={14} />
                            </button>
                          </div>
                        </li>
                      ))}
                    </ul>
                    {hasMore.value ? (
                      <div class="portal-more">
                        <HkButton variant="ghost" disabled={loading.value} onClick={() => load(false)}>
                          {t("common.actions.loadMore")}
                        </HkButton>
                      </div>
                    ) : null}
                  </>
                ),

                detail: () => (
                  selected.value ? <FileDetailView file={selected.value} /> : null
                ),

                // The pane's actions only exist while a row is picked (the
                // slot is omitted otherwise, exactly as the template did).
                ...(selected.value
                  ? {
                      actions: () => (
                        <HkIconButton
                          size={28}
                          variant="ghost"
                          aria-label={t("common.portal.openOriginal")}
                          {...{ title: t("common.portal.openOriginal") }}
                          onClick={() => openExternal(selected.value!.url)}
                        >
                          {{ icon: () => <ExternalLink size={16} /> }}
                        </HkIconButton>
                      ),
                    }
                  : {}),
              }}
            </HkMasterDetail>
          </>
        )}

        {/* The upload lane: one row per IN-FLIGHT file with its progress. A
            failed file leaves no row at all — its toast names the file and
            the reason. */}
        {uploads.value.length > 0 ? (
          <div
            class="upload-lane"
            role="status"
            aria-live="polite"
            aria-label={t("common.portal.uploading", { name: uploads.value[0]!.name })}
          >
            {uploads.value.map((entry) => (
              <div key={entry.id} class="upload-row">
                <HkProgressRing value={entry.progress} size={20} strokeWidth={3} />
                <span class="upload-name" title={entry.name}>{entry.name}</span>
              </div>
            ))}
          </div>
        ) : null}

        <HkAdaptiveDialog
          modelValue={pendingDelete.value !== null}
          title={t("common.actions.delete")}
          footerActions={deleteActions.value}
          width="28rem"
          onUpdate:modelValue={(v: boolean) => !v && ((pendingDelete.value = null))}
        >
          <p>{t("common.portal.deleteConfirm")}</p>
        </HkAdaptiveDialog>
      </section>
    );
  },
});
