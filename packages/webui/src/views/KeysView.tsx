import {
  HkAdaptiveDialog,
  HkAdminTablePage,
  HkBadge,
  HkButton,
  HkIconButton,
  HkInput,
  HkSwitch,
  useToast,
  type HTableColumn,
  type ModalAction,
} from "@celestia-island/hikari";
import { Copy, KeyRound, Pencil, Trash2 } from "lucide-vue-next";
import { computed, defineComponent, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import * as api from "@/api";
import { usePageCreateAction } from "@/composables/usePageCreateAction";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { ApiKeyCreated, ApiKeyPublic, KeyWrite } from "@/api";
import type { KeyOp } from "@/types";
import { makeErrorText } from "@/composables/errorText";
import { listBuckets } from "@/api/buckets";
import {
  KEY_OPS,
  canonicalBuckets,
  canonicalOps,
  fieldToInstant,
  instantToField,
  isFutureField,
  keyBucketsOf,
} from "@/composables/keyForm";

import "./KeysView.scss";

/**
 * The API-key roster plus its copy-once card.
 *
 * The minted credential appears INSIDE the create window (a second step of
 * that secondary window), never as a block grown at the top of the page
 * after the dialog closed — a click may not insert page DOM. A FAILED
 * clipboard write is a TOAST: the card never grows an error paragraph
 * beside its buttons.
 *
 * A key carries three fields the user writes: its name, its PERMISSION
 * set (`read` / `upload` / `share`, at least one) and an OPTIONAL expiry.
 * Create and edit are the SAME form — the edit window reuses it with no
 * plaintext key anywhere (there is none to show: the credential is minted
 * once and stored hashed) and submits the whole record, because
 * `keys.update` is a full replacement. An emptied expiry field therefore
 * CLEARS the expiry rather than meaning "leave it alone".
 *
 * Every operation's RESULT is a toast; the roster's own load failure stays
 * on the table scaffold (hard error / stale banner). The expiry badge is
 * the SERVER's `expired` flag — the view never re-derives "is this dead"
 * from `expires_at` and its own clock.
 */

/** The view's own keyed toast slots — one per operation, so a repeat
 *  attempt replaces its predecessor's message instead of stacking two. */
const CREATE_TOAST = "keys-create";
const UPDATE_TOAST = "keys-update";
const REVOKE_TOAST = "keys-revoke";

/** Which copy button most recently succeeded (the tri-state: a target, or
 *  nothing). A FAILED copy is raised on the toast surface. */
type CopyTarget = "key" | "config" | "prompt";

/** Which window owns the shared form (each namespaces its test ids, so a
 *  test can address the create and edit fields apart). */
type FormScope = "create" | "edit";

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
    const createBusy = ref(false);
    const copied = ref<CopyTarget | null>(null);
    const pendingRevoke = ref<ApiKeyPublic | null>(null);
    const revokeBusy = ref(false);

    // ── the shared create/edit form ────────────────────────────────────
    //
    // ONE form object drives BOTH windows: the create window's first step
    // and the edit window are never open together (the create window's
    // second step hides the form), so the two dialogs cannot disagree
    // about what a valid key looks like.
    const editTarget = ref<ApiKeyPublic | null>(null);
    const updateBusy = ref(false);
    const formName = ref("");
    const formOps = ref<KeyOp[]>([]);
    /// The `datetime-local` value (`""` = no expiry) — the same field
    /// shape FilesView's share dialog uses; this wave adds no picker.
    const formExpiry = ref("");
    /// The key's bucket reference (Q7). `null` = send WITHOUT the field
    /// (the server materializes the owner's granted buckets); an array
    /// (even empty) = the explicit pick, sent as-is.
    const formBuckets = ref<string[] | null>(null);
    /// The registry's slugs the picker offers (the create/edit dialog
    /// loads them once per open; empty registry = only "no buckets").
    const knownSlugs = ref<string[]>([]);

    const nameValid = computed(() => formName.value.trim().length > 0);
    /// At least one permission — the server's rule, mirrored so an empty
    /// set is refused before a round trip (the server still answers
    /// `at least one permission is required` if one slips through).
    const opsValid = computed(() => formOps.value.length > 0);
    /// An expiry is either absent or strictly in the future.
    const expiryValid = computed(
      () => formExpiry.value.trim() === "" || isFutureField(formExpiry.value),
    );
    const canSubmitForm = computed(
      () => nameValid.value && opsValid.value && expiryValid.value,
    );

    function resetForm(): void {
      formName.value = "";
      formOps.value = [];
      formExpiry.value = "";
      formBuckets.value = null;
      knownSlugs.value = [];
    }

    /** Toggle one permission (the switches' `update:modelValue` lane). */
    function toggleOp(op: KeyOp, on: boolean): void {
      formOps.value = on
        ? canonicalOps([...formOps.value, op])
        : formOps.value.filter((value) => value !== op);
    }

    /** Toggle one slug in the picker (multi-select semantics: click to
     *  add, click again to remove). */
    function toggleBucket(slug: string): void {
      const current = formBuckets.value ?? [];
      formBuckets.value = current.includes(slug)
        ? current.filter((s) => s !== slug)
        : [...current, slug];
    }

    /** The registry's slugs for the picker, canonical (deduped, sorted).
     *  MUT:p2a-key-buckets-canonical — offering an unsorted/duplicated
     *  list reddens the picker test. */
    const bucketOptions = computed(() =>
      canonicalBuckets(knownSlugs.value).map((slug) => ({
        slug,
        picked: (formBuckets.value ?? []).includes(slug),
      })),
    );

    /** The write payload: the canonical op order, the trimmed name, the
     *  expiry as an RFC 3339 instant — OMITTED entirely when the field
     *  is empty, which is the server's "no expiry" (and the way an edit
     *  clears one) — and the bucket reference (Q7): `formBuckets === null`
     *  sends WITHOUT the field (server materializes granted_buckets);
     *  an array (even `[]` = no buckets) is the explicit pick.
     *  MUT:p2a-key-buckets-omit — sending `[]` for a K-era key's null
     *  (or dropping the field for an explicit empty) reddens this. */
    function formPayload(): KeyWrite {
      const instant = fieldToInstant(formExpiry.value);
      return {
        name: formName.value.trim(),
        ops: canonicalOps(formOps.value),
        ...(instant === undefined ? {} : { expires_at: instant }),
        ...(formBuckets.value === null ? {} : { buckets: canonicalBuckets(formBuckets.value) }),
      };
    }

    /** The roster's column set (the family table anatomy). `key` doubles as
     *  the cell-slot name (`cell-<key>`); the action column opts out of
     *  click-to-copy. */
    const columns = computed<HTableColumn[]>(() => [
      { key: "name", title: t("common.keys.nameLabel") },
      { key: "prefix", title: t("common.keys.prefix"), copyable: false },
      { key: "ops", title: t("common.keys.permissions"), copyable: false },
      { key: "buckets", title: t("common.keys.buckets"), copyable: false },
      { key: "expiry", title: t("common.keys.expiry"), copyable: false },
      { key: "state", title: t("common.keys.state") },
      { key: "actions", title: "", align: "right", width: "5.5rem", copyable: false },
    ]);

    /// The never-empty failure text (shared factory; the contract and its
    /// pins live in composables/errorText).
    const errorText = makeErrorText(t);

    /** Drop this view's stale message for one operation: the toast store
     *  is module-level, so a reopened window or a fresh row would
     *  otherwise inherit the previous attempt's message. */
    function clearToast(key: string): void {
      const slot = toast.toasts.find((item) => item.key === key);
      if (slot) toast.remove(slot.id);
    }

    async function load() {
      error.value = null;
      loading.value = true;
      try { keys.value = await api.listKeys(); } catch (e) { error.value = errorText(e); } finally { loading.value = false; }
    }
    onMounted(() => void load());
    usePageRefresh(() => load(), { loading });
    usePageCreateAction({
      label: () => t("common.keys.create"),
      onCreate: () => {
        clearToast(CREATE_TOAST);
        resetForm();
        created.value = null;
        createOpen.value = true;
        void loadBucketSlugs();
      },
    });

    // ── create ─────────────────────────────────────────────────────────

    /** Load the registry's slugs for the picker (best-effort: a failed
     *  read leaves the picker empty rather than blocking the dialog —
     *  the write path still refuses unknown slugs server-side). */
    async function loadBucketSlugs(): Promise<void> {
      try {
        knownSlugs.value = (await listBuckets()).buckets.map((b) => b.slug);
      } catch {
        knownSlugs.value = [];
      }
    }

    async function doCreate() {
      // The client-side mirror of the server's three refusals (blank name,
      // no permission, past expiry): the submit is disabled for all three
      // and this guard keeps a programmatic path from slipping past it.
      if (createBusy.value || !canSubmitForm.value) return;
      createBusy.value = true;
      clearToast(CREATE_TOAST);
      try {
        created.value = await api.createKey(formPayload());
        // The dialog STAYS OPEN and swaps to the minted-key step: the
        // copy-once card used to grow at the top of the page after the
        // dialog closed (a module inserted into the page by a click —
        // banned by the user's rule). The credential now appears where
        // the action happened, in the secondary window.
        await load();
      } catch (e) {
        // The server's answer wins over the mirror: its messages (`name
        // must not be empty`, `at least one permission is required`,
        // `expires_at must be in the future`) are shown as they arrive.
        toast.show(errorText(e), { type: "error", key: CREATE_TOAST });
      } finally { createBusy.value = false; }
    }

    /** Close the create window and drop the minted key from memory (it is
     *  shown once, by design — reopening the dialog mints a new one). */
    function finishCreated(): void {
      created.value = null;
      createOpen.value = false;
      resetForm();
    }

    const createActions = computed<ModalAction[]>(() => [
      {
        label: t("common.actions.cancel"),
        variant: "secondary",
        onClick: () => finishCreated(),
      },
      {
        label: t("common.actions.create"),
        variant: "primary",
        loading: createBusy.value,
        // Three refusals, disabled up front: a blank name, no permission,
        // an expiry that has already passed.
        disabled: !canSubmitForm.value,
        onClick: () => { void doCreate(); },
      },
    ]);

    // ── edit ───────────────────────────────────────────────────────────

    function openEdit(row: ApiKeyPublic): void {
      clearToast(UPDATE_TOAST);
      editTarget.value = row;
      formName.value = row.name;
      // The server reports ops in canonical order; the form normalizes
      // anyway so an out-of-order producer cannot smuggle a reordering
      // into the replaced payload.
      formOps.value = canonicalOps(row.ops ?? []);
      formExpiry.value = instantToField(row.expires_at);
      // Q7: a scopes WITHOUT a `buckets` array (K-era) edits as "not
      // picked yet" (the dialog loads the registry and the submit pins
      // the explicit array); an array — `[]` included — is the row's
      // explicit pick and pre-fills the picker from it.
      formBuckets.value = keyBucketsOf(row.buckets);
      void loadBucketSlugs();
    }

    function closeEdit(): void {
      editTarget.value = null;
      resetForm();
    }

    async function doUpdate() {
      const target = editTarget.value;
      if (!target || updateBusy.value || !canSubmitForm.value) return;
      updateBusy.value = true;
      clearToast(UPDATE_TOAST);
      try {
        // FULL replacement: name, ops and expiry all go, and an emptied
        // expiry field is absent from the payload — which the server
        // reads as "this key never expires" (the way to clear one).
        await api.updateKey(target.id, formPayload());
        closeEdit();
        toast.show(t("common.keys.updated"), { type: "success", key: UPDATE_TOAST });
        await load();
      } catch (e) {
        // The window stays open with the typed values so the attempt can
        // be retried; the failure is this operation's toast, never a
        // paragraph beside the form.
        toast.show(errorText(e), { type: "error", key: UPDATE_TOAST });
      } finally { updateBusy.value = false; }
    }

    const editActions = computed<ModalAction[]>(() => [
      {
        label: t("common.actions.cancel"),
        variant: "secondary",
        onClick: () => closeEdit(),
      },
      {
        label: t("common.actions.save"),
        variant: "primary",
        loading: updateBusy.value,
        disabled: !canSubmitForm.value,
        onClick: () => { void doUpdate(); },
      },
    ]);

    // ── copy-once card ─────────────────────────────────────────────────

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

    // ── revoke ─────────────────────────────────────────────────────────

    async function doRevoke() {
      if (!pendingRevoke.value || revokeBusy.value) return;
      revokeBusy.value = true;
      clearToast(REVOKE_TOAST);
      try {
        await api.revokeKey(pendingRevoke.value.id);
        pendingRevoke.value = null;
        void load();
      } catch (e) {
        toast.show(errorText(e), { type: "error", key: REVOKE_TOAST });
      } finally { revokeBusy.value = false; }
    }

    // ── cells ──────────────────────────────────────────────────────────

    /// One permission's caption — the ops column and the dialog's switch
    /// captions print the same three strings.
    function opLabel(op: KeyOp): string {
      return t(`common.keys.op_${op}`);
    }

    /// An RFC 3339 instant in the viewer's locale and zone.
    function formatExpiry(instant: string): string {
      const date = new Date(instant);
      if (Number.isNaN(date.getTime())) return instant;
      return date.toLocaleString();
    }

    /** The permissions cell: one label per op, in the canonical order the
     *  server reports them.
     *
     *  An EMPTY set is not an empty cell. Every key minted before
     *  per-key permissions were enforced was normalized fail-closed to
     *  no permission at all by the upgrade migration (its stored set had
     *  never been read by anything), so an empty set is the common state
     *  right after deploying this wave — and such a key can do NOTHING
     *  on the MCP surface. It therefore renders a loud, actionable tag
     *  ("no permissions — edit to grant"), not a blank or a muted dash:
     *  silence there reads as "no permissions configured, all fine",
     *  which is the exact opposite of what the row means. */
    function renderOps(row: ApiKeyPublic) {
      const ops = canonicalOps(row.ops ?? []);
      if (ops.length === 0) {
        return (
          <HkBadge variant="warning" size="sm" dot data-testid={`key-no-permissions-${row.id}`}>
            {t("common.keys.noPermissions")}
          </HkBadge>
        );
      }
      return (
        <span class="key-ops" data-testid={`key-ops-${row.id}`}>
          {ops.map((op) => (
            <HkBadge key={op} variant="info" size="sm">{opLabel(op)}</HkBadge>
          ))}
        </span>
      );
    }

    /**
     * The buckets cell (Q7). A scopes WITHOUT a `buckets` array — the
     * K-era shape, fail-closed to "no buckets" server-side — renders
     * the loud「无桶」marker, not a blank: silence would read as "all
     * buckets allowed", the exact opposite of what the row means.
     * MUT:p2a-key-no-buckets-marker — dropping the marker branch leaves
     * the K-era row blank and reddens the roster test. An EXPLICIT empty
     * array is a real pick and shows the plain empty state.
     */
    function renderBuckets(row: ApiKeyPublic) {
      const buckets = keyBucketsOf(row.buckets);
      if (buckets === null) {
        return (
          <HkBadge variant="warning" size="sm" dot data-testid={`key-no-buckets-${row.id}`}>
            {t("common.keys.noBuckets")}
          </HkBadge>
        );
      }
      if (buckets.length === 0) {
        return (
          <span class="key-buckets-none" data-testid={`key-empty-buckets-${row.id}`}>
            {t("common.keys.noBucketsPicked")}
          </span>
        );
      }
      return (
        <span class="key-ops" data-testid={`key-buckets-${row.id}`}>
          {buckets.map((slug) => (
            <HkBadge key={slug} variant="info" size="sm" mono>{slug}</HkBadge>
          ))}
        </span>
      );
    }

    /**
     * The expiry cell.
     *
     * "Expired" is the SERVER's verdict (`row.expired`), never a local
     * comparison of `expires_at` against the browser clock: the server
     * owns the clock and the fail-closed parse, so a skewed client can
     * neither hide a dead key nor mark a live one dead.
     */
    function renderExpiry(row: ApiKeyPublic) {
      if (!row.expires_at) {
        return (
          <span class="key-expiry never" data-testid={`key-expiry-${row.id}`}>
            {t("common.keys.neverExpires")}
          </span>
        );
      }
      return (
        <span class="key-expiry" data-testid={`key-expiry-${row.id}`}>
          <span class="key-expiry-instant">{formatExpiry(row.expires_at)}</span>
          {row.expired
            ? <HkBadge variant="warning" size="sm">{t("common.keys.expired")}</HkBadge>
            : null}
        </span>
      );
    }

    /** The row actions: edit (never on a revoked key — the server answers
     *  not-found for one) and revoke. */
    function renderRowActions(row: ApiKeyPublic) {
      if (row.revoked_at) return null;
      return (
        <span class="key-row-actions">
          <HkIconButton
            size={24}
            variant="ghost"
            aria-label={t("common.keys.edit")}
            {...{ title: t("common.keys.edit") }}
            data-testid={`edit-key-${row.id}`}
            onClick={() => openEdit(row)}
          >
            {{ icon: () => <Pencil size={14} /> }}
          </HkIconButton>
          <HkIconButton
            size={24}
            variant="ghost"
            aria-label={t("common.keys.revoke")}
            {...{ title: t("common.keys.revoke") }}
            data-testid={`revoke-key-${row.id}`}
            onClick={() => { clearToast(REVOKE_TOAST); pendingRevoke.value = row; }}
          >
            {{ icon: () => <Trash2 size={14} /> }}
          </HkIconButton>
        </span>
      );
    }

    // ── the shared form body ───────────────────────────────────────────

    /** Name / permissions / expiry — ONE renderer for both windows. */
    function renderForm(scope: FormScope) {
      const id = (field: string): string => `${scope}-${field}`;
      return (
        <>
          <HkInput
            v-model={formName.value}
            placeholder={t("common.keys.nameLabel")}
            aria-label={t("common.keys.nameLabel")}
            data-testid={scope === "create" ? "key-name" : "edit-key-name"}
          />
          <fieldset class="key-ops-field">
            <legend>{t("common.keys.permissions")}</legend>
            {/* A DIV, not a LABEL: HkSwitch already renders its own
                `<label>` wrapper, and a label inside a label makes the
                caption's click ambiguous. The switch carries the
                accessible name explicitly. */}
            {KEY_OPS.map((op) => (
              <div class="key-switch-row" key={op}>
                <span>{opLabel(op)}</span>
                <HkSwitch
                  modelValue={formOps.value.includes(op)}
                  aria-label={opLabel(op)}
                  data-testid={id(`op-${op}`)}
                  onUpdate:modelValue={(on: boolean) => toggleOp(op, on)}
                />
              </div>
            ))}
          </fieldset>
          {/* The bucket picker (Q7): one toggle row per registry slug,
              click to add / click again to remove. An EMPTY selection
              with the field PRESENT is the explicit `[]` = no buckets
              pick — the payload still carries `buckets: []`; leaving the
              whole field out (the null state) is the K-era "not picked
              yet" lane that materializes the owner's buckets. */}
          <fieldset class="key-ops-field">
            <legend>{t("common.keys.buckets")}</legend>
            {bucketOptions.value.length === 0 ? (
              <p class="key-form-hint" data-testid={id("buckets-empty")}>
                {t("common.keys.noBucketsAvailable")}
              </p>
            ) : (
              bucketOptions.value.map((option) => (
                <div class="key-switch-row" key={option.slug}>
                  <span>{option.slug}</span>
                  <HkSwitch
                    modelValue={option.picked}
                    aria-label={option.slug}
                    data-testid={id(`bucket-${option.slug}`)}
                    onUpdate:modelValue={() => toggleBucket(option.slug)}
                  />
                </div>
              ))
            )}
            <p class="key-form-hint">{t("common.keys.bucketsHint")}</p>
          </fieldset>
          <label class="key-expiry-field">
            <span>{t("common.keys.expiryOptional")}</span>
            <input
              v-model={formExpiry.value}
              type="datetime-local"
              class="key-native-input"
              data-testid={scope === "create" ? "key-expiry" : "edit-key-expiry"}
            />
          </label>
          {/* The two refusals the submit's `disabled` state implies are
              also SAID, next to the field that causes them: a dead button
              with no reason is the failure mode this avoids. They ride
              inside the window (where the action happens), not in the
              page. */}
          {opsValid.value ? null : <p class="key-form-hint" data-testid={id("ops-hint")}>{t("common.keys.opsRequired")}</p>}
          {expiryValid.value ? null : <p class="key-form-hint" data-testid={id("expiry-hint")}>{t("common.keys.expiryFuture")}</p>}
        </>
      );
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

            "cell-ops": ({ row }: { row: ApiKeyPublic }) => renderOps(row),

            "cell-buckets": ({ row }: { row: ApiKeyPublic }) => renderBuckets(row),

            "cell-expiry": ({ row }: { row: ApiKeyPublic }) => renderExpiry(row),

            "cell-state": ({ row }: { row: ApiKeyPublic }) => (
              row.revoked_at
                ? <HkBadge variant="muted" pill>{t("common.keys.revoked")}</HkBadge>
                : <HkBadge variant="success" pill>{t("common.keys.active")}</HkBadge>
            ),

            "cell-actions": ({ row }: { row: ApiKeyPublic }) => renderRowActions(row),
          }}
        </HkAdminTablePage>

        {/* The create window, two steps: the form → the minted-key card.
            The credential rides HERE, where the action happened (never a
            block grown at the top of the page after the dialog closed). */}
        <HkAdaptiveDialog
          modelValue={createOpen.value}
          title={t("common.keys.create")}
          width="26rem"
          footerActions={created.value ? undefined : createActions.value}
          onUpdate:modelValue={(v: boolean) => { if (!v) finishCreated(); }}
        >
          {created.value ? (
            <div class="copy-once" data-testid="copy-once-card">
              <p class="warning">{t("common.keys.copyOnceWarning")}</p>
              <code class="key-text" data-testid="plaintext-key">{created.value.key}</code>
              <div class="actions">
                <HkButton variant="primary" data-testid="copy-key" onClick={copyKey}>
                  {{
                    icon: () => <Copy size={14} />,
                    default: () => copied.value === "key" ? t("common.keys.copied") : t("common.actions.copyLink"),
                  }}
                </HkButton>
                <HkButton variant="ghost" data-testid="copy-config" onClick={copyConfig}>
                  {copied.value === "config" ? t("common.keys.copied") : t("common.keys.copyConfig")}
                </HkButton>
                <HkButton variant="ghost" data-testid="copy-prompt" onClick={copyPrompt}>
                  {copied.value === "prompt" ? t("common.keys.copied") : t("common.keys.copyPrompt")}
                </HkButton>
              </div>
              <div class="dialog-body">
                <HkButton variant="secondary" data-testid="copy-once-done" onClick={finishCreated}>{t("common.actions.confirm")}</HkButton>
              </div>
            </div>
          ) : (
            <div class="dialog-body">{renderForm("create")}</div>
          )}
        </HkAdaptiveDialog>

        {/* The edit window: the SAME form, no plaintext key, and a full
            replacement on submit. */}
        <HkAdaptiveDialog
          modelValue={editTarget.value !== null}
          title={t("common.keys.edit")}
          width="26rem"
          footerActions={editActions.value}
          onUpdate:modelValue={(v: boolean) => { if (!v) closeEdit(); }}
        >
          <div class="dialog-body">
            {editTarget.value ? (
              <span class="key-edit-prefix">
                <code class="key-edit-prefix-text">{editTarget.value.key_prefix}</code>
              </span>
            ) : null}
            {renderForm("edit")}
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
