<script setup lang="ts">
import {
  HkAdaptiveDialog,
  HkAdminTablePage,
  HkBadge,
  HkIconButton,
  HkInput,
  HkSwitch,
  type HTableColumn,
} from "@celestia-island/hikari";
import { Pencil, Plus, Trash2, Unplug } from "lucide-vue-next";
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as api from "@/api";
import { usePageCreateAction } from "@/composables/usePageCreateAction";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { OAuthProviderPublic } from "@/types";

/**
 * The admin login-provider registry: list / create / edit / enable /
 * delete. The client secret is write-only — the server answers with
 * `has_secret` + the last four characters, and an EMPTY secret field
 * on save keeps the stored one.
 */

const { t } = useI18n();

const rows = ref<OAuthProviderPublic[]>([]);
const loading = ref(false);
const loadError = ref<string | null>(null);
/** A toggle failure's visible message (the enable/disable lane's own
 *  error row — a silent rollback reads as success). */
const toggleError = ref<string | null>(null);

const PROVIDER_TYPES = ["builtin:github", "builtin:feishu", "builtin:erp", "oidc"] as const;

async function load() {
  loading.value = true;
  loadError.value = null;
  try {
    rows.value = await api.listProviders();
  } catch (err) {
    loadError.value = err instanceof Error ? err.message : t("common.status.error");
  } finally {
    loading.value = false;
  }
}

onMounted(() => {
  void load();
});

usePageRefresh(() => load(), { loading });

// ── the edit dialog ────────────────────────────────────────────────────

const dialogOpen = ref(false);
const saving = ref(false);
const saveError = ref<string | null>(null);
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
  saveError.value = null;
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
  saveError.value = null;
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
  saveError.value = null;
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
    saveError.value = err instanceof Error ? err.message : t("common.status.error");
  } finally {
    saving.value = false;
  }
}

// ── enable toggle + delete ─────────────────────────────────────────────

async function toggle(row: OAuthProviderPublic, enabled: boolean) {
  const previous = rows.value;
  toggleError.value = null;
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
    toggleError.value = err instanceof Error ? err.message : t("common.status.error");
  }
}

const pendingDelete = ref<OAuthProviderPublic | null>(null);
const deleteBusy = ref(false);
const deleteError = ref<string | null>(null);

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
  deleteError.value = null;
  try {
    await api.deleteProvider(target.id);
    rows.value = rows.value.filter((r) => r.id !== target.id);
    pendingDelete.value = null;
  } catch (err) {
    deleteError.value = err instanceof Error ? err.message : t("common.status.error");
  } finally {
    deleteBusy.value = false;
  }
}

/** The roster's column set (the family table anatomy); `key` doubles as
 * the cell-slot name. The secret column is copy-exempt (it renders a
 * mask, never the value). */
const columns = computed<HTableColumn[]>(() => [
  { key: "name", title: t("common.providers.title") },
  { key: "secret", title: t("common.providers.secret"), copyable: false },
  { key: "enabled", title: t("common.providers.enabled"), align: "center", copyable: false },
  { key: "actions", title: "", align: "right", width: "6rem", copyable: false },
]);
</script>

<template>
  <section class="providers">
    <p v-if="toggleError" class="toggle-error" role="alert">{{ toggleError }}</p>
    <HkAdminTablePage
      :title="t('common.providers.title')"
      :subtitle="t('common.status.total', { count: rows.length })"
      :icon="Unplug"
      :rows="rows"
      :columns="columns"
      row-key="id"
      :loading="loading"
      :error="loadError ?? undefined"
      :empty-title="t('common.providers.empty')"
    >
      <template #cell-name="{ row }">
        <button type="button" class="provider-name-cell" @click="openEdit(row)">
          <span class="provider-name">{{ row.display_name }}</span>
          <span class="provider-meta">{{ row.id }} · {{ row.type }}</span>
        </button>
      </template>

      <template #cell-secret="{ row }">
        <HkBadge v-if="row.has_secret" variant="muted" mono pill>{{ `••••${row.secret_tail}` }}</HkBadge>
        <HkBadge v-else variant="warning" pill>{{ t("common.providers.noSecret") }}</HkBadge>
      </template>

      <template #cell-enabled="{ row }">
        <HkSwitch
          :model-value="row.enabled"
          :aria-label="t('common.providers.enabled')"
          @update:model-value="(v: boolean) => toggle(row, v)"
        />
      </template>

      <template #cell-actions="{ row }">
        <span class="provider-actions">
          <HkIconButton
            :size="24"
            variant="ghost"
            :aria-label="t('common.providers.edit')"
            :title="t('common.providers.edit')"
            @click="openEdit(row)"
          >
            <template #icon><Pencil :size="14" /></template>
          </HkIconButton>
          <HkIconButton
            :size="24"
            variant="ghost"
            :aria-label="t('common.actions.delete')"
            :title="t('common.actions.delete')"
            @click="((pendingDelete = row), (deleteError = null))"
          >
            <template #icon><Trash2 :size="14" /></template>
          </HkIconButton>
        </span>
      </template>
    </HkAdminTablePage>

    <HkAdaptiveDialog
      :model-value="dialogOpen"
      :title="editingId ? editingId : t('common.providers.create')"
      :footer-actions="dialogActions"
      width="30rem"
      @update:model-value="(v: boolean) => (dialogOpen = v)"
    >
      <form class="edit-form" @submit.prevent="save">
        <label class="field">
          <span>{{ t("common.providers.id") }}</span>
          <HkInput v-model="form.id" name="provider-id" autocomplete="off" :disabled="editingId !== null" data-testid="provider-id" />
        </label>
        <label class="field">
          <span>{{ t("common.providers.displayName") }}</span>
          <HkInput v-model="form.display_name" name="provider-name" />
        </label>
        <div class="field">
          <span>{{ t("common.providers.type") }}</span>
          <div class="type-picker" role="radiogroup">
            <button
              v-for="kind in PROVIDER_TYPES"
              :key="kind"
              type="button"
              role="radio"
              :aria-checked="form.provider_type === kind"
              class="type-option"
              :class="{ active: form.provider_type === kind }"
              @click="form.provider_type = kind"
            >
              {{ kind }}
            </button>
          </div>
        </div>
        <label class="field">
          <span>{{ t("common.providers.clientId") }}</span>
          <HkInput v-model="form.client_id" name="provider-client-id" autocomplete="off" />
        </label>
        <label class="field">
          <span>{{ t("common.providers.clientSecret") }}</span>
          <HkInput v-model="form.client_secret" name="provider-secret" type="password" autocomplete="new-password" />
          <small class="hint">{{ t("common.providers.secretHint") }}</small>
        </label>
        <label v-if="form.provider_type === 'builtin:erp'" class="field">
          <span>{{ t("common.providers.baseUrl") }}</span>
          <HkInput v-model="form.base_url" name="provider-base-url" placeholder="https://auth.example.com" />
        </label>
        <label v-if="form.provider_type === 'oidc'" class="field">
          <span>{{ t("common.providers.issuerUrl") }}</span>
          <HkInput v-model="form.issuer_url" name="provider-issuer" placeholder="https://sso.example.com" />
        </label>
        <label class="field">
          <span>{{ t("common.providers.scopes") }}</span>
          <HkInput
            v-model="form.scopes"
            name="provider-scopes"
            placeholder="openid profile email"
            data-testid="provider-scopes"
          />
          <small class="hint">{{ t("common.providers.scopesHint") }}</small>
        </label>
        <div class="field switch-field">
          <span>{{ t("common.providers.enabled") }}</span>
          <HkSwitch v-model="form.enabled" :aria-label="t('common.providers.enabled')" />
        </div>
        <p v-if="saveError" class="dialog-error" role="alert">{{ saveError }}</p>
        <button type="submit" hidden><Plus :size="14" /></button>
      </form>
    </HkAdaptiveDialog>

    <HkAdaptiveDialog
      :model-value="pendingDelete !== null"
      :title="t('common.actions.delete')"
      :footer-actions="deleteActions"
      width="28rem"
      @update:model-value="(v: boolean) => !v && ((pendingDelete = null))"
    >
      <p>{{ t("common.providers.deleteConfirm", { id: pendingDelete?.id ?? "" }) }}</p>
      <p v-if="deleteError" class="dialog-error" role="alert">{{ deleteError }}</p>
    </HkAdaptiveDialog>
  </section>
</template>

<style scoped lang="scss">
.providers {
  display: flex;
  flex-direction: column;
  gap: 1rem;
}

.edit-form {
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
}

.field {
  display: flex;
  flex-direction: column;
  gap: 0.3rem;
  font-size: 0.85rem;
}

.hint {
  opacity: 0.65;
  font-size: 0.72rem;
}

.switch-field {
  flex-direction: row;
  align-items: center;
  justify-content: space-between;
}

.type-picker {
  display: flex;
  flex-wrap: wrap;
  gap: 0.4rem;
}

.type-option {
  border: 1px solid color-mix(in srgb, currentColor 22%, transparent);
  background: transparent;
  color: inherit;
  padding: 0.3rem 0.7rem;
  border-radius: 999px;
  cursor: pointer;
  font-size: 0.75rem;

  &.active {
    background: color-mix(in srgb, rgb(13 148 136) 20%, transparent);
    border-color: rgb(13 148 136);
  }
}

.dialog-error,
.toggle-error {
  color: rgb(220 38 38);
  font-size: 0.85rem;
}

.toggle-error {
  margin: 0;
  padding: 0.5rem 0.75rem;
  border-radius: 0.5rem;
  background: color-mix(in srgb, rgb(220 38 38) 12%, transparent);
}
.provider-name-cell { display: flex; flex-direction: column; gap: 0.15rem; text-align: start; background: none; border: 0; color: inherit; cursor: pointer; padding: 0; }
.provider-name { font-weight: 600; }
.provider-meta { opacity: 0.65; font-size: 0.85em; }
.provider-actions { display: inline-flex; gap: 0.25rem; }
</style>
