import {
  HkAdaptiveDialog,
  HkAdminTablePage,
  HkBadge,
  HkButton,
  HkIconButton,
  HkInput,
  HkSelect,
  HkSwitch,
  useToast,
  type HTableColumn,
  type ModalAction,
} from "@celestia-island/hikari";
import { PackageOpen, ScanSearch } from "lucide-vue-next";
import { computed, defineComponent, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import {
  adoptUnregistered,
  getUnregisteredShow,
  listUnregistered,
  setUnregisteredShow,
  type UnregisteredPage,
} from "@/api/unregistered";
import { listBuckets, type StorageBucket } from "@/api/buckets";
import { listUsers } from "@/api";
import type { UserDto } from "@/types";
import { usePageRefresh } from "@/composables/usePageRefresh";
import { makeErrorText } from "@/composables/errorText";
import { formatSize } from "@/composables/formatSize";

import "./UnregisteredView.scss";

/**
 * The unregistered-object page (admin, read-only + adopt — P3):
 *  - the R-E show switch (`storage.unregistered.get/put`): OFF (the
 *    default) means the whole lane is invisible — even this page's list
 *    calls are refused by the server;
 *  - a per-bucket listing of blobs with no `files` row
 *    (`storage.unregistered.list`);
 *  - the ADOPT action (the only write; there is NO delete): owner
 *    picker, optional parent folder id and display name;
 *  - the "backend cannot be scanned" notice for ipfs.
 */
const ADOPT_TOAST = "unreg-adopt";
const SHOW_TOAST = "unreg-show";

export default defineComponent({
  name: "UnregisteredView",
  setup() {
    const { t } = useI18n();
    const toast = useToast();
    const errorText = makeErrorText(t);

    const buckets = ref<StorageBucket[]>([]);
    const chosen = ref<string>("");
    const show = ref(false);
    const showBusy = ref(false);
    const loading = ref(false);
    const error = ref<string | null>(null);
    const page = ref<UnregisteredPage | null>(null);
    const unsupported = ref(false);

    function clearToast(key: string): void {
      const slot = toast.toasts.find((item) => item.key === key);
      if (slot) toast.remove(slot.id);
    }

    async function loadPage() {
      error.value = null;
      unsupported.value = false;
      page.value = null;
      if (!chosen.value) return;
      loading.value = true;
      try {
        page.value = await listUnregistered(chosen.value, 0, 200);
      } catch (e) {
        const text = errorText(e);
        if (text.includes("cannot be scanned")) {
          unsupported.value = true;
        } else {
          error.value = text;
        }
      } finally {
        loading.value = false;
      }
    }

    async function load() {
      try {
        buckets.value = (await listBuckets()).buckets;
        show.value = (await getUnregisteredShow()).show;
        // The first READY bucket is the preselection (the admin narrows
        // from there) — never "the first bucket" as a DEFAULT UPLOAD
        // decision; this is a listing scope only.
        chosen.value = chosen.value || buckets.value[0]?.slug || "";
        await loadPage();
      } catch (e) {
        error.value = errorText(e);
      }
    }
    onMounted(() => void load());
    usePageRefresh(() => load(), { loading });

    async function onShowPicked(on: boolean) {
      if (showBusy.value) return;
      showBusy.value = true;
      clearToast(SHOW_TOAST);
      try {
        await setUnregisteredShow(on);
        show.value = on;
        if (on) await loadPage();
      } catch (e) {
        toast.show(errorText(e), { type: "error", key: SHOW_TOAST });
      } finally {
        showBusy.value = false;
      }
    }

    // ── adopt ───────────────────────────────────────────────────────────
    const adoptTarget = ref<string | null>(null);
    const members = ref<UserDto[]>([]);
    const formOwnerId = ref("");
    const formParentId = ref("");
    const formName = ref("");
    const formBusy = ref(false);

    async function openAdopt(key: string) {
      clearToast(ADOPT_TOAST);
      adoptTarget.value = key;
      formOwnerId.value = "";
      formParentId.value = "";
      formName.value = "";
      try {
        const users = await listUsers(0, 200);
        members.value = users.items.filter((u) => u.role !== "admin");
      } catch {
        members.value = [];
      }
    }

    const memberOptions = computed(() =>
      members.value.map((m) => ({ value: m.id, label: m.display_name || m.username })),
    );

    async function doAdopt() {
      const key = adoptTarget.value;
      if (!key || formBusy.value) return;
      formBusy.value = true;
      clearToast(ADOPT_TOAST);
      try {
        await adoptUnregistered({
          bucket: chosen.value,
          key,
          owner_id: formOwnerId.value || undefined,
          parent_id: formParentId.value || undefined,
          name: formName.value || undefined,
        });
        adoptTarget.value = null;
        toast.show(t("common.unregistered.adopted"), { type: "success", key: ADOPT_TOAST });
        await loadPage();
      } catch (e) {
        toast.show(errorText(e), { type: "error", key: ADOPT_TOAST });
      } finally {
        formBusy.value = false;
      }
    }

    const adoptActions = computed<ModalAction[]>(() => [
      { label: t("common.actions.cancel"), variant: "secondary", onClick: () => (adoptTarget.value = null) },
      {
        label: t("common.unregistered.adopt"),
        variant: "primary",
        loading: formBusy.value,
        onClick: () => void doAdopt(),
      },
    ]);

    // ── columns ─────────────────────────────────────────────────────────
    const columns = computed<HTableColumn[]>(() => [
      { key: "key", title: t("common.unregistered.key"), copyable: true },
      { key: "size", title: t("common.unregistered.size"), copyable: false },
      { key: "mime", title: t("common.unregistered.mime"), copyable: false },
      { key: "actions", title: "", align: "right", width: "6rem", copyable: false },
    ]);

    const bucketOptions = computed(() =>
      buckets.value.map((b) => ({ value: b.slug, label: b.slug })),
    );

    function renderRowActions(row: { key: string }) {
      return (
        <HkIconButton
          size={24}
          variant="ghost"
          aria-label={t("common.unregistered.adopt")}
          {...{ title: t("common.unregistered.adopt") }}
          data-testid={`adopt-${row.key}`}
          onClick={() => void openAdopt(row.key)}
        >
          {{ icon: () => <PackageOpen size={14} /> }}
        </HkIconButton>
      );
    }

    return () => (
      <section class="unregistered-page">
        <div class="unreg-controls" data-testid="unreg-controls">
          <div class="switch-row">
            <span>{t("common.unregistered.show")}</span>
            <HkSwitch
              modelValue={show.value}
              disabled={showBusy.value}
              ariaLabel={t("common.unregistered.show")}
              data-testid="unreg-show"
              onUpdate:modelValue={(on: boolean) => void onShowPicked(on)}
            />
          </div>
          <HkSelect
            modelValue={chosen.value}
            options={bucketOptions.value}
            panelTitle={t("common.unregistered.pickBucket")}
            placeholder={t("common.unregistered.pickBucket")}
            data-testid="unreg-bucket-select"
            onUpdate:modelValue={(v: string) => {
              chosen.value = v;
              void loadPage();
            }}
          />
          <HkIconButton
            size={24}
            variant="ghost"
            aria-label={t("common.actions.refresh")}
            {...{ title: t("common.actions.refresh") }}
            data-testid="unreg-rescan"
            onClick={() => void loadPage()}
          >
            {{ icon: () => <ScanSearch size={14} /> }}
          </HkIconButton>
        </div>

        {unsupported.value ? (
          <HkBadge variant="warning" size="md" data-testid="unreg-unsupported">
            {t("common.unregistered.unsupported")}
          </HkBadge>
        ) : null}

        <HkAdminTablePage
          title={t("common.unregistered.title")}
          subtitle={t("common.status.total", { count: page.value?.total ?? 0 })}
          countReadout={false}
          icon={PackageOpen}
          rows={page.value?.items ?? []}
          columns={columns.value}
          rowKey="key"
          loading={loading.value}
          error={error.value ?? undefined}
          emptyTitle={t("common.unregistered.empty")}
        >
          {{
            "cell-key": ({ row }: { row: { key: string } }) => (
              <span class="unreg-key">{row.key}</span>
            ),
            "cell-size": ({ row }: { row: { size: number } }) => formatSize(row.size),
            "cell-mime": ({ row }: { row: { hinted_mime: string } }) => (
              <HkBadge variant="muted" mono size="sm">{row.hinted_mime}</HkBadge>
            ),
            "cell-actions": ({ row }: { row: { key: string } }) => renderRowActions(row),
          }}
        </HkAdminTablePage>

        <HkAdaptiveDialog
          modelValue={adoptTarget.value !== null}
          title={t("common.unregistered.adoptTitle")}
          width="28rem"
          footerActions={adoptActions.value}
          onUpdate:modelValue={(v: boolean) => !v && (adoptTarget.value = null)}
        >
          <div class="dialog-body" data-testid="unreg-adopt-dialog">
            <p class="adopt-key">{adoptTarget.value}</p>
            <label class="field-label">
              <span>{t("common.unregistered.owner")}</span>
              <HkSelect
                modelValue={formOwnerId.value}
                options={memberOptions.value}
                panelTitle={t("common.unregistered.owner")}
                placeholder={t("common.unregistered.ownerSelf")}
                data-testid="unreg-owner-select"
                onUpdate:modelValue={(v: string) => (formOwnerId.value = v)}
              />
            </label>
            <label class="field-label">
              <span>{t("common.unregistered.parent")}</span>
              <HkInput
                v-model={formParentId.value}
                placeholder={t("common.unregistered.parentHint")}
                data-testid="unreg-parent"
              />
            </label>
            <label class="field-label">
              <span>{t("common.unregistered.name")}</span>
              <HkInput
                v-model={formName.value}
                placeholder={t("common.unregistered.nameHint")}
                data-testid="unreg-name"
              />
            </label>
            <HkButton
              variant="secondary"
              size="sm"
              disabled={formBusy.value}
              data-testid="unreg-adopt-submit"
              onClick={() => void doAdopt()}
            >
              {t("common.unregistered.adopt")}
            </HkButton>
          </div>
        </HkAdaptiveDialog>
      </section>
    );
  },
});
