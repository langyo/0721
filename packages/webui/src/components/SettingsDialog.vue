<script setup lang="ts">
import {
  HkButton,
  HkSettingsDialog,
  HkSettingsGroup,
  HkThemePanelContent,
} from "@celestia-island/hikari";
import { Check, Info, Languages, LogOut, Palette } from "lucide-vue-next";
import { onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import { health } from "@/api";
import { LOCALE_OPTIONS, setLocale } from "@/i18n";
import type { HealthReport } from "@/types";

/**
 * The chrome-level settings panel (every signed-in user): theme mode +
 * palette (hikari's own theme panel — light/dark/auto and the preset
 * rows), language, about, and the sign-out row. Site administration
 * lives on its own /settings route, admin-gated; this dialog is the
 * user-facing chrome the header gear opens.
 */

defineProps<{ modelValue: boolean }>();
const emit = defineEmits<{
  "update:modelValue": [value: boolean];
  logout: [];
}>();

const { t, locale } = useI18n();
const report = ref<HealthReport | null>(null);
const section = ref("theme");

onMounted(() => {
  // Fire-and-forget: the about pane degrades to the app name without it.
  void health().then((r) => {
    report.value = r;
  }).catch(() => undefined);
});

const sections = [
  { key: "theme", label: t("common.chrome.theme"), icon: Palette },
  { key: "language", label: t("common.chrome.language"), icon: Languages },
  { key: "about", label: t("common.chrome.about"), icon: Info },
];

function chooseLocale(code: string) {
  setLocale(code);
}
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
        <ul class="locale-list" role="listbox" :aria-label="t('common.chrome.language')">
          <li v-for="option in LOCALE_OPTIONS" :key="option.code" role="option" :aria-selected="option.code === locale">
            <button type="button" class="locale-row" @click="chooseLocale(option.code)">
              <span class="locale-label">{{ option.label }}</span>
              <span class="locale-code">{{ option.code }}</span>
              <Check v-if="option.code === locale" :size="16" class="locale-check" />
            </button>
          </li>
        </ul>
      </HkSettingsGroup>
    </template>

    <template #about>
      <HkSettingsGroup :title="t('common.chrome.about')">
        <p class="about-line">{{ t("common.app.name") }}</p>
        <p v-if="report" class="about-line dim">
          {{ t("common.status.version", { version: report.version }) }}
        </p>
        <HkButton variant="danger" :aria-label="t('common.chrome.signOut')" @click="emit('logout')">
          <template #icon><LogOut :size="16" /></template>
          {{ t("common.chrome.signOut") }}
        </HkButton>
      </HkSettingsGroup>
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

.about-line {
  margin: 0 0 0.5rem;
}

.dim {
  opacity: 0.7;
  margin-bottom: 1rem;
}
</style>
