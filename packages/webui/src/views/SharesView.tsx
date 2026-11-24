import {
  HkAdminTablePage,
  HkIconButton,
  useToast,
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

/**
 * The share roster: one row per live share link, revoke per row.
 *
 * Every OPERATION's outcome is a TOAST (the family feedback contract): a
 * revoked share says so, and a failed revoke says so — nothing about the
 * attempt inserts a paragraph beside (or above) the table. The table's own
 * `error` / `staleError` lanes stay as they are: those are the scaffold's
 * ROSTER state (the fetch that feeds the table), not the per-row action's
 * result.
 */

/** This view's success slot. Keyed so the revoked note can be dismissed
 *  without touching any other surface's success toast. */
const REVOKED_TOAST_KEY = "shares-revoked";

export default defineComponent({
  name: "SharesView",
  setup() {
    const { t } = useI18n();
    const toast = useToast();
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

    /** Announce the revoke on THIS view's own slot.
     *
     *  The key is load-bearing on BOTH ends: an unkeyed success push lands
     *  in the one slot the whole app shares, so clearing it would wipe an
     *  unrelated view's confirmation, while a keyed push with an unkeyed
     *  clear leaves a stale "revoked" sitting over a failure. Push and
     *  clear are therefore one pair, written here and read nowhere else. */
    function announceRevoked(): void {
      toast.show(t("common.files.revoked"), { type: "success", key: REVOKED_TOAST_KEY });
    }

    /** Dismiss THIS view's revoked note — never the shared success slot. */
    function clearRevoked(): void {
      const stale = toast.toasts.find(
        (slot) => slot.type === "success" && slot.key === REVOKED_TOAST_KEY,
      );
      if (stale) toast.remove(stale.id);
    }

    async function revoke(token: string) {
      try {
        await files.shareDelete(token);
        // Success is announced on the toast surface only (never a note
        // beside the roster).
        announceRevoked();
        void load();
      } catch (e) {
        // Each attempt starts from a clean slate: a stale "revoked" toast
        // over a failed revoke claims a success that did not happen (the
        // old inline note was reset the same way).
        clearRevoked();
        // The failure lane — a silent catch leaves the row on screen with
        // no explanation and no retry.
        toast.error(errorText(e));
      }
    }

    return () => (
      <section>
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
