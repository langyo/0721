<script setup lang="ts">
import {
  HkButton,
  HkEmptyState,
  HkInput,
  HkSectionCard,
  HkSwitch,
} from "@celestia-island/hikari";
import { Plus, RefreshCw, Trash2 } from "lucide-vue-next";
import { computed, onMounted, reactive, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as api from "@/api";
import { usePageRefresh } from "@/composables/usePageRefresh";
import type { SiteConfig, SiteConfigStrict } from "@/types";

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

const { t } = useI18n();

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
  form.defaultQuota = config.upload["default-quota-bytes"]?.toString() ?? "";
}

async function load() {
  loading.value = true;
  loadError.value = null;
  try {
    fill(await api.getSiteConfig());
  } catch (err) {
    loadError.value = err instanceof Error ? err.message : t("common.status.error");
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
    saveError.value = err instanceof Error ? err.message : t("common.settings.saveFailed");
  } finally {
    passwordBusy.value = false;
  }
}

usePageRefresh(() => load(), { loading });

// ── validation (mirrors SiteConfig::validate on the server) ───────────

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

const valid = computed(
  () => mediaPathError.value === null && sizeLimitError.value === null && timezoneError.value === null,
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
          ? BigInt(form.defaultQuota.trim())
          : null,
      },
    };
    await api.putSiteConfig(strict);
    savedFlash.value = true;
    window.setTimeout(() => {
      savedFlash.value = false;
    }, 2400);
  } catch (err) {
    saveError.value = err instanceof Error ? err.message : t("common.settings.saveFailed");
  } finally {
    saveBusy.value = false;
  }
}
</script>

<template>
  <section class="site-settings">
    <HkEmptyState v-if="loading" loading boxed :title="t('common.status.loading')" />
    <HkEmptyState v-else-if="loadError" boxed :title="t('common.status.error')" :description="loadError" />

    <template v-else>
      <p class="strict-hint">{{ t("common.settings.strictHint") }}</p>

      <HkSectionCard :title="t('common.settings.sectionPortal')">
        <div class="grid">
          <label class="field">
            <span>{{ t("common.settings.titleSuffix") }}</span>
            <HkInput v-model="form.titleSuffix" name="title-suffix" />
          </label>
          <label class="field">
            <span>{{ t("common.settings.language") }}</span>
            <select v-model="form.language" class="native-select" data-testid="settings-language">
              <option v-for="lang in LANGUAGES" :key="lang" :value="lang">{{ lang }}</option>
            </select>
          </label>
          <label class="field narrow">
            <span>{{ t("common.settings.timezone") }}</span>
            <HkInput
              :model-value="String(form.timezone)"
              name="timezone"
              type="number"
              :error="timezoneError ?? undefined"
              @update:model-value="(v: string) => (form.timezone = Number.parseInt(v, 10) || 0)"
            />
          </label>
        </div>
        <div class="field">
          <span>{{ t("common.settings.footerBanner") }}</span>
          <ul class="banner-list">
            <li v-for="(banner, index) in form.footerBanner" :key="index" class="banner-row">
              <HkInput
                v-model="banner.text"
                :name="`banner-text-${index}`"
                :placeholder="t('common.settings.bannerText')"
              />
              <HkInput
                v-model="banner.url"
                :name="`banner-url-${index}`"
                :placeholder="t('common.settings.bannerUrl')"
              />
              <button
                type="button"
                class="row-remove"
                :aria-label="t('common.actions.remove')"
                @click="form.footerBanner.splice(index, 1)"
              >
                <Trash2 :size="14" />
              </button>
            </li>
          </ul>
          <HkButton variant="ghost" @click="form.footerBanner.push({ text: '', url: '' })">
            <template #icon><Plus :size="14" /></template>
            {{ t("common.settings.addBanner") }}
          </HkButton>
        </div>
      </HkSectionCard>

      <HkSectionCard :title="t('common.settings.sectionRouter')">
        <div class="grid">
          <label class="field">
            <span>{{ t("common.settings.mediaEntryPath") }}</span>
            <HkInput
              v-model="form.mediaEntryPath"
              name="media-entry-path"
              :error="mediaPathError ?? undefined"
            />
          </label>
        </div>
        <div class="field">
          <span>{{ t("common.settings.limitReferrerHost") }}</span>
          <ul class="banner-list">
            <li v-for="(host, index) in form.limitReferrerHost" :key="index" class="banner-row">
              <HkInput v-model="form.limitReferrerHost[index]!" :name="`referrer-host-${index}`" />
              <button
                type="button"
                class="row-remove"
                :aria-label="t('common.actions.remove')"
                @click="form.limitReferrerHost.splice(index, 1)"
              >
                <Trash2 :size="14" />
              </button>
            </li>
          </ul>
          <HkButton variant="ghost" @click="form.limitReferrerHost.push('')">
            <template #icon><Plus :size="14" /></template>
            {{ t("common.actions.add") }}
          </HkButton>
        </div>
      </HkSectionCard>

      <HkSectionCard :title="t('common.settings.sectionUpload')">
        <div class="grid">
          <label class="field">
            <span>{{ t("common.settings.imageSizeLimit") }}</span>
            <HkInput
              v-model="form.imageSizeLimit"
              name="image-size-limit"
              :error="sizeLimitError ?? undefined"
            />
          </label>
          <div class="field switch-field">
            <span>{{ t("common.settings.webpAutoConvert") }}</span>
            <HkSwitch v-model="form.webpAutoConvert" :aria-label="t('common.settings.webpAutoConvert')" />
          </div>
          <div class="field switch-field">
            <span>{{ t("common.settings.useSourceFileName") }}</span>
            <HkSwitch v-model="form.useSourceFileName" :aria-label="t('common.settings.useSourceFileName')" />
          </div>
        </div>
      </HkSectionCard>

      <HkSectionCard :title="t('common.settings.passwordLogin')">
        <div class="field switch-field password-switch">
          <div>
            <span>{{ t("common.settings.passwordLogin") }}</span>
            <small class="password-hint">{{ t("common.settings.passwordLoginHint") }}</small>
          </div>
          <HkSwitch
            :model-value="passwordLogin"
            :disabled="passwordBusy"
            :aria-label="t('common.settings.passwordLogin')"
            data-testid="password-login-switch"
            @update:model-value="(v: boolean) => savePasswordLogin(v)"
          />
        </div>
      </HkSectionCard>

      <footer class="save-row">
        <HkButton variant="primary" :disabled="!valid" :aria-label="t('common.actions.save')" @click="save">
          <template #icon><RefreshCw v-if="saveBusy" :size="14" /><Plus v-else :size="14" /></template>
          {{ saveBusy ? t("common.status.loading") : t("common.actions.save") }}
        </HkButton>
        <span v-if="savedFlash" class="save-flash" role="status">{{ t("common.settings.saved") }}</span>
        <span v-else-if="saveError" class="save-error" role="alert">{{ saveError }}</span>
      </footer>
    </template>
  </section>
</template>

<style scoped lang="scss">
.site-settings {
  display: flex;
  flex-direction: column;
  gap: 1rem;
}

.strict-hint {
  margin: 0;
  opacity: 0.7;
  font-size: 0.85rem;
}

.grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(14rem, 1fr));
  gap: 0.9rem;
  margin-bottom: 0.9rem;
}

.field {
  display: flex;
  flex-direction: column;
  gap: 0.35rem;
  font-size: 0.85rem;
}

.field.narrow {
  max-width: 10rem;
}

.switch-field {
  justify-content: flex-end;
  gap: 0.6rem;
}

.password-switch {
  flex-direction: row;
  align-items: center;
  justify-content: space-between;
}

.password-hint {
  display: block;
  opacity: 0.65;
  font-size: 0.72rem;
  margin-top: 0.15rem;
}

.banner-list {
  list-style: none;
  margin: 0 0 0.5rem;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.4rem;
}

.banner-row {
  display: flex;
  gap: 0.5rem;
  align-items: center;
}

.row-remove {
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

.native-select {
  padding: 0.45rem 0.6rem;
  border-radius: 0.5rem;
  border: 1px solid color-mix(in srgb, currentColor 22%, transparent);
  background: transparent;
  color: inherit;
}

.save-row {
  display: flex;
  align-items: center;
  gap: 0.9rem;
}

.save-flash {
  color: rgb(22 128 61);
  font-size: 0.85rem;
}

.save-error {
  color: rgb(220 38 38);
  font-size: 0.85rem;
}
</style>
