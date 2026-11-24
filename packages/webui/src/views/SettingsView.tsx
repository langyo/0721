import { HkButton, HkEmptyState, HkIconButton, HkInput, HkSectionCard, HkSelect, HkSwitch } from "@celestia-island/hikari";
import { Plus, RefreshCw, Trash2 } from "lucide-vue-next";
import { computed, defineComponent, onMounted, reactive, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as api from "@/api";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { SiteConfig, SiteConfigStrict } from "@/types";
import { makeErrorText } from "@/composables/errorText";

import "./SettingsView.scss";

/**
 * The site configuration editor — a STRICT full-document form.
 *
 * The `settings.put` RPC replaces the whole configuration: the wire form
 * (`SiteConfigStrict`) requires every section and every leaf and rejects
 * unknown fields, so a partial submit would fail with 400 instead of
 * silently resetting the omitted sections. The form therefore renders
 * the complete document and mirrors the same validation client-side
 * (pinned media path, parseable size limit) before saving.
 */
export default defineComponent({
  name: "SettingsView",
  setup() {
    const { t } = useI18n();

    /** The family select takes {value,label}; the language roster is plain
     * codes today, so the label is the code. */
    const languageOptions = computed(() => LANGUAGES.map((code) => ({ value: code, label: code })));

    const errorText = makeErrorText(t);
    const saveErrorText = makeErrorText(t, "common.settings.saveFailed");

    const loading = ref(false);
    const loadError = ref<string | null>(null);
    const passwordLogin = ref(true);
    const passwordBusy = ref(false);
    const savedFlash = ref(false);
    const saveBusy = ref(false);
    const saveError = ref<string | null>(null);

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

    function fill(config: SiteConfig) {
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

    async function load() {
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

    async function savePasswordLogin(enabled: boolean) {
      passwordBusy.value = true;
      const previous = passwordLogin.value;
      passwordLogin.value = enabled;
      try {
        await api.putPasswordLogin(enabled);
        savedFlash.value = true;
        window.setTimeout(() => {
          savedFlash.value = false;
        }, 2400);
      } catch (err) {
        passwordLogin.value = previous;
        saveError.value = saveErrorText(err);
      } finally {
        passwordBusy.value = false;
      }
    }

    usePageRefresh(() => load(), { loading });

    // ── validation (mirrors SiteConfig::validate on the server) ────────

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

    async function save() {
      if (!valid.value || saveBusy.value) return;
      saveBusy.value = true;
      saveError.value = null;
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
        savedFlash.value = true;
        window.setTimeout(() => {
          savedFlash.value = false;
        }, 2400);
      } catch (err) {
        saveError.value = saveErrorText(err);
      } finally {
        saveBusy.value = false;
      }
    }

    return () => (
      <section class="site-settings">
        {loading.value ? <HkEmptyState loading boxed title={t("common.status.loading")} /> : loadError.value ? (
          <HkEmptyState boxed title={t("common.status.error")} description={loadError.value} />
        ) : (
          <>
            <p class="strict-hint">{t("common.settings.strictHint")}</p>

            <HkSectionCard title={t("common.settings.sectionPortal")}>
              <div class="grid">
                <label class="field">
                  <span>{t("common.settings.titleSuffix")}</span>
                  <HkInput v-model={form.titleSuffix} name="title-suffix" />
                </label>
                <label class="field">
                  <span>{t("common.settings.language")}</span>
                  <HkSelect
                    v-model={form.language}
                    options={languageOptions.value}
                    panelTitle={t("common.settings.language")}
                    data-testid="settings-language"
                  />
                </label>
                <label class="field narrow">
                  <span>{t("common.settings.timezone")}</span>
                  <HkInput
                    modelValue={String(form.timezone)}
                    name="timezone"
                    type="number"
                    error={timezoneError.value ?? undefined}
                    onUpdate:modelValue={(v: string) => (form.timezone = Number.parseInt(v, 10) || 0)}
                  />
                </label>
              </div>
              <div class="field">
                <span>{t("common.settings.footerBanner")}</span>
                <ul class="banner-list">
                  {form.footerBanner.map((banner, index) => (
                    <li key={index} class="banner-row">
                      <HkInput
                        v-model={banner.text}
                        name={`banner-text-${index}`}
                        placeholder={t("common.settings.bannerText")}
                      />
                      <HkInput
                        v-model={banner.url}
                        name={`banner-url-${index}`}
                        placeholder={t("common.settings.bannerUrl")}
                      />
                      <HkIconButton
                        size={24}
                        variant="ghost"
                        aria-label={t("common.actions.remove")}
                        {...{ title: t("common.actions.remove") }}
                        onClick={() => form.footerBanner.splice(index, 1)}
                      >
                        {{ icon: () => <Trash2 size={14} /> }}
                      </HkIconButton>
                    </li>
                  ))}
                </ul>
                <HkButton variant="ghost" onClick={() => form.footerBanner.push({ text: "", url: "" })}>
                  {{
                    icon: () => <Plus size={14} />,
                    default: () =>                         t("common.settings.addBanner"),
                  }}
                </HkButton>
              </div>
            </HkSectionCard>

            <HkSectionCard title={t("common.settings.sectionRouter")}>
              <div class="grid">
                <label class="field">
                  <span>{t("common.settings.mediaEntryPath")}</span>
                  <HkInput
                    v-model={form.mediaEntryPath}
                    name="media-entry-path"
                    error={mediaPathError.value ?? undefined}
                  />
                </label>
              </div>
              <div class="field">
                <span>{t("common.settings.limitReferrerHost")}</span>
                <ul class="banner-list">
                  {form.limitReferrerHost.map((_host, index) => (
                    <li key={index} class="banner-row">
                      <HkInput v-model={form.limitReferrerHost[index]!} name={`referrer-host-${index}`} />
                      <HkIconButton
                        size={24}
                        variant="ghost"
                        aria-label={t("common.actions.remove")}
                        {...{ title: t("common.actions.remove") }}
                        onClick={() => form.limitReferrerHost.splice(index, 1)}
                      >
                        {{ icon: () => <Trash2 size={14} /> }}
                      </HkIconButton>
                    </li>
                  ))}
                </ul>
                <HkButton variant="ghost" onClick={() => form.limitReferrerHost.push("")}>
                  {{
                    icon: () => <Plus size={14} />,
                    default: () =>                         t("common.actions.add"),
                  }}
                </HkButton>
              </div>
            </HkSectionCard>

            <HkSectionCard title={t("common.settings.sectionUpload")}>
              <div class="grid">
                <label class="field">
                  <span>{t("common.settings.defaultQuota")}</span>
                  <HkInput
                    v-model={form.defaultQuota}
                    name="default-quota"
                    type="number"
                    placeholder="unlimited"
                    error={quotaError.value ?? undefined}
                    data-testid="default-quota-input"
                  />
                  <small class="hint">{t("common.settings.defaultQuotaHint")}</small>
                </label>
                <label class="field">
                  <span>{t("common.settings.imageSizeLimit")}</span>
                  <HkInput
                    v-model={form.imageSizeLimit}
                    name="image-size-limit"
                    error={sizeLimitError.value ?? undefined}
                  />
                </label>
                <div class="field switch-field">
                  <span>{t("common.settings.webpAutoConvert")}</span>
                  <HkSwitch v-model={form.webpAutoConvert} aria-label={t("common.settings.webpAutoConvert")} />
                </div>
                <div class="field switch-field">
                  <span>{t("common.settings.useSourceFileName")}</span>
                  <HkSwitch v-model={form.useSourceFileName} aria-label={t("common.settings.useSourceFileName")} />
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
                  onUpdate:modelValue={(v: boolean) => savePasswordLogin(v)}
                />
              </div>
            </HkSectionCard>

            <footer class="save-row">
              <HkButton variant="primary" disabled={!valid.value} aria-label={t("common.actions.save")} onClick={save}>
                {{
                  icon: () => (saveBusy.value ? <RefreshCw size={14} /> : <Plus size={14} />),
                  default: () =>                       saveBusy.value ? t("common.status.loading") : t("common.actions.save"),
                }}
              </HkButton>
              {savedFlash.value ? <span class="save-flash" role="status">{t("common.settings.saved")}</span> : saveError.value ? (
                <span class="save-error" role="alert">{saveError.value}</span>
              ) : null}
            </footer>
          </>
        )}
      </section>
    );
  },
});
