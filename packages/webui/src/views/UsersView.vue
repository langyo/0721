<script setup lang="ts">
import {
  HkAdaptiveDialog,
  HkEmptyState,
  HkInput,
} from "@celestia-island/hikari";
import { ShieldCheck, Trash2, Users as UsersIcon } from "lucide-vue-next";
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as api from "@/api";
import { ApiError } from "@/api/client";
import { usePageCreateAction } from "@/composables/usePageCreateAction";
import { usePageRefresh } from "@/composables/usePageRefresh";
import { useAuthStore } from "@/stores/auth";
import type { Role, UserDto } from "@/types";

/**
 * The admin user roster: list / create / delete. The create action is the
 * header's plus (the slot law — the dialog itself stays page-owned), the
 * refresh the header's refresh button.
 */

const { t } = useI18n();
const auth = useAuthStore();

const PAGE_SIZE = 30;

const users = ref<UserDto[]>([]);
const total = ref(0);
const loading = ref(false);
const loadError = ref<string | null>(null);
const hasMore = computed(() => users.value.length < total.value);

async function load(reset: boolean) {
  loading.value = true;
  loadError.value = null;
  try {
    const offset = reset ? 0 : users.value.length;
    const page = await api.listUsers(offset, PAGE_SIZE);
    users.value = reset ? page.items : [...users.value, ...page.items];
    total.value = page.total;
  } catch (err) {
    loadError.value = err instanceof Error ? err.message : t("common.status.error");
  } finally {
    loading.value = false;
  }
}

onMounted(() => {
  void load(true);
});

usePageRefresh(() => load(true), { loading });

// ── create dialog ──────────────────────────────────────────────────────

const createOpen = ref(false);
const createBusy = ref(false);
const createError = ref<string | null>(null);
const form = ref({ username: "", password: "", displayName: "", role: "member" as Role });

const createActions = computed(() => [
  {
    label: t("common.actions.cancel"),
    variant: "secondary" as const,
    onClick: () => {
      createOpen.value = false;
    },
  },
  {
    label: t("common.actions.create"),
    variant: "primary" as const,
    loading: createBusy.value,
    disabled: form.value.username.length < 3 || form.value.password.length < 1,
    onClick: () => {
      void submitCreate();
    },
  },
]);

usePageCreateAction({
  label: () => t("common.users.createTitle"),
  onCreate: () => {
    createError.value = null;
    createOpen.value = true;
  },
});

async function submitCreate() {
  createBusy.value = true;
  createError.value = null;
  try {
    await api.createUser({
      username: form.value.username,
      password: form.value.password,
      display_name: form.value.displayName ? form.value.displayName : null,
      role: form.value.role,
    });
    createOpen.value = false;
    form.value = { username: "", password: "", displayName: "", role: "member" };
    await load(true);
  } catch (err) {
    createError.value =
      err instanceof ApiError && err.status === 409
        ? t("common.users.conflict")
        : err instanceof Error
          ? err.message
          : t("common.status.error");
  } finally {
    createBusy.value = false;
  }
}

// ── delete ─────────────────────────────────────────────────────────────

const pendingDelete = ref<UserDto | null>(null);
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
    disabled: pendingDelete.value?.id === auth.user?.id,
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
    await api.deleteUser(target.id);
    users.value = users.value.filter((u) => u.id !== target.id);
    total.value = Math.max(0, total.value - 1);
    pendingDelete.value = null;
  } catch (err) {
    deleteError.value = err instanceof Error ? err.message : t("common.status.error");
  } finally {
    deleteBusy.value = false;
  }
}

function formatWhen(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}
</script>

<template>
  <section class="users">
    <HkEmptyState v-if="loading && users.length === 0" loading boxed :title="t('common.status.loading')" />
    <HkEmptyState v-else-if="loadError" boxed :title="t('common.status.error')" :description="loadError" />
    <HkEmptyState
      v-else-if="users.length === 0"
      boxed
      :icon="UsersIcon"
      :title="t('common.users.empty')"
    />

    <template v-else>
      <p class="users-count">{{ t("common.status.total", { count: total }) }}</p>
      <ul class="user-list">
        <li v-for="user in users" :key="user.id" class="user-row">
          <div class="user-avatar" :class="{ admin: user.role === 'admin' }">
            <ShieldCheck v-if="user.role === 'admin'" :size="16" />
            <UsersIcon v-else :size="16" />
          </div>
          <div class="user-main">
            <span class="user-name">{{ user.display_name }}</span>
            <span class="user-login">@{{ user.username }}</span>
          </div>
          <span class="user-role">{{ user.role === "admin" ? t("common.status.admin") : t("common.status.member") }}</span>
          <span class="user-created" :title="t('common.users.created')">{{ formatWhen(user.created_at) }}</span>
          <button
            type="button"
            class="user-delete"
            :aria-label="t('common.actions.delete')"
            :title="t('common.actions.delete')"
            @click="((pendingDelete = user), (deleteError = null))"
          >
            <Trash2 :size="14" />
          </button>
        </li>
      </ul>
      <div v-if="hasMore" class="users-more">
        <button type="button" class="users-more-btn" :disabled="loading" @click="load(false)">
          {{ t("common.actions.loadMore") }}
        </button>
      </div>
    </template>

    <HkAdaptiveDialog
      :model-value="createOpen"
      :title="t('common.users.createTitle')"
      :footer-actions="createActions"
      width="26rem"
      @update:model-value="(v: boolean) => (createOpen = v)"
    >
      <form class="create-form" @submit.prevent="submitCreate">
        <label class="field">
          <span>{{ t("common.users.username") }}</span>
          <HkInput v-model="form.username" name="username" autocomplete="off" data-testid="create-username" />
        </label>
        <label class="field">
          <span>{{ t("common.users.password") }}</span>
          <HkInput v-model="form.password" name="password" type="password" data-testid="create-password" />
        </label>
        <label class="field">
          <span>{{ t("common.users.displayName") }}</span>
          <HkInput v-model="form.displayName" name="display-name" autocomplete="off" />
        </label>
        <div class="field">
          <span>{{ t("common.users.role") }}</span>
          <div class="role-picker" role="radiogroup" :aria-label="t('common.users.role')">
            <button
              v-for="role in (['member', 'admin'] as Role[])"
              :key="role"
              type="button"
              role="radio"
              :aria-checked="form.role === role"
              class="role-option"
              :class="{ active: form.role === role }"
              @click="form.role = role"
            >
              {{ role === "admin" ? t("common.status.admin") : t("common.status.member") }}
            </button>
          </div>
        </div>
        <p v-if="createError" class="dialog-error" role="alert">{{ createError }}</p>
      </form>
    </HkAdaptiveDialog>

    <HkAdaptiveDialog
      :model-value="pendingDelete !== null"
      :title="t('common.actions.delete')"
      :footer-actions="deleteActions"
      width="28rem"
      @update:model-value="(v: boolean) => !v && ((pendingDelete = null))"
    >
      <p>{{ t("common.users.deleteConfirm", { name: pendingDelete?.display_name ?? "" }) }}</p>
      <p v-if="deleteError" class="dialog-error" role="alert">{{ deleteError }}</p>
    </HkAdaptiveDialog>
  </section>
</template>

<style scoped lang="scss">
.users {
  display: flex;
  flex-direction: column;
  gap: 1rem;
}

.users-count {
  margin: 0;
  opacity: 0.75;
  font-size: 0.875rem;
}

.user-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.user-row {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  padding: 0.6rem 0.75rem;
  border: 1px solid color-mix(in srgb, currentColor 16%, transparent);
  border-radius: 0.75rem;
  background: color-mix(in srgb, currentColor 4%, transparent);
}

.user-avatar {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 2rem;
  height: 2rem;
  border-radius: 999px;
  background: color-mix(in srgb, currentColor 10%, transparent);
  opacity: 0.8;

  &.admin {
    background: color-mix(in srgb, rgb(13 148 136) 22%, transparent);
  }
}

.user-main {
  display: flex;
  flex-direction: column;
  min-width: 0;
  flex: 1;
}

.user-name {
  font-weight: 600;
  font-size: 0.9rem;
}

.user-login {
  font-size: 0.75rem;
  opacity: 0.6;
}

.user-role {
  font-size: 0.75rem;
  padding: 0.15rem 0.6rem;
  border-radius: 999px;
  background: color-mix(in srgb, currentColor 8%, transparent);
}

.user-created {
  font-size: 0.75rem;
  opacity: 0.6;
}

.user-delete {
  border: none;
  background: transparent;
  color: inherit;
  opacity: 0.6;
  cursor: pointer;
  padding: 0.35rem;
  border-radius: 0.4rem;
  display: inline-flex;

  &:hover {
    opacity: 1;
    background: color-mix(in srgb, rgb(220 38 38) 16%, transparent);
    color: rgb(220 38 38);
  }
}

.users-more {
  display: flex;
  justify-content: center;
}

.users-more-btn {
  border: 1px solid color-mix(in srgb, currentColor 22%, transparent);
  background: transparent;
  color: inherit;
  padding: 0.45rem 1.4rem;
  border-radius: 999px;
  cursor: pointer;

  &:hover:not(:disabled) {
    background: color-mix(in srgb, currentColor 8%, transparent);
  }

  &:disabled {
    opacity: 0.5;
    cursor: default;
  }
}

.create-form {
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

.role-picker {
  display: inline-flex;
  gap: 0.4rem;
}

.role-option {
  border: 1px solid color-mix(in srgb, currentColor 22%, transparent);
  background: transparent;
  color: inherit;
  padding: 0.35rem 1rem;
  border-radius: 999px;
  cursor: pointer;
  font-size: 0.82rem;

  &.active {
    background: color-mix(in srgb, rgb(13 148 136) 20%, transparent);
    border-color: rgb(13 148 136);
  }
}

.dialog-error {
  color: rgb(220 38 38);
  font-size: 0.85rem;
}
</style>
