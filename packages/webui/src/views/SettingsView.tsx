import {
  HkAdaptiveDialog,
  HkButton,
  HkEmptyState,
  HkIconButton,
  HkInput,
  HkSectionCard,
  HkSelect,
  HkSwitch,
  useToast,
} from "@celestia-island/hikari";
import { Plus, RefreshCw, Trash2 } from "lucide-vue-next";
import { computed, defineComponent, onMounted, reactive, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as api from "@/api";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { SiteConfig, SiteConfigStrict } from "@/types";
import { makeErrorText } from "@/composables/errorText";
import { nativeTitle } from "@/utils/nativeTitle";

import "./SettingsView.css";

/**
 * The site configuration editor — a STRICT full-document form.
 *
 * The `settings.put` RPC replaces the whole configuration: the wire form
 * (`SiteConfigStrict`) requires every section and every leaf and rejects
 * unknown fields, so a partial submit would fail with 400 instead of
 * silently resetting the omitted sections. The form therefore renders
 * the complete document and mirrors the same validation client-side
 * (pinned media path, parseable size limit) before saving.
 *
 * Two families of list live in this document, and BOTH obey one rule: a
 * list grows only through its own second-level dialog, never by inserting
 * an input row beside the button. "Add banner entry" and "Add" open a
 * dialog that collects ONE entry (text + link / one host), and only the
 * dialog's confirm appends it to the form; the list itself is read-only
 * display with a per-row delete. The value reaches the wire document the
 * same way it always did — through `save()`.
 *
 * Saving feedback is a TOAST on both lanes (the document save and the
 * password kill-switch): the inline `save-error` span is gone, so a failed
 * save never grows a red paragraph under the button.
 */
export default defineComponent({
  name: "SettingsView",
  setup() {
    const { t } = useI18n();
    const toast = useToast();

    const errorText = makeErrorText(t);
    const saveErrorText = makeErrorText(t, "common.settings.saveFailed");

    const loading = ref(false);
    const loadError = ref<string | null>(null);
    const passwordLogin = ref(true);
    const passwordBusy = ref(false);
    const saveBusy = ref(false);

    /** The legacy language tags the portal config accepts (underscore form). */
    const LANGUAGES = [
      "zh_hans",
      "zh_hant",
      "en_us",
      "ja",
      "ko",
      "de",
      "fr",
      "es",
      "pt",
      "ru",
      "it",
    ];

    /** The family select takes {value,label}; the language roster is plain
     * codes today, so the label is the code. */
    const languageOptions = computed(() => LANGUAGES.map((code) => ({ value: code, label: code })));

    const form = reactive({
      titleSuffix: "",
      footerBanner: [] as { text: string; url: string }[],
      language: "zh_hans",
      timezone: 8,
      mediaEntryPath: "/media",
      limitReferrerHost: [] as string[],
      imageSizeLimit: "8MiB",
      webpAutoConvert: true,
      useSourceFileName: false,
      defaultQuota: "",
    });

    function fill(config: SiteConfig): void {
      form.titleSuffix = config.portal["title-suffix"];
      form.footerBanner = config.portal["footer-banner"].map((b) => ({
        text: b.text,
        url: b.url ?? "",
      }));
      form.language = config.portal.language;
      form.timezone = config.portal.timezone;
      form.mediaEntryPath = config.router["media-entry-path"];
      form.limitReferrerHost = [...config.router["limit-referrer-host"]];
      form.imageSizeLimit = config.upload["image-size-limit"];
      form.webpAutoConvert = config.upload["webp-auto-convert"];
      form.useSourceFileName = config.upload["use-source-file-name"];
      form.defaultQuota = config.upload["default-quota-bytes"] != null
        ? config.upload["default-quota-bytes"]!.toString()
        : "";
    }

    async function load(): Promise<void> {
      loading.value = true;
      loadError.value = null;
      try {
        fill(await api.getSiteConfig());
      } catch (err) {
        loadError.value = errorText(err);
      } finally {
        loading.value = false;
      }
    }

    onMounted(() => {
      void load();
      // The password kill-switch (admin): rides its own lane so a broken
      // settings document cannot take the toggle down with it.
      void api.getPasswordLogin().then((enabled) => {
        passwordLogin.value = enabled;
      }).catch(() => undefined);
    });

    async function savePasswordLogin(enabled: boolean): Promise<void> {
      passwordBusy.value = true;
      const previous = passwordLogin.value;
      passwordLogin.value = enabled;
      try {
        await api.putPasswordLogin(enabled);
        toast.success(t("common.settings.saved"));
      } catch (err) {
        passwordLogin.value = previous;
        toast.error(saveErrorText(err));
      } finally {
        passwordBusy.value = false;
      }
    }

    usePageRefresh(() => load(), { loading });

    // ── validation (mirrors SiteConfig::validate on the server) ─────────

    const SIZE_LIMIT_RE = /^\s*\d+(\.\d+)?\s*(b|k|kb|kib|m|mb|mib|g|gb|gib|t|tb|tib)?\s*$/i;

    const mediaPathError = computed(() =>
      form.mediaEntryPath === "/media" ? null : "must stay \"/media\"",
    );
    const sizeLimitError = computed(() =>
      SIZE_LIMIT_RE.test(form.imageSizeLimit) ? null : "not a byte size (\"8MiB\" style)",
    );
    const timezoneError = computed(() =>
      Number.isInteger(form.timezone) && Math.abs(form.timezone) <= 14 ? null : "-14..=14",
    );

    const quotaError = computed(() => {
      const raw = form.defaultQuota.trim();
      if (!raw) return null; // empty = unlimited (explicit semantics)
      const n = Number(raw);
      if (!Number.isFinite(n) || !Number.isInteger(n) || n < 0) {
        return "must be a non-negative integer (or empty for unlimited)";
      }
      return null;
    });

    const valid = computed(
      () =>
        mediaPathError.value === null &&
        sizeLimitError.value === null &&
        timezoneError.value === null &&
        quotaError.value === null,
    );

    async function save(): Promise<void> {
      if (!valid.value || saveBusy.value) return;
      saveBusy.value = true;
      try {
        const strict: SiteConfigStrict = {
          portal: {
            "title-suffix": form.titleSuffix,
            "footer-banner": form.footerBanner
              .filter((b) => b.text.trim() !== "")
              .map((b) => ({
                text: b.text,
                ...(b.url.trim() ? { url: b.url.trim() } : {}),
              })),
            language: form.language,
            timezone: form.timezone,
          },
          router: {
            "media-entry-path": form.mediaEntryPath,
            "limit-referrer-host": form.limitReferrerHost.filter((h) => h.trim() !== ""),
          },
          upload: {
            "image-size-limit": form.imageSizeLimit,
            "webp-auto-convert": form.webpAutoConvert,
            "use-source-file-name": form.useSourceFileName,
            "default-quota-bytes": form.defaultQuota.trim()
              ? BigInt(Math.trunc(Number(form.defaultQuota)))
              : null,
          },
        };
        await api.putSiteConfig(strict);
        toast.success(t("common.settings.saved"));
      } catch (err) {
        toast.error(saveErrorText(err));
      } finally {
        saveBusy.value = false;
      }
    }

    // ── footer banner: one-entry dialog (no inline row growth) ──────────

    const bannerOpen = ref(false);
    const bannerDraft = ref({ text: "", url: "" });

    function openBannerDialog(): void {
      bannerDraft.value = { text: "", url: "" };
      bannerOpen.value = true;
    }

    /** Confirm appends the collected entry to the FORM document; nothing is
     *  persisted until the page's own save runs (the strict whole-document
     *  submit is unchanged). */
    function confirmBanner(): void {
      const text = bannerDraft.value.text;
      const url = bannerDraft.value.url;
      if (text.trim() === "") return;
      form.footerBanner.push({ text, url });
      bannerOpen.value = false;
      bannerDraft.value = { text: "", url: "" };
    }

    /** A function so the array is rebuilt at every render, where the
     *  dialog's `draft` is read afresh — the confirm button's `disabled`
     *  must follow the draft keystroke by keystroke. (A `computed` would
     *  work too: Vue invalidates it on its reactive dependency. The
     *  function is simply the smaller thing to read here.) */
    const bannerActions = () => [
      {
        label: t("common.actions.cancel"),
        variant: "secondary" as const,
        onClick: () => {
          bannerOpen.value = false;
        },
      },
      {
        label: t("common.actions.add"),
        variant: "primary" as const,
        // An empty banner entry is dropped by save() anyway; collecting one
        // here means the dialog refuses to hand back nothing.
        disabled: bannerDraft.value.text.trim() === "",
        onClick: () => {
          confirmBanner();
        },
      },
    ];

    // ── referrer-host allow-list: one-entry dialog ─────────────────────

    const hostOpen = ref(false);
    const hostDraft = ref("");

    function openHostDialog(): void {
      hostDraft.value = "";
      hostOpen.value = true;
    }

    /** Confirm appends the collected host to the FORM document (an all-blank
     *  host is not a host — save() filters blanks the same way). */
    function confirmHost(): void {
      const host = hostDraft.value;
      if (host.trim() === "") return;
      form.limitReferrerHost.push(host);
      hostOpen.value = false;
      hostDraft.value = "";
    }

    const hostActions = () => [
      {
        label: t("common.actions.cancel"),
        variant: "secondary" as const,
        onClick: () => {
          hostOpen.value = false;
        },
      },
      {
        label: t("common.actions.add"),
        variant: "primary" as const,
        disabled: hostDraft.value.trim() === "",
        onClick: () => {
          confirmHost();
        },
      },
    ];

    return () => (
      <section class="site-settings">
        {loading.value && <HkEmptyState loading boxed title={t("common.status.loading")} />}
        {!loading.value && loadError.value && (
          <HkEmptyState boxed title={t("common.status.error")} description={loadError.value} />
        )}

        {!loading.value && !loadError.value && (
          <>
            <p class="strict-hint">{t("common.settings.strictHint")}</p>

            <HkSectionCard title={t("common.settings.sectionPortal")}>
              <div class="grid">
                <label class="field">
                  <span>{t("common.settings.titleSuffix")}</span>
                  <HkInput
                    modelValue={form.titleSuffix}
                    name="title-suffix"
                    onUpdate:modelValue={(v: string) => { form.titleSuffix = v; }}
                  />
                </label>
                <label class="field">
                  <span>{t("common.settings.language")}</span>
                  <HkSelect
                    modelValue={form.language}
                    options={languageOptions.value}
                    panelTitle={t("common.settings.language")}
                    data-testid="settings-language"
                    onUpdate:modelValue={(v: string) => { form.language = v; }}
                  />
                </label>
                <label class="field narrow">
                  <span>{t("common.settings.timezone")}</span>
                  <HkInput
                    modelValue={String(form.timezone)}
                    name="timezone"
                    type="number"
                    error={timezoneError.value ?? undefined}
                    onUpdate:modelValue={(v: string) => { form.timezone = Number.parseInt(v, 10) || 0; }}
                  />
                </label>
              </div>
              <div class="field">
                <span>{t("common.settings.footerBanner")}</span>
                {/* Read-only display: an entry is collected in the dialog
                    below and only READ here (plus its delete). */}
                <ul class="banner-list">
                  {form.footerBanner.map((banner, index) => (
                    <li key={index} class="banner-row">
                      <span class="banner-text" data-testid={`banner-text-${index}`}>
                        {banner.text}
                      </span>
                      {banner.url.trim() !== "" && (
                        <span class="banner-url" data-testid={`banner-url-${index}`}>
                          {banner.url}
                        </span>
                      )}
                      <HkIconButton
                        size={24}
                        variant="ghost"
                        aria-label={t("common.actions.remove")}
                        {...nativeTitle(t("common.actions.remove"))}
                        onClick={() => { form.footerBanner.splice(index, 1); }}
                      >
                        {{ icon: () => <Trash2 size={14} /> }}
                      </HkIconButton>
                    </li>
                  ))}
                </ul>
                <HkButton variant="ghost" onClick={openBannerDialog}>
                  {[
                    <Plus key="icon" size={14} />,
                    t("common.settings.addBanner"),
                  ]}
                </HkButton>
              </div>
            </HkSectionCard>

            <HkSectionCard title={t("common.settings.sectionRouter")}>
              <div class="grid">
                <label class="field">
                  <span>{t("common.settings.mediaEntryPath")}</span>
                  <HkInput
                    modelValue={form.mediaEntryPath}
                    name="media-entry-path"
                    error={mediaPathError.value ?? undefined}
                    onUpdate:modelValue={(v: string) => { form.mediaEntryPath = v; }}
                  />
                </label>
              </div>
              <div class="field">
                <span>{t("common.settings.limitReferrerHost")}</span>
                <ul class="banner-list">
                  {form.limitReferrerHost.map((host, index) => (
                    <li key={index} class="banner-row">
                      <span class="banner-text mono" data-testid={`referrer-host-${index}`}>
                        {host}
                      </span>
                      <HkIconButton
                        size={24}
                        variant="ghost"
                        aria-label={t("common.actions.remove")}
                        {...nativeTitle(t("common.actions.remove"))}
                        onClick={() => { form.limitReferrerHost.splice(index, 1); }}
                      >
                        {{ icon: () => <Trash2 size={14} /> }}
                      </HkIconButton>
                    </li>
                  ))}
                </ul>
                <HkButton variant="ghost" onClick={openHostDialog}>
                  {[
                    <Plus key="icon" size={14} />,
                    t("common.actions.add"),
                  ]}
                </HkButton>
              </div>
            </HkSectionCard>

            <HkSectionCard title={t("common.settings.sectionUpload")}>
              <div class="grid">
                <label class="field">
                  <span>{t("common.settings.defaultQuota")}</span>
                  <HkInput
                    modelValue={form.defaultQuota}
                    name="default-quota"
                    type="number"
                    placeholder="unlimited"
                    error={quotaError.value ?? undefined}
                    data-testid="default-quota-input"
                    onUpdate:modelValue={(v: string) => { form.defaultQuota = v; }}
                  />
                  <small class="hint">{t("common.settings.defaultQuotaHint")}</small>
                </label>
                <label class="field">
                  <span>{t("common.settings.imageSizeLimit")}</span>
                  <HkInput
                    modelValue={form.imageSizeLimit}
                    name="image-size-limit"
                    error={sizeLimitError.value ?? undefined}
                    onUpdate:modelValue={(v: string) => { form.imageSizeLimit = v; }}
                  />
                </label>
                <div class="field switch-field">
                  <span>{t("common.settings.webpAutoConvert")}</span>
                  <HkSwitch
                    modelValue={form.webpAutoConvert}
                    aria-label={t("common.settings.webpAutoConvert")}
                    onUpdate:modelValue={(v: boolean) => { form.webpAutoConvert = v; }}
                  />
                </div>
                <div class="field switch-field">
                  <span>{t("common.settings.useSourceFileName")}</span>
                  <HkSwitch
                    modelValue={form.useSourceFileName}
                    aria-label={t("common.settings.useSourceFileName")}
                    onUpdate:modelValue={(v: boolean) => { form.useSourceFileName = v; }}
                  />
                </div>
              </div>
            </HkSectionCard>

            <HkSectionCard title={t("common.settings.passwordLogin")}>
              <div class="field switch-field password-switch">
                <div>
                  <span>{t("common.settings.passwordLogin")}</span>
                  <small class="password-hint">{t("common.settings.passwordLoginHint")}</small>
                </div>
                <HkSwitch
                  modelValue={passwordLogin.value}
                  disabled={passwordBusy.value}
                  aria-label={t("common.settings.passwordLogin")}
                  data-testid="password-login-switch"
                  onUpdate:modelValue={(v: boolean) => { void savePasswordLogin(v); }}
                />
              </div>
            </HkSectionCard>

            <footer class="save-row">
              <HkButton
                variant="primary"
                disabled={!valid.value}
                aria-label={t("common.actions.save")}
                onClick={() => { void save(); }}
              >
                {[
                  saveBusy.value
                    ? <RefreshCw key="icon" size={14} />
                    : <Plus key="icon" size={14} />,
                  saveBusy.value ? t("common.status.loading") : t("common.actions.save"),
                ]}
              </HkButton>

            </footer>
          </>
        )}

        {/* One-entry collector for the portal footer banner. The value lands
            in form.footerBanner on confirm — never beside the button. */}
        <HkAdaptiveDialog
          modelValue={bannerOpen.value}
          title={t("common.settings.addBanner")}
          footerActions={bannerActions()}
          width="26rem"
          onUpdate:modelValue={(v: boolean) => { bannerOpen.value = v; }}
        >
          <div class="entry-form">
            <label class="field">
              <span>{t("common.settings.bannerText")}</span>
              <HkInput
                modelValue={bannerDraft.value.text}
                name="banner-draft-text"
                autocomplete="off"
                placeholder={t("common.settings.bannerText")}
                data-testid="banner-dialog-text"
                onUpdate:modelValue={(v: string) => { bannerDraft.value.text = v; }}
              />
            </label>
            <label class="field">
              <span>{t("common.settings.bannerUrl")}</span>
              <HkInput
                modelValue={bannerDraft.value.url}
                name="banner-draft-url"
                autocomplete="off"
                placeholder={t("common.settings.bannerUrl")}
                data-testid="banner-dialog-url"
                onUpdate:modelValue={(v: string) => { bannerDraft.value.url = v; }}
              />
            </label>
          </div>
        </HkAdaptiveDialog>

        {/* One-entry collector for the referrer-host allow-list. */}
        <HkAdaptiveDialog
          modelValue={hostOpen.value}
          title={t("common.settings.limitReferrerHost")}
          footerActions={hostActions()}
          width="26rem"
          onUpdate:modelValue={(v: boolean) => { hostOpen.value = v; }}
        >
          <div class="entry-form">
            <label class="field">
              <span>{t("common.settings.limitReferrerHost")}</span>
              <HkInput
                modelValue={hostDraft.value}
                name="referrer-host-draft"
                autocomplete="off"
                placeholder={t("common.settings.limitReferrerHost")}
                data-testid="host-dialog-input"
                onUpdate:modelValue={(v: string) => { hostDraft.value = v; }}
              />
            </label>
          </div>
        </HkAdaptiveDialog>
      </section>
    );
  },
});