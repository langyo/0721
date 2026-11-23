<script setup lang="ts">
import { HkAboutBody, HkLocalePicker, HkSettingsDialog, HkSettingsGroup, HkThemePanelContent, type HAboutComponentVersion, type HAboutCredit, type HAboutLink } from "@celestia-island/hikari";
import { Check, Info, Languages, Palette } from "lucide-vue-next";
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import { health } from "@/api";
import { LOCALE_OPTIONS, setLocale } from "@/i18n";
import type { HealthReport } from "@/types";

/**
 * The chrome-level settings panel (every signed-in user): theme mode +
 * palette (hikari's own theme panel — light/dark/auto and the preset
 * rows), language, and the family about body (HkAboutBody — the same
 * identity hero / credits sentence / license rows every Celestia front
 * end renders). Sign-out lives in the header's account window, not
 * here. Site administration stays on its own /settings route,
 * admin-gated.
 */

defineProps<{ modelValue: boolean }>();
const emit = defineEmits<{ "update:modelValue": [value: boolean] }>();

const { t, locale } = useI18n();
const report = ref<HealthReport | null>(null);
const section = ref("theme");

onMounted(() => {
  // Fire-and-forget: the about pane degrades to the panel version
  // without it.
  void health().then((r) => {
    report.value = r;
  }).catch(() => undefined);
});

/** The family locale picker takes {code,label} rows. */
const localeCodes = computed(() =>
  LOCALE_OPTIONS.value.map((o) => ({ code: o.code, label: o.label })),
);

const sections = [
  { key: "theme", label: t("common.chrome.theme"), icon: Palette },
  { key: "language", label: t("common.chrome.language"), icon: Languages },
  { key: "about", label: t("common.chrome.about"), icon: Info },
];

function chooseLocale(code: string) {
  setLocale(code);
}

/** The credits sentence: organization first, author second — the
 * family anatomy (accent names, localized runs). */
const credits = computed<HAboutCredit[]>(() => [
  { text: t("common.about.creditsFrom") },
  { name: "Celestia Island", href: "https://celestia.world", face: "accent" },
  { text: t("common.about.creditsJoiner") },
  { name: t("common.about.author"), href: "https://github.com/langyo", face: "accent" },
  { text: t("common.about.creditsRole") },
]);

const licenses: HAboutLink[] = [{ label: "Apache-2.0", href: "https://www.apache.org/licenses/LICENSE-2.0" }];

/** The About body's `version` slot means the APPLICATION version, so it
 * is the panel's own build identity; the server/engine identity is a
 * separate fact and rides a component row (HkAboutBody's anatomy for
 * "engines"), never the app slot. */
const appVersion: string = __APP_VERSION__;

const componentRows = computed<HAboutComponentVersion[]>(() => {
  const rows: HAboutComponentVersion[] = [];
  if (report.value?.version) {
    rows.push({ label: t("common.about.engineVersion"), value: report.value.version });
  }
  return rows;
});
</script>

<template>
  <HkSettingsDialog
    :model-value="modelValue"
    :title="t('common.chrome.settings')"
    :sections="sections"
    :section="section"
    :nav-label="t('common.chrome.settings')"
    width="46rem"
    @update:model-value="(v: boolean) => emit('update:modelValue', v)"
    @update:section="(key: string) => (section = key)"
  >
    <template #theme>
      <!-- hikari's own theme panel content: color mode (light/dark/auto
           with the solar cell) and the preset/custom scheme rows. The
           brand preset from @/theme is registered at boot, so its row
           rides the stock list. -->
      <HkThemePanelContent />
    </template>

    <template #language>
      <HkSettingsGroup :title="t('common.chrome.language')">
        <HkLocalePicker
          :locales="localeCodes"
          :current-locale="locale"
          @update:current-locale="(code: string) => chooseLocale(code)"
        />
      </HkSettingsGroup>
    </template>

    <template #about>
      <!-- THE family about body — the same identity surface every
           Celestia front end mounts (no decorative backdrop). -->
      <HkAboutBody
        app-name="0721"
        :version="appVersion"
        logo-src="/logo.webp"
        :tagline="t('common.about.tagline')"
        :credits="credits"
        :licenses="licenses"
        :component-versions="componentRows"
        :copyright="t('common.about.copyright')"
      />
    </template>
  </HkSettingsDialog>
</template>

<style scoped lang="scss">
.locale-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
}

.locale-row {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  width: 100%;
  padding: 0.5rem 0.75rem;
  border: none;
  border-radius: 0.5rem;
  background: transparent;
  color: inherit;
  cursor: pointer;
  text-align: start;

  &:hover {
    background: color-mix(in srgb, currentColor 8%, transparent);
  }
}

.locale-code {
  margin-inline-start: auto;
  opacity: 0.6;
  font-size: 0.85em;
}

.locale-check {
  opacity: 0.9;
}
</style>
