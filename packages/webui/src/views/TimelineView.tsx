import { HkButton, HkEmptyState, HkIconButton, HkMasterDetail } from "@celestia-island/hikari";
import { ExternalLink, Images } from "lucide-vue-next";
import { defineComponent, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as files from "@/api/files";
import FileDetailView from "@/components/FileDetailView";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { FileDto } from "@/types";
import { nativeTitle } from "@/utils/nativeTitle";

import "./TimelineView.css";

const PAGE = 40;

/** The image timeline: a tile grid of the newest first, with the shared
 *  detail pane beside it (the family master-detail anatomy). */
export default defineComponent({
  name: "TimelineView",
  setup() {
    const { t } = useI18n();
    const items = ref<FileDto[]>([]);
    const total = ref(0);
    const loading = ref(false);

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

    return () => (
      <section>
        {loading.value && items.value.length === 0
          ? <HkEmptyState loading boxed title={t("common.status.loading")} />
          : items.value.length === 0
            ? <HkEmptyState boxed icon={Images} title={t("common.status.empty")} />
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
                    <>
                      <p class="count">{t("common.status.total", { count: total.value })}</p>
                      <ul class="grid">
                        {/* The tile picks the row; the pane's actions carry the
                            open-in-new-tab the tile's old anchor used to do. */}
                        {items.value.map((f) => (
                          <li key={f.id} class={["tile", selected.value?.id === f.id ? "active" : ""]}>
                            <button
                              type="button"
                              class="tile-btn"
                              aria-label={f.original_name || f.name}
                              onClick={() => { selected.value = f; }}
                            >
                              {f.kind === "image" && (
                                <img
                                  src={`${f.url}?thumbnail=true`}
                                  alt={f.original_name || f.name}
                                  loading="lazy"
                                />
                              )}
                            </button>
                          </li>
                        ))}
                      </ul>
                      {items.value.length < total.value && (
                        <div class="more">
                          <HkButton
                            variant="ghost"
                            label={t("common.actions.loadMore")}
                            disabled={loading.value}
                            onClick={() => void load(false)}
                          />
                        </div>
                      )}
                    </>
                  ),
                  detail: () => (
                    selected.value ? <FileDetailView file={selected.value} /> : null
                  ),
                  actions: selected.value
                    ? () => (
                      <HkIconButton
                        size={28}
                        variant="ghost"
                        aria-label={t("common.portal.openOriginal")}
                        {...nativeTitle(t("common.portal.openOriginal"))}
                        onClick={() => { openExternal(selected.value!.url); }}
                      >
                        {{ icon: () => <ExternalLink size={16} /> }}
                      </HkIconButton>
                    )
                    : undefined,
                }}
              </HkMasterDetail>
            )}
      </section>
    );
  },
});