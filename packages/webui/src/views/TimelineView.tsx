import { HkButton, HkEmptyState, HkIconButton, HkMasterDetail } from "@celestia-island/hikari";
import { ExternalLink, Images } from "lucide-vue-next";
import { defineComponent, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import * as files from "@/api/files";
import FileDetailView from "@/components/FileDetailView";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { FileDto } from "@/types";

import "./TimelineView.scss";

export default defineComponent({
  name: "TimelineView",
  setup() {
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

    return () => (
      <section>
        {loading.value && items.value.length === 0 ? (
          <HkEmptyState loading boxed title={t("common.status.loading")} />
        ) : items.value.length === 0 ? (
          <HkEmptyState boxed icon={Images} title={t("common.status.empty")} />
        ) : (
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
                  <p class="count">{t("common.status.total", { count: total.value })}</p>
                  <ul class="grid">
                    {/* The tile picks the row; the pane's actions carry the
                         open-in-new-tab the tile's old anchor used to do. */}
                    {items.value.map((f) => (
                      <li key={f.id} class={["tile", { active: selected.value?.id === f.id }]}>
                        <button type="button" class="tile-btn" aria-label={f.original_name || f.name} onClick={() => (selected.value = f)}>
                          {f.kind === "image" ? (
                            <img src={`${f.url}?thumbnail=true`} alt={f.original_name || f.name} loading="lazy" />
                          ) : null}
                        </button>
                      </li>
                    ))}
                  </ul>
                  {items.value.length < total.value ? (
                    <div class="more">
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
        )}
      </section>
    );
  },
});
