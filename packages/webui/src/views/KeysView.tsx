import {
  HkAdaptiveDialog,
  HkAdminTablePage,
  HkBadge,
  HkButton,
  HkIconButton,
  HkInput,
  useToast,
  type HTableColumn,
} from "@celestia-island/hikari";
import { Copy, KeyRound, Trash2 } from "lucide-vue-next";
import { computed, defineComponent, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as api from "@/api";
import { usePageCreateAction } from "@/composables/usePageCreateAction";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { ApiKeyCreated, ApiKeyPublic } from "@/api";
import { makeErrorText } from "@/composables/errorText";
import { nativeTitle } from "@/utils/nativeTitle";

import "./KeysView.css";

/// Which copy button most recently succeeded (the tri-state: a target,
/// or nothing).
type CopyTarget = "key" | "config" | "prompt";

/**
 * The API-key roster plus its copy-once card.
 *
 * A FAILED clipboard write is a TOAST (the family feedback contract):
 * the card never grows an error paragraph beside its buttons — the
 * success half of the tri-state stays on the button that copied, and
 * the failure half is raised once, on the toast surface.
 */
export default defineComponent({
  name: "KeysView",
  setup() {
    const { t } = useI18n();
    const toast = useToast();
    const keys = ref<ApiKeyPublic[]>([]);
    const loading = ref(false);
    const error = ref<string | null>(null);
    const created = ref<ApiKeyCreated | null>(null);
    const createOpen = ref(false);
    const newName = ref("");
    const createBusy = ref(false);
    const copied = ref<CopyTarget | null>(null);
    const pendingRevoke = ref<ApiKeyPublic | null>(null);
    const revokeBusy = ref(false);

    /** The roster's column set (the family table anatomy). `key` doubles as
     * the cell-slot name (`cell-<key>`); the action column opts out of
     * click-to-copy. */
    const columns = computed<HTableColumn[]>(() => [
      { key: "name", title: t("common.keys.nameLabel") },
      { key: "prefix", title: t("common.keys.prefix"), copyable: false },
      { key: "state", title: t("common.keys.state") },
      { key: "actions", title: "", align: "right", width: "3.5rem", copyable: false },
    ]);

    // The never-empty failure text (shared factory; the contract and
    // its pins live in composables/errorText).
    const errorText = makeErrorText(t);

    async function load() {
      error.value = null;
      loading.value = true;
      try { keys.value = await api.listKeys(); } catch (e) { error.value = errorText(e); } finally { loading.value = false; }
    }
    onMounted(() => void load());
    usePageRefresh(() => load(), { loading });
    usePageCreateAction({
      label: () => t("common.keys.create"),
      onCreate: () => { newName.value = ""; created.value = null; createOpen.value = true; },
    });

    async function doCreate() {
      if (!newName.value.trim() || createBusy.value) return;
      createBusy.value = true;
      try {
        created.value = await api.createKey(newName.value.trim());
        // The dialog STAYS OPEN and swaps to the minted-key step: the
        // copy-once card used to grow at the top of the page after the
        // dialog closed (a module inserted into the page by a click —
        // banned by the user's rule). The credential now appears where
        // the action happened, in the secondary window.
        await load();
      } catch (e) { error.value = errorText(e); } finally { createBusy.value = false; }
    }

    /** Close the create window and drop the minted key from memory (it is
     *  shown once, by design — reopening the dialog starts a new key). */
    function finishCreated(): void {
      created.value = null;
      createOpen.value = false;
    }

    /// The MCP endpoint an agent connects to (the SSE lane).
    function mcpEndpoint(): string {
      return `${window.location.origin}/api/mcp/sse`;
    }

    /// Copy one of the copy-once card's payloads. Success is echoed on the
    /// button that copied; a FAILURE is never silent (a denied clipboard
    /// permission used to leave the button unchanged).
    async function copyText(target: CopyTarget, text: string) {
      try {
        await navigator.clipboard.writeText(text);
        copied.value = target;
        setTimeout(() => { if (copied.value === target) copied.value = null; }, 3000);
      } catch {
        copied.value = null;
        toast.error(t("common.keys.copyFailed"));
      }
    }

    function copyKey() {
      if (created.value) void copyText("key", created.value.key);
    }

    function copyConfig() {
      void copyText("config", t("common.keys.configTemplate", { endpoint: mcpEndpoint(), key: created.value?.key ?? "" }));
    }

    function copyPrompt() {
      void copyText("prompt", t("common.keys.promptTemplate", { endpoint: mcpEndpoint(), key: created.value?.key ?? "" }));
    }

    async function doRevoke() {
      if (!pendingRevoke.value || revokeBusy.value) return;
      revokeBusy.value = true;
      try { await api.revokeKey(pendingRevoke.value.id); pendingRevoke.value = null; void load(); }
      catch (e) { error.value = errorText(e); } finally { revokeBusy.value = false; }
    }

    return () => (
      <section class="keys-page">
        <HkAdminTablePage
          title={t("common.keys.title")}
          subtitle={t("common.status.total", { count: keys.value.length })}
          countReadout={false}
          icon={KeyRound}
          rows={keys.value}
          columns={columns.value}
          rowKey="id"
          loading={loading.value}
          error={keys.value.length === 0 ? (error.value ?? undefined) : undefined}
          staleError={keys.value.length > 0 ? (error.value ?? undefined) : undefined}
          emptyTitle={t("common.keys.empty")}
        >
          {{
            "cell-name": ({ row }: { row: ApiKeyPublic }) => (
              <span class="key-name-cell">
                <KeyRound size={14} />
                <span>{row.name}</span>
              </span>
            ),
            "cell-prefix": ({ row }: { row: ApiKeyPublic }) => (
              <code class="key-prefix">{row.key_prefix}</code>
            ),
            "cell-state": ({ row }: { row: ApiKeyPublic }) => (
              row.revoked_at
                ? <HkBadge variant="muted" pill>{t("common.keys.revoked")}</HkBadge>
                : <HkBadge variant="success" pill>{t("common.keys.active")}</HkBadge>
            ),
            "cell-actions": ({ row }: { row: ApiKeyPublic }) => (
              !row.revoked_at
                ? (
                  <HkIconButton
                    size={24}
                    variant="ghost"
                    aria-label={t("common.keys.revoke")}
                    {...nativeTitle(t("common.keys.revoke"))}
                    onClick={() => { pendingRevoke.value = row; }}
                  >
                    {{ icon: () => <Trash2 size={14} /> }}
                  </HkIconButton>
                )
                : null
            ),
          }}
        </HkAdminTablePage>

        <HkAdaptiveDialog
          modelValue={createOpen.value}
          title={t("common.keys.create")}
          width="26rem"
          onUpdate:modelValue={(v: boolean) => { if (!v) finishCreated(); }}
        >
          {created.value
            ? (
              <div class="copy-once" data-testid="copy-once-card">
                <p class="warning">{t("common.keys.copyOnceWarning")}</p>
                <code class="key-text" data-testid="plaintext-key">{created.value.key}</code>
                <div class="actions">
                  <HkButton
                    variant="primary"
                    label={copied.value === "key" ? t("common.keys.copied") : t("common.actions.copyLink")}
                    data-testid="copy-key"
                    onClick={copyKey}
                  />
                  <HkButton
                    variant="ghost"
                    label={copied.value === "config" ? t("common.keys.copied") : t("common.keys.copyConfig")}
                    data-testid="copy-config"
                    onClick={copyConfig}
                  />
                  <HkButton
                    variant="ghost"
                    label={copied.value === "prompt" ? t("common.keys.copied") : t("common.keys.copyPrompt")}
                    data-testid="copy-prompt"
                    onClick={copyPrompt}
                  />
                </div>
                <div class="dialog-body">
                  <HkButton
                    variant="secondary"
                    label={t("common.actions.confirm")}
                    data-testid="copy-once-done"
                    onClick={finishCreated}
                  />
                </div>
              </div>
            )
            : (
              <div class="dialog-body">
                <HkInput
                  modelValue={newName.value}
                  onUpdate:modelValue={(v: string) => { newName.value = v; }}
                  placeholder={t("common.keys.nameLabel")}
                  data-testid="key-name"
                />
                <HkButton
                  variant="primary"
                  label={t("common.actions.create")}
                  data-testid="new-key"
                  disabled={!newName.value.trim() || createBusy.value}
                  onClick={doCreate}
                />
              </div>
            )}
        </HkAdaptiveDialog>

        <HkAdaptiveDialog
          modelValue={pendingRevoke.value !== null}
          title={t("common.keys.revoke")}
          width="26rem"
          onUpdate:modelValue={(v: boolean) => { if (!v) pendingRevoke.value = null; }}
        >
          <p>{t("common.keys.revokeConfirm", { name: pendingRevoke.value?.name ?? "" })}</p>
          <div class="dialog-body">
            <HkButton
              variant="danger"
              label={t("common.keys.revoke")}
              disabled={revokeBusy.value}
              onClick={doRevoke}
            />
            <HkButton
              variant="secondary"
              label={t("common.actions.cancel")}
              onClick={() => { pendingRevoke.value = null; }}
            />
          </div>
        </HkAdaptiveDialog>
      </section>
    );
  },
});