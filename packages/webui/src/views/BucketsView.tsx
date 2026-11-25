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
import { Database, Pencil, Trash2 } from "lucide-vue-next";
import { computed, defineComponent, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import {
  deleteBucket,
  getDefaultBucket,
  listBuckets,
  setDefaultBucket,
  setGrant,
  upsertBucket,
  type BucketUpsert,
  type StorageBucket,
} from "@/api/buckets";
import { listUsers } from "@/api";
import type { UserDto } from "@/types";
import { usePageCreateAction } from "@/composables/usePageCreateAction";
import { usePageRefresh } from "@/composables/usePageRefresh";
import { makeErrorText } from "@/composables/errorText";

import "./BucketsView.scss";

/**
 * The bucket registry (admin): the roster, the strict upsert form, the
 * delete guard and the per-bucket grant editor.
 *
 * Contract pins (B-SPEC v5.1):
 *  - `storage.buckets.upsert` is a STRICT FULL DOCUMENT — the form sends
 *    exactly the four mutable fields (display_name / anonymous_read /
 *    grant_on_signup / enabled) with the slug, never a partial doc, so
 *    an omitted field can never silently reset (the server refuses it
 *    anyway; the wire is pinned by the view tests).
 *  - `grant_on_signup ⇒ anonymous_read` (R-A): the inconsistent
 *    combination is DISABLED client-side, not merely refused server-side.
 *  - Delete: a bootstrap bucket shows no affordance at all (the family
 *    convention — not a dead button); the server's three refusals
 *    (bootstrap / live rows / live key reference) surface as toasts.
 *  - Grants (R-A): exactly the three toggles {read, write, share}; an
 *    all-false save writes the explicit deny row (revocation); there is
 *    NO delete-row / reset-to-default affordance; admin targets are not
 *    offered (the server refuses them — shown as role-granted instead).
 */

/** The view's own keyed toast slots — one per operation. */
const UPSERT_TOAST = "buckets-upsert";
const DELETE_TOAST = "buckets-delete";
const GRANT_TOAST = "buckets-grant";
const DEFAULTS_TOAST = "buckets-defaults";

export default defineComponent({
  name: "BucketsView",
  setup() {
    const { t } = useI18n();
    const toast = useToast();

    const buckets = ref<StorageBucket[]>([]);
    const loading = ref(false);
    const error = ref<string | null>(null);
    const defaultSlug = ref<string | null>(null);
    const defaultBusy = ref(false);

    const errorText = makeErrorText(t);

    function clearToast(key: string): void {
      const slot = toast.toasts.find((item) => item.key === key);
      if (slot) toast.remove(slot.id);
    }

    async function load() {
      error.value = null;
      loading.value = true;
      try {
        buckets.value = (await listBuckets()).buckets;
        defaultSlug.value = (await getDefaultBucket()).bucket;
      } catch (e) {
        error.value = errorText(e);
      } finally {
        loading.value = false;
      }
    }
    onMounted(() => void load());
    usePageRefresh(() => load(), { loading });

    // ── upsert ─────────────────────────────────────────────────────────

    const editTarget = ref<StorageBucket | null>(null);
    const formSlug = ref("");
    const formDisplayName = ref("");
    const formAnonymousRead = ref(false);
    const formGrantOnSignup = ref(false);
    const formEnabled = ref(false);
    const formBusy = ref(false);

    /** The strict document — the EXACT wire shape (all four mutable
     *  fields present, slug identifying the row).
     *  MUT:p2a-upsert-strict-doc — assembling a partial document (dropping
     *  any field or sending the raw form state) reddens the wire test. */
    function formPayload(): BucketUpsert {
      return {
        slug: formSlug.value.trim(),
        display_name: formDisplayName.value.trim(),
        anonymous_read: formAnonymousRead.value,
        grant_on_signup: formGrantOnSignup.value,
        enabled: formEnabled.value,
      };
    }

    /** R-A's coupling, enforced in the FORM: default-open requires
     *  anonymous-readable. The switches are disabled in the inconsistent
     *  combination so the wire can never carry it.
     *  MUT:p2a-signup-coupling — breaking the coupling lets the submit
     *  enable (and the wire carry) grant_on_signup=true + anonymous_read=false. */
    // The R-A coupling, as LOCKS: a default-open bucket cannot go
    // anonymous-read-off (the switch is pinned ON while grant_on_signup
    // is true), and with anonymous_read off the signup switch cannot go
    // on. The inconsistent combination is unreachable through the form.
    const anonymousReadDisabled = computed(
      () => formGrantOnSignup.value && formAnonymousRead.value,
    );
    const grantOnSignupDisabled = computed(() => !formAnonymousRead.value);
    const formValid = computed(() => formSlug.value.trim() !== "");

    function openEdit(row: StorageBucket): void {
      clearToast(UPSERT_TOAST);
      editTarget.value = row;
      formSlug.value = row.slug;
      formDisplayName.value = row.display_name;
      formAnonymousRead.value = row.anonymous_read;
      formGrantOnSignup.value = row.grant_on_signup;
      formEnabled.value = row.enabled;
    }

    const createOpen = ref(false);

    function openCreate(): void {
      clearToast(UPSERT_TOAST);
      createOpen.value = true;
      editTarget.value = null;
      formSlug.value = "";
      formDisplayName.value = "";
      formAnonymousRead.value = true;
      formGrantOnSignup.value = true;
      formEnabled.value = true;
    }

    function closeEdit(): void {
      editTarget.value = null;
      createOpen.value = false;
    }

    async function doUpsert() {
      if (formBusy.value || !formValid.value) return;
      formBusy.value = true;
      clearToast(UPSERT_TOAST);
      try {
        await upsertBucket(formPayload());
        closeEdit();
        toast.show(t("common.buckets.saved"), { type: "success", key: UPSERT_TOAST });
        await load();
      } catch (e) {
        // The server's refusal (strict shape / R-A constraint) wins —
        // shown as it arrives; the dialog stays open for a retry.
        toast.show(errorText(e), { type: "error", key: UPSERT_TOAST });
      } finally {
        formBusy.value = false;
      }
    }

    const editActions = computed<ModalAction[]>(() => [
      { label: t("common.actions.cancel"), variant: "secondary", onClick: closeEdit },
      {
        label: t("common.actions.save"),
        variant: "primary",
        loading: formBusy.value,
        disabled: !formValid.value,
        onClick: () => void doUpsert(),
      },
    ]);

    usePageCreateAction({
      label: () => t("common.buckets.create"),
      onCreate: openCreate,
    });

    // ── delete ─────────────────────────────────────────────────────────

    const pendingDelete = ref<StorageBucket | null>(null);
    const deleteBusy = ref(false);

    async function doDelete() {
      const target = pendingDelete.value;
      if (!target || deleteBusy.value) return;
      deleteBusy.value = true;
      clearToast(DELETE_TOAST);
      try {
        await deleteBucket(target.slug);
        pendingDelete.value = null;
        toast.show(t("common.buckets.deleted"), { type: "success", key: DELETE_TOAST });
        await load();
      } catch (e) {
        // The server's three R-B refusals surface verbatim (the view's
        // own bootstrap check already kept this dialog closed for the
        // immutable bucket — this lane catches live rows / key refs).
        toast.show(errorText(e), { type: "error", key: DELETE_TOAST });
      } finally {
        deleteBusy.value = false;
      }
    }

    const deleteActions = computed<ModalAction[]>(() => [
      { label: t("common.actions.cancel"), variant: "secondary", onClick: () => (pendingDelete.value = null) },
      {
        label: t("common.actions.delete"),
        variant: "danger",
        loading: deleteBusy.value,
        onClick: () => void doDelete(),
      },
    ]);

    // ── the default upload bucket (R-F / R-I) ──────────────────────────

    const defaultOptions = computed(() =>
      buckets.value.map((b) => ({ value: b.slug, label: b.slug })),
    );

    async function onDefaultPicked(slug: string) {
      if (defaultBusy.value) return;
      defaultBusy.value = true;
      clearToast(DEFAULTS_TOAST);
      try {
        await setDefaultBucket(slug);
        defaultSlug.value = slug;
        toast.show(t("common.buckets.defaultSaved"), { type: "success", key: DEFAULTS_TOAST });
      } catch (e) {
        toast.show(errorText(e), { type: "error", key: DEFAULTS_TOAST });
      } finally {
        defaultBusy.value = false;
      }
    }

    // ── grants (R-A) ───────────────────────────────────────────────────

    const grantsTarget = ref<StorageBucket | null>(null);
    const members = ref<UserDto[]>([]);
    const grantBusyId = ref<string | null>(null);

    /** Local grant state per member — read/write/share, exactly the three
     *  bits the server stores. No fourth affordance exists (no delete,
     *  no reset-to-default: revocation IS the all-false save). */
    type GrantBits = { read: boolean; write: boolean; share: boolean };
    const grantBits = ref<Record<string, GrantBits>>({});

    async function openGrants(row: StorageBucket) {
      clearToast(GRANT_TOAST);
      grantsTarget.value = row;
      grantBits.value = {};
      try {
        const page = await listUsers(0, 100);
        // Admin users are ROLE-granted (R-A): no rows exist for them and
        // the server refuses writes — they are shown as a badge, not as
        // an editable row, so the refusal surface is never even reached.
        members.value = page.items.filter((u) => u.role !== "admin");
      } catch (e) {
        toast.show(errorText(e), { type: "error", key: GRANT_TOAST });
      }
    }

    function toggleBit(userId: string, bit: keyof GrantBits, on: boolean): void {
      const bits = grantBits.value[userId] ?? { read: false, write: false, share: false };
      grantBits.value = {
        ...grantBits.value,
        [userId]: { ...bits, [bit]: on },
      };
    }

    /** Save one member's row: the upsert writes all three bits — an
     *  all-false save is the EXPLICIT DENY row (revocation), never a
     *  row deletion. MUT:p2a-grant-all-false-kept — collapsing an
     *  all-false save into a "delete row" call reddens the payload test. */
    async function saveGrant(userId: string) {
      if (grantBusyId.value) return;
      const bits = grantBits.value[userId] ?? { read: false, write: false, share: false };
      grantBusyId.value = userId;
      clearToast(GRANT_TOAST);
      try {
        await setGrant(userId, grantsTarget.value!.slug, bits);
        toast.show(t("common.buckets.grantSaved"), { type: "success", key: GRANT_TOAST });
      } catch (e) {
        toast.show(errorText(e), { type: "error", key: GRANT_TOAST });
      } finally {
        grantBusyId.value = null;
      }
    }

    // ── roster columns ─────────────────────────────────────────────────

    const columns = computed<HTableColumn[]>(() => [
      { key: "slug", title: t("common.buckets.slug") },
      { key: "backend", title: t("common.buckets.backend"), copyable: false },
      { key: "flags", title: t("common.buckets.flags"), copyable: false },
      { key: "state", title: t("common.keys.state"), copyable: false },
      { key: "actions", title: "", align: "right", width: "8rem", copyable: false },
    ]);

    function backendLabel(backend: string): string {
      const key = `common.buckets.backend_${backend}`;
      const label = t(key);
      return label === key ? backend : label;
    }

    function renderFlags(row: StorageBucket) {
      return (
        <span class="bucket-flags">
          {row.anonymous_read ? (
            <HkBadge variant="info" size="sm">{t("common.buckets.anonymousRead")}</HkBadge>
          ) : null}
          {row.grant_on_signup ? (
            <HkBadge variant="success" size="sm">{t("common.buckets.grantOnSignup")}</HkBadge>
          ) : null}
          {row.is_bootstrap ? (
            <HkBadge variant="warning" size="sm">{t("common.buckets.bootstrap")}</HkBadge>
          ) : null}
        </span>
      );
    }

    function renderRowActions(row: StorageBucket) {
      return (
        <span class="bucket-row-actions">
          <HkIconButton
            size={24}
            variant="ghost"
            aria-label={t("common.buckets.grants")}
            {...{ title: t("common.buckets.grants") }}
            data-testid={`grants-${row.slug}`}
            onClick={() => void openGrants(row)}
          >
            {{ icon: () => <Database size={14} /> }}
          </HkIconButton>
          <HkIconButton
            size={24}
            variant="ghost"
            aria-label={t("common.buckets.edit")}
            {...{ title: t("common.buckets.edit") }}
            data-testid={`edit-${row.slug}`}
            onClick={() => openEdit(row)}
          >
            {{ icon: () => <Pencil size={14} /> }}
          </HkIconButton>
          {/* Bootstrap buckets are immutable (R-H): no affordance at all
              rather than a dead button (the family roster convention).
              MUT:p2a-bootstrap-no-delete — rendering the delete button
              (disabled or not) for a bootstrap bucket reddens this. */}
          {row.is_bootstrap ? null : (
            <HkIconButton
              size={24}
              variant="ghost"
              aria-label={t("common.actions.delete")}
              {...{ title: t("common.actions.delete") }}
              data-testid={`delete-${row.slug}`}
              onClick={() => { clearToast(DELETE_TOAST); pendingDelete.value = row; }}
            >
              {{ icon: () => <Trash2 size={14} /> }}
            </HkIconButton>
          )}
        </span>
      );
    }

    /** The grant editor's switch caption for one bit. */
    function grantLabel(bit: keyof GrantBits): string {
      return t(`common.buckets.grant_${bit}`);
    }

    return () => (
      <section class="buckets-page">
        <HkAdminTablePage
          title={t("common.buckets.title")}
          subtitle={t("common.status.total", { count: buckets.value.length })}
          countReadout={false}
          icon={Database}
          rows={buckets.value}
          columns={columns.value}
          rowKey="id"
          loading={loading.value}
          error={buckets.value.length === 0 ? (error.value ?? undefined) : undefined}
          staleError={buckets.value.length > 0 ? (error.value ?? undefined) : undefined}
          emptyTitle={t("common.buckets.empty")}
        >
          {{
            "cell-slug": ({ row }: { row: StorageBucket }) => (
              <span class="bucket-slug-cell">
                <span class="bucket-slug">{row.slug}</span>
                <span class="bucket-display">{row.display_name}</span>
              </span>
            ),
            "cell-backend": ({ row }: { row: StorageBucket }) => (
              <HkBadge variant="muted" mono size="sm">{backendLabel(row.backend)}</HkBadge>
            ),
            "cell-flags": ({ row }: { row: StorageBucket }) => renderFlags(row),
            "cell-state": ({ row }: { row: StorageBucket }) => (
              row.enabled
                ? <HkBadge variant="success" pill>{t("common.buckets.enabled")}</HkBadge>
                : <HkBadge variant="warning" pill>{t("common.buckets.disabled")}</HkBadge>
            ),
            "cell-actions": ({ row }: { row: StorageBucket }) => renderRowActions(row),
          }}
        </HkAdminTablePage>

        {/* The default upload bucket (admin, one select + save). */}
        <div class="bucket-defaults" data-testid="bucket-defaults">
          <span class="defaults-label">{t("common.buckets.defaultBucket")}</span>
          <HkSelect
            modelValue={defaultSlug.value ?? ""}
            options={defaultOptions.value}
            panelTitle={t("common.buckets.defaultBucket")}
            placeholder={t("common.buckets.defaultUnset")}
            data-testid="bucket-default-select"
            onUpdate:modelValue={(v: string) => { if (v) void onDefaultPicked(v); }}
          />
        </div>

        {/* The upsert window (create = the header plus; edit = per row).
            The four switches are the strict document's booleans. */}
        <HkAdaptiveDialog
          modelValue={editTarget.value !== null || createOpen.value}
          title={editTarget.value ? t("common.buckets.edit") : t("common.buckets.create")}
          width="26rem"
          footerActions={editActions.value}
          onUpdate:modelValue={(v: boolean) => { if (!v) closeEdit(); }}
        >
          <div class="dialog-body">
            <label class="field-label">
              <span>{t("common.buckets.slug")}</span>
              <HkInput
                v-model={formSlug.value}
                disabled={editTarget.value !== null}
                data-testid="bucket-slug"
              />
            </label>
            <label class="field-label">
              <span>{t("common.buckets.displayName")}</span>
              <HkInput v-model={formDisplayName.value} data-testid="bucket-display-name" />
            </label>
            <div class="switch-row">
              <span>{t("common.buckets.anonymousRead")}</span>
              <HkSwitch
                modelValue={formAnonymousRead.value}
                disabled={anonymousReadDisabled.value}
                ariaLabel={t("common.buckets.anonymousRead")}
                data-testid="bucket-anonymous-read"
                onUpdate:modelValue={(on: boolean) => (formAnonymousRead.value = on)}
              />
            </div>
            <div class="switch-row">
              <span>{t("common.buckets.grantOnSignup")}</span>
              <HkSwitch
                modelValue={formGrantOnSignup.value}
                disabled={grantOnSignupDisabled.value}
                ariaLabel={t("common.buckets.grantOnSignup")}
                data-testid="bucket-grant-on-signup"
                onUpdate:modelValue={(on: boolean) => {
                  // The R-A coupling lives HERE: flipping signup on while
                  // anonymous_read is off is impossible — the switch is
                  // disabled and the flip is refused outright.
                  if (on && !formAnonymousRead.value) return;
                  formGrantOnSignup.value = on;
                }}
              />
            </div>
            <div class="switch-row">
              <span>{t("common.buckets.enabled")}</span>
              <HkSwitch
                modelValue={formEnabled.value}
                ariaLabel={t("common.buckets.enabled")}
                data-testid="bucket-enabled"
                onUpdate:modelValue={(on: boolean) => (formEnabled.value = on)}
              />
            </div>
          </div>
        </HkAdaptiveDialog>

        {/* The delete confirm (non-bootstrap buckets only reach it). */}
        <HkAdaptiveDialog
          modelValue={pendingDelete.value !== null}
          title={t("common.actions.delete")}
          width="28rem"
          footerActions={deleteActions.value}
          onUpdate:modelValue={(v: boolean) => !v && (pendingDelete.value = null)}
        >
          <p>{t("common.buckets.deleteConfirm", { slug: pendingDelete.value?.slug ?? "" })}</p>
        </HkAdaptiveDialog>

        {/* The grant editor: exactly three toggles per member; the all-
            false save is the explicit deny row; admins are ROLE-granted
            and never offered as editable rows (R-A). */}
        <HkAdaptiveDialog
          modelValue={grantsTarget.value !== null}
          title={t("common.buckets.grantsTitle", { slug: grantsTarget.value?.slug ?? "" })}
          width="30rem"
          onUpdate:modelValue={(v: boolean) => !v && (grantsTarget.value = null)}
        >
          <div class="grant-editor" data-testid="grant-editor">
            {members.value.map((m) => (
              <div class="grant-row" key={m.id} data-testid={`grant-row-${m.id}`}>
                <span class="grant-name">{m.display_name || m.username}</span>
                {(["read", "write", "share"] as const).map((bit) => (
                  <label class="grant-bit" key={bit}>
                    <span>{grantLabel(bit)}</span>
                    <HkSwitch
                      modelValue={(grantBits.value[m.id] ?? { read: false, write: false, share: false })[bit]}
                      ariaLabel={`${m.username}: ${grantLabel(bit)}`}
                      data-testid={`grant-${m.id}-${bit}`}
                      onUpdate:modelValue={(on: boolean) => toggleBit(m.id, bit, on)}
                    />
                  </label>
                ))}
                <HkButton
                  variant="secondary"
                  size="sm"
                  disabled={grantBusyId.value !== null}
                  data-testid={`grant-save-${m.id}`}
                  onClick={() => void saveGrant(m.id)}
                >
                  {t("common.actions.save")}
                </HkButton>
              </div>
            ))}
            {members.value.length === 0 ? (
              <p class="grant-empty">{t("common.buckets.noMembers")}</p>
            ) : null}
            <p class="grant-hint">{t("common.buckets.grantHint")}</p>
          </div>
        </HkAdaptiveDialog>
      </section>
    );
  },
});
