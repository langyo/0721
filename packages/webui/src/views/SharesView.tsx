import {
  HkAdminTablePage,
  HkIconButton,
  type HTableColumn,
} from "@celestia-island/hikari";
import { Link2, Link2Off } from "lucide-vue-next";
import { computed, defineComponent, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import * as files from "@/api/files";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { ShareDto } from "@/types";
import { makeErrorText } from "@/composables/errorText";

import "./SharesView.scss";

export default defineComponent({
  name: "SharesView",
  setup() {
    const { t } = useI18n();
    const shares = ref<ShareDto[]>([]);
    const loading = ref(false);
    const error = ref<string | null>(null);

    // The never-empty failure text (shared factory; the contract and
    // its pins live in composables/errorText).
    const errorText = makeErrorText(t);

    async function load() {
      error.value = null;
      loading.value = true;
      try { shares.value = await files.shareList(); } catch (e) { error.value = errorText(e); } finally { loading.value = false; }
    }
    onMounted(() => void load());
    usePageRefresh(() => load(), { loading });

    /** The roster's column set (the family table anatomy); `key` doubles as
     * the cell-slot name. The token column is copy-exempt (its rendered value
     * is truncated — the full token rides the link href). */
    const columns = computed<HTableColumn[]>(() => [
      { key: "fileName", title: t("common.files.fileName") },
      { key: "fileSize", title: t("common.files.size"), align: "right" },
      { key: "token", title: t("common.files.shareLink"), copyable: false },
      { key: "actions", title: "", align: "right", width: "3.5rem", copyable: false },
    ]);

    const revoked = ref(false);
    async function revoke(token: string) {
      // Each attempt starts from a clean slate: a stale "revoked" note over
      // a failed revoke claims a success that did not happen.
      revoked.value = false;
      try {
        await files.shareDelete(token);
        revoked.value = true;
        void load();
      } catch (e) {
        // The roster's banner is the failure lane — a silent catch leaves
        // the row on screen with no explanation and no retry.
        error.value = errorText(e);
      }
    }

    return () => (
      <section>
        {revoked.value ? <p role="status" class="ok">{t("common.files.revoked")}</p> : null}
        <HkAdminTablePage
          title={t("common.files.myShares")}
          subtitle={t("common.status.total", { count: shares.value.length })}
          countReadout={false}
          icon={Link2}
          rows={shares.value}
          columns={columns.value}
          rowKey="token"
          loading={loading.value}
          error={shares.value.length === 0 ? (error.value ?? undefined) : undefined}
          staleError={shares.value.length > 0 ? (error.value ?? undefined) : undefined}
          emptyTitle={t("common.files.noShares")}
        >
          {{
            "cell-fileName": ({ row }: { row: ShareDto }) => (
              <span class="share-name">{row.file_name}</span>
            ),

            "cell-fileSize": ({ row }: { row: ShareDto }) => (
              <span class="share-size">{row.file_size} B</span>
            ),

            "cell-token": ({ row }: { row: ShareDto }) => (
              <a href={`/s/${row.token}`} class="share-link">/s/{row.token.slice(0, 12)}…</a>
            ),

            "cell-actions": ({ row }: { row: ShareDto }) => (
              <HkIconButton
                size={24}
                variant="ghost"
                aria-label={t("common.files.revoke")}
                {...{ title: t("common.files.revoke") }}
                onClick={() => revoke(row.token)}
              >
                {{ icon: () => <Link2Off size={14} /> }}
              </HkIconButton>
            ),
          }}
        </HkAdminTablePage>
      </section>
    );
  },
});
