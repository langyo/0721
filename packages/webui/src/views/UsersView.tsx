import { HkAdaptiveDialog, HkAdminTablePage, HkBadge, HkDateTag, HkIconButton, HkInput, HkRadio, type HTableColumn } from "@celestia-island/hikari";
import { ShieldCheck, Trash2, Users as UsersIcon } from "lucide-vue-next";
import { computed, defineComponent, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as api from "@/api";
import { ApiError } from "@/api/client";
import { usePageCreateAction } from "@/composables/usePageCreateAction";
import { usePageRefresh } from "@/composables/usePageRefresh";
import { useAuthStore } from "@/stores/auth";
import type { Role, UserDto } from "@/types";
import { makeErrorText } from "@/composables/errorText";

import "./UsersView.scss";

/**
 * The admin user roster: list / create / delete. The create action is the
 * header's plus (the slot law — the dialog itself stays page-owned), the
 * refresh the header's refresh button.
 */
export default defineComponent({
  name: "UsersView",
  setup() {
    const { t } = useI18n();
    const auth = useAuthStore();

    const PAGE_SIZE = 30;

    const users = ref<UserDto[]>([]);
    const total = ref(0);
    const loading = ref(false);
    const loadError = ref<string | null>(null);
    const hasMore = computed(() => users.value.length < total.value);

    // The never-empty failure text (shared factory; the contract and
    // its pins live in composables/errorText).
    const errorText = makeErrorText(t);

    async function load(reset: boolean) {
      // Re-entrancy guard: the paging offset below is read from the live
      // list, so two overlapping calls would append the same page twice.
      if (loading.value && !reset) return;
      loading.value = true;
      loadError.value = null;
      try {
        const offset = reset ? 0 : users.value.length;
        const page = await api.listUsers(offset, PAGE_SIZE);
        users.value = reset ? page.items : [...users.value, ...page.items];
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

    // ── create dialog ──────────────────────────────────────────────────

    const createOpen = ref(false);
    const createBusy = ref(false);
    const createError = ref<string | null>(null);
    const form = ref({ username: "", password: "", displayName: "", role: "member" as Role });

    /** The two account roles, as the family radio's {value,label} rows. */
    const roleOptions = computed<{ value: Role; label: string }[]>(() => [
      { value: "member", label: t("common.status.member") },
      { value: "admin", label: t("common.status.admin") },
    ]);

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
            : errorText(err);
      } finally {
        createBusy.value = false;
      }
    }

    // ── delete ─────────────────────────────────────────────────────────

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
        deleteError.value = errorText(err);
      } finally {
        deleteBusy.value = false;
      }
    }

    /** The roster's column set — the family table anatomy. `key` doubles as
     * the cell-slot name (`cell-<key>`), `copyable: false` on the action
     * column keeps click-to-copy off it. */
    const columns = computed<HTableColumn[]>(() => [
      { key: "name", title: t("common.users.account"), sortable: false },
      { key: "role", title: t("common.users.role"), align: "left" },
      { key: "created", title: t("common.users.created"), align: "left" },
      { key: "actions", title: "", align: "right", width: "3.5rem", copyable: false },
    ]);

    return () => (
      <section class="users">
        <HkAdminTablePage
          title={t("common.users.title")}
          subtitle={t("common.status.total", { count: total.value })}
          countReadout={false}
          icon={UsersIcon}
          rows={users.value}
          columns={columns.value}
          rowKey="id"
          loading={loading.value}
          error={users.value.length === 0 ? (loadError.value ?? undefined) : undefined}
          staleError={users.value.length > 0 ? (loadError.value ?? undefined) : undefined}
          emptyTitle={t("common.users.empty")}
        >
          {{
            "cell-name": ({ row }: { row: UserDto }) => (
              <span class="user-name-cell">
                <span class="user-name">{row.display_name}</span>
                <span class="user-login">@{row.username}</span>
              </span>
            ),

            "cell-role": ({ row }: { row: UserDto }) => (
              <HkBadge variant={row.role === "admin" ? "warning" : "muted"} pill>
                {row.role === "admin" ? <ShieldCheck size={12} /> : null}
                {row.role === "admin" ? t("common.status.admin") : t("common.status.member")}
              </HkBadge>
            ),

            "cell-created": ({ row }: { row: UserDto }) => (
              <HkDateTag value={row.created_at} time seconds />
            ),

            "cell-actions": ({ row }: { row: UserDto }) => (
              // Deleting yourself would drop the admin surface mid-session:
              // the family roster convention is no affordance at all rather
              // than a dead button.
              row.id !== auth.user?.id ? (
                <HkIconButton
                  size={24}
                  variant="ghost"
                  aria-label={t("common.actions.delete")}
                  {...{ title: t("common.actions.delete") }}
                  onClick={() => { pendingDelete.value = row; deleteError.value = null; }}
                >
                  {{ icon: () => <Trash2 size={14} /> }}
                </HkIconButton>
              ) : null
            ),
          }}
        </HkAdminTablePage>

        {hasMore.value ? (
          <div class="users-more">
            <HkIconButton
              size={28}
              variant="ghost"
              disabled={loading.value}
              aria-label={t("common.actions.loadMore")}
              {...{ title: t("common.actions.loadMore") }}
              onClick={() => load(false)}
            >
              {{ icon: () => <UsersIcon size={16} /> }}
            </HkIconButton>
          </div>
        ) : null}

        <HkAdaptiveDialog
          modelValue={createOpen.value}
          title={t("common.users.createTitle")}
          footerActions={createActions.value}
          width="26rem"
          onUpdate:modelValue={(v: boolean) => (createOpen.value = v)}
        >
          <form class="create-form" onSubmit={(e: Event) => { e.preventDefault(); submitCreate(); }}>
            <label class="field">
              <span>{t("common.users.username")}</span>
              <HkInput v-model={form.value.username} name="username" autocomplete="off" data-testid="create-username" />
            </label>
            <label class="field">
              <span>{t("common.users.password")}</span>
              <HkInput v-model={form.value.password} name="password" type="password" data-testid="create-password" />
            </label>
            <label class="field">
              <span>{t("common.users.displayName")}</span>
              <HkInput v-model={form.value.displayName} name="display-name" autocomplete="off" />
            </label>
            <div class="field">
              <span>{t("common.users.role")}</span>
              <div role="radiogroup" aria-label={t("common.users.role")}>
                <HkRadio v-model={form.value.role} options={roleOptions.value} size="sm" />
              </div>
            </div>
            {createError.value ? <p class="dialog-error" role="alert">{createError.value}</p> : null}
          </form>
        </HkAdaptiveDialog>

        <HkAdaptiveDialog
          modelValue={pendingDelete.value !== null}
          title={t("common.actions.delete")}
          footerActions={deleteActions.value}
          width="28rem"
          onUpdate:modelValue={(v: boolean) => !v && ((pendingDelete.value = null))}
        >
          <p>{t("common.users.deleteConfirm", { name: pendingDelete.value?.display_name ?? "" })}</p>
          {deleteError.value ? <p class="dialog-error" role="alert">{deleteError.value}</p> : null}
        </HkAdaptiveDialog>
      </section>
    );
  },
});
