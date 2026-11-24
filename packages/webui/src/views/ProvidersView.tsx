import {
  HkAdaptiveDialog,
  HkAdminTablePage,
  HkBadge,
  HkIconButton,
  HkInput,
  HkRadio,
  HkSwitch,
  useToast,
  type HTableColumn,
} from "@celestia-island/hikari";
import { Pencil, Plus, Trash2, Unplug } from "lucide-vue-next";
import { computed, defineComponent, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as api from "@/api";
import { usePageCreateAction } from "@/composables/usePageCreateAction";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { OAuthProviderPublic } from "@/types";
import { makeErrorText } from "@/composables/errorText";

import "./ProvidersView.scss";

/**
 * The admin login-provider registry: list / create / edit / enable /
 * delete. The client secret is write-only — the server answers with
 * `has_secret` + the last four characters, and an EMPTY secret field
 * on save keeps the stored one.
 *
 * Operation failures are TOASTS (the family feedback contract): a rejected
 * save, a rejected delete and a rejected enable-toggle each raise one —
 * the page, the edit dialog and the delete dialog insert no note of their
 * own. The load lane keeps its scaffold empty state (a permanent surface).
 */
export default defineComponent({
  name: "ProvidersView",
  setup() {
    const { t } = useI18n();

    const rows = ref<OAuthProviderPublic[]>([]);
    const loading = ref(false);
    const loadError = ref<string | null>(null);

    const PROVIDER_TYPES = ["builtin:github", "builtin:feishu", "builtin:erp", "oidc"] as const;

    /** The family radio takes {value,label}; the provider kinds are already
     * plain tokens, so each label is its own code. */
    const providerTypeOptions = computed(() =>
      PROVIDER_TYPES.map((kind) => ({ value: kind, label: kind })),
    );

    // The never-empty failure text (shared factory; the contract and
    // its pins live in composables/errorText).
    const errorText = makeErrorText(t);
    const toast = useToast();

    async function load() {
      loading.value = true;
      loadError.value = null;
      try {
        rows.value = await api.listProviders();
      } catch (err) {
        loadError.value = errorText(err);
      } finally {
        loading.value = false;
      }
    }

    onMounted(() => {
      void load();
    });

    usePageRefresh(() => load(), { loading });

    // ── the edit dialog ────────────────────────────────────────────────

    const dialogOpen = ref(false);
    const saving = ref(false);
    const editingId = ref<string | null>(null);
    const form = ref(emptyForm());

    function emptyForm() {
      return {
        id: "",
        display_name: "",
        provider_type: "builtin:erp" as (typeof PROVIDER_TYPES)[number],
        client_id: "",
        client_secret: "",
        base_url: "",
        issuer_url: "",
        scopes: "",
        enabled: true,
      };
    }

    function openCreate() {
      editingId.value = null;
      form.value = emptyForm();
      dialogOpen.value = true;
    }

    function openEdit(row: OAuthProviderPublic) {
      editingId.value = row.id;
      form.value = {
        id: row.id,
        display_name: row.display_name,
        provider_type: row.type as (typeof PROVIDER_TYPES)[number],
        client_id: row.client_id,
        client_secret: "",
        base_url: row.base_url ?? "",
        issuer_url: row.issuer_url ?? "",
        scopes: row.scopes ?? "",
        enabled: row.enabled,
      };
      dialogOpen.value = true;
    }

    const valid = computed(() => /^[a-z0-9][a-z0-9._-]*$/.test(form.value.id) && form.value.id.length <= 48 && form.value.display_name.trim() !== "" && form.value.client_id.trim() !== "");

    const dialogActions = computed(() => [
      {
        label: t("common.actions.cancel"),
        variant: "secondary" as const,
        onClick: () => {
          dialogOpen.value = false;
        },
      },
      {
        label: t("common.actions.save"),
        variant: "primary" as const,
        loading: saving.value,
        disabled: !valid.value,
        onClick: () => {
          void save();
        },
      },
    ]);

    usePageCreateAction({
      label: () => t("common.providers.create"),
      onCreate: openCreate,
    });

    async function save() {
      if (!valid.value || saving.value) return;
      saving.value = true;
      try {
        await api.upsertProvider({
          id: form.value.id,
          display_name: form.value.display_name,
          type: form.value.provider_type,
          client_id: form.value.client_id,
          client_secret: form.value.client_secret,
          base_url: form.value.base_url.trim() || null,
          issuer_url: form.value.issuer_url.trim() || null,
          // Empty = null = the server's per-template default scopes.
          scopes: form.value.scopes.trim() || null,
          enabled: form.value.enabled,
          icon: null,
        });
        dialogOpen.value = false;
        await load();
      } catch (err) {
        // A failed save is a toast — the edit dialog must not grow a note.
        toast.error(errorText(err));
      } finally {
        saving.value = false;
      }
    }

    // ── enable toggle + delete ─────────────────────────────────────────

    async function toggle(row: OAuthProviderPublic, enabled: boolean) {
      const previous = rows.value;
      rows.value = rows.value.map((r) => (r.id === row.id ? { ...r, enabled } : r));
      try {
        await api.upsertProvider({
          id: row.id,
          display_name: row.display_name,
          type: row.type,
          client_id: row.client_id,
          client_secret: "",
          base_url: row.base_url ?? null,
          issuer_url: row.issuer_url ?? null,
          scopes: row.scopes ?? null,
          enabled,
          icon: row.icon ?? null,
        });
      } catch (err) {
        rows.value = previous;
        // The failure must be VISIBLE (the row just snapped back — say why).
        // The family lane for that is a toast, not a banner above the table.
        toast.error(errorText(err));
      }
    }

    const pendingDelete = ref<OAuthProviderPublic | null>(null);
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
        await api.deleteProvider(target.id);
        rows.value = rows.value.filter((r) => r.id !== target.id);
        pendingDelete.value = null;
      } catch (err) {
        // A failed delete is a toast — the dialog must not grow a note.
        toast.error(errorText(err));
      } finally {
        deleteBusy.value = false;
      }
    }

    /** The roster's column set (the family table anatomy); `key` doubles as
     * the cell-slot name. The secret column is copy-exempt (it renders a
     * mask, never the value). */
    const columns = computed<HTableColumn[]>(() => [
      { key: "name", title: t("common.providers.displayName") },
      { key: "secret", title: t("common.providers.secret"), copyable: false },
      { key: "enabled", title: t("common.providers.enabled"), align: "center", copyable: false },
      { key: "actions", title: "", align: "right", width: "6rem", copyable: false },
    ]);

    return () => (
      <section class="providers">
        <HkAdminTablePage
          title={t("common.providers.title")}
          subtitle={t("common.status.total", { count: rows.value.length })}
          countReadout={false}
          icon={Unplug}
          rows={rows.value}
          columns={columns.value}
          rowKey="id"
          loading={loading.value}
          error={rows.value.length === 0 ? (loadError.value ?? undefined) : undefined}
          staleError={rows.value.length > 0 ? (loadError.value ?? undefined) : undefined}
          emptyTitle={t("common.providers.empty")}
        >
          {{
            "cell-name": ({ row }: { row: OAuthProviderPublic }) => (
              <button type="button" class="provider-name-cell" onClick={() => openEdit(row)}>
                <span class="provider-name">{row.display_name}</span>
                <span class="provider-meta">{row.id} · {row.type}</span>
              </button>
            ),

            "cell-secret": ({ row }: { row: OAuthProviderPublic }) => (
              row.has_secret
                ? <HkBadge variant="muted" mono pill>{`••••${row.secret_tail}`}</HkBadge>
                : <HkBadge variant="warning" pill>{t("common.providers.noSecret")}</HkBadge>
            ),

            "cell-enabled": ({ row }: { row: OAuthProviderPublic }) => (
              <HkSwitch
                modelValue={row.enabled}
                aria-label={t("common.providers.enabled")}
                onUpdate:modelValue={(v: boolean) => toggle(row, v)}
              />
            ),

            "cell-actions": ({ row }: { row: OAuthProviderPublic }) => (
              <span class="provider-actions">
                <HkIconButton
                  size={24}
                  variant="ghost"
                  aria-label={t("common.providers.edit")}
                  {...{ title: t("common.providers.edit") }}
                  onClick={() => openEdit(row)}
                >
                  {{ icon: () => <Pencil size={14} /> }}
                </HkIconButton>
                <HkIconButton
                  size={24}
                  variant="ghost"
                  aria-label={t("common.actions.delete")}
                  {...{ title: t("common.actions.delete") }}
                  onClick={() => { pendingDelete.value = row; }}
                >
                  {{ icon: () => <Trash2 size={14} /> }}
                </HkIconButton>
              </span>
            ),
          }}
        </HkAdminTablePage>

        <HkAdaptiveDialog
          modelValue={dialogOpen.value}
          title={editingId.value ? editingId.value : t("common.providers.create")}
          footerActions={dialogActions.value}
          width="30rem"
          onUpdate:modelValue={(v: boolean) => (dialogOpen.value = v)}
        >
          <form class="edit-form" onSubmit={(e: Event) => { e.preventDefault(); save(); }}>
            <label class="field">
              <span>{t("common.providers.id")}</span>
              <HkInput v-model={form.value.id} name="provider-id" autocomplete="off" disabled={editingId.value !== null} data-testid="provider-id" />
            </label>
            <label class="field">
              <span>{t("common.providers.displayName")}</span>
              <HkInput v-model={form.value.display_name} name="provider-name" />
            </label>
            <div class="field">
              <span>{t("common.providers.type")}</span>
              <div role="radiogroup" aria-label={t("common.providers.type")}>
                <HkRadio v-model={form.value.provider_type} options={providerTypeOptions.value} size="sm" />
              </div>
            </div>
            <label class="field">
              <span>{t("common.providers.clientId")}</span>
              <HkInput v-model={form.value.client_id} name="provider-client-id" autocomplete="off" />
            </label>
            <label class="field">
              <span>{t("common.providers.clientSecret")}</span>
              <HkInput v-model={form.value.client_secret} name="provider-secret" type="password" autocomplete="new-password" />
              <small class="hint">{t("common.providers.secretHint")}</small>
            </label>
            {form.value.provider_type === "builtin:erp" ? (
              <label class="field">
                <span>{t("common.providers.baseUrl")}</span>
                <HkInput v-model={form.value.base_url} name="provider-base-url" placeholder="https://auth.example.com" />
              </label>
            ) : null}
            {form.value.provider_type === "oidc" ? (
              <label class="field">
                <span>{t("common.providers.issuerUrl")}</span>
                <HkInput v-model={form.value.issuer_url} name="provider-issuer" placeholder="https://sso.example.com" />
              </label>
            ) : null}
            <label class="field">
              <span>{t("common.providers.scopes")}</span>
              <HkInput
                v-model={form.value.scopes}
                name="provider-scopes"
                placeholder="openid profile email"
                data-testid="provider-scopes"
              />
              <small class="hint">{t("common.providers.scopesHint")}</small>
            </label>
            <div class="field switch-field">
              <span>{t("common.providers.enabled")}</span>
              <HkSwitch v-model={form.value.enabled} aria-label={t("common.providers.enabled")} />
            </div>
            <button type="submit" hidden><Plus size={14} /></button>
          </form>
        </HkAdaptiveDialog>

        <HkAdaptiveDialog
          modelValue={pendingDelete.value !== null}
          title={t("common.actions.delete")}
          footerActions={deleteActions.value}
          width="28rem"
          onUpdate:modelValue={(v: boolean) => !v && ((pendingDelete.value = null))}
        >
          <p>{t("common.providers.deleteConfirm", { id: pendingDelete.value?.id ?? "" })}</p>
        </HkAdaptiveDialog>
      </section>
    );
  },
});
