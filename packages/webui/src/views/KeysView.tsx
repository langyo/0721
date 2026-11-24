import {
  HkAdaptiveDialog,
  HkAdminTablePage,
  HkBadge,
  HkButton,
  HkIconButton,
  HkInput,
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

import "./KeysView.scss";

export default defineComponent({
  name: "KeysView",
  setup() {
    const { t } = useI18n();
    const keys = ref<ApiKeyPublic[]>([]);
    const loading = ref(false);
    const error = ref<string | null>(null);
    const created = ref<ApiKeyCreated | null>(null);
    const createOpen = ref(false);
    const newName = ref("");
    const createBusy = ref(false);
    /// Which copy button most recently succeeded (the tri-state: a target,
    /// or nothing). A FAILED copy is its own flag so the failure is visible.
    type CopyTarget = "key" | "config" | "prompt";
    const copied = ref<CopyTarget | null>(null);
    const copyFailed = ref(false);
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
        createOpen.value = false;
        await load();
      } catch (e) { error.value = errorText(e); } finally { createBusy.value = false; }
    }

    /// The MCP endpoint an agent connects to (the SSE lane).
    function mcpEndpoint(): string {
      return `${window.location.origin}/api/mcp/sse`;
    }

    /// Copy one of the copy-once card's payloads. Success is echoed on the
    /// button that copied; a FAILURE is never silent (a denied clipboard
    /// permission used to leave the button unchanged).
    async function copyText(target: CopyTarget, text: string) {
      copyFailed.value = false;
      try {
        await navigator.clipboard.writeText(text);
        copied.value = target;
        setTimeout(() => { if (copied.value === target) copied.value = null; }, 3000);
      } catch {
        copied.value = null;
        copyFailed.value = true;
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
        {/* Copy-once card: shown immediately after creation */}
        {created.value ? (
          <div class="copy-once" data-testid="copy-once-card">
            <p class="warning">{t("common.keys.copyOnceWarning")}</p>
            <code class="key-text" data-testid="plaintext-key">{created.value.key}</code>
            <div class="actions">
              <HkButton variant="primary" data-testid="copy-key" onClick={copyKey}>
                {{
                  icon: () => <Copy size={14} />,
                  default: () =>                       copied.value === "key" ? t("common.keys.copied") : t("common.actions.copyLink"),
                }}
              </HkButton>
              <HkButton variant="ghost" data-testid="copy-config" onClick={copyConfig}>
                {copied.value === "config" ? t("common.keys.copied") : t("common.keys.copyConfig")}
              </HkButton>
              <HkButton variant="ghost" data-testid="copy-prompt" onClick={copyPrompt}>
                {copied.value === "prompt" ? t("common.keys.copied") : t("common.keys.copyPrompt")}
              </HkButton>
            </div>
            {copyFailed.value ? <p role="alert" data-testid="copy-failed" class="copy-failed">{t("common.keys.copyFailed")}</p> : null}
          </div>
        ) : null}

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
              row.revoked_at ? null : (
                <HkIconButton
                  size={24}
                  variant="ghost"
                  aria-label={t("common.keys.revoke")}
                  {...{ title: t("common.keys.revoke") }}
                  onClick={() => (pendingRevoke.value = row)}
                >
                  {{ icon: () => <Trash2 size={14} /> }}
                </HkIconButton>
              )
            ),
          }}
        </HkAdminTablePage>

        <HkAdaptiveDialog
          modelValue={createOpen.value}
          title={t("common.keys.create")}
          width="26rem"
          onUpdate:modelValue={(v: boolean) => (createOpen.value = v)}
        >
          <div class="dialog-body">
            <HkInput v-model={newName.value} placeholder={t("common.keys.nameLabel")} data-testid="key-name" />
            <HkButton variant="primary" data-testid="new-key" disabled={!newName.value.trim() || createBusy.value} onClick={doCreate}>{t("common.actions.create")}</HkButton>
          </div>
        </HkAdaptiveDialog>

        <HkAdaptiveDialog
          modelValue={pendingRevoke.value !== null}
          title={t("common.keys.revoke")}
          width="26rem"
          onUpdate:modelValue={(v: boolean) => !v && (pendingRevoke.value = null)}
        >
          <p>{t("common.keys.revokeConfirm", { name: pendingRevoke.value?.name ?? "" })}</p>
          <div class="dialog-body">
            <HkButton variant="danger" disabled={revokeBusy.value} onClick={doRevoke}>{t("common.keys.revoke")}</HkButton>
            <HkButton variant="secondary" onClick={() => (pendingRevoke.value = null)}>{t("common.actions.cancel")}</HkButton>
          </div>
        </HkAdaptiveDialog>
      </section>
    );
  },
});
