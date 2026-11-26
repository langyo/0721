import {
  HkAboutBody,
  HkLocalePicker,
  HkSettingsDialog,
  HkSettingsGroup,
  HkThemePanelContent,
  type HAboutComponentVersion,
  type HAboutCredit,
  type HAboutLink,
} from "@celestia-island/hikari";
import { Info, Languages, Palette } from "lucide-vue-next";
import { appVersionLabel } from "@/lib/versionLabel";
import { computed, defineComponent, onMounted, ref, type PropType } from "vue";
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
export default defineComponent({
  name: "SettingsDialog",
  props: {
    modelValue: { type: Boolean as PropType<boolean>, required: true },
  },
  emits: {
    "update:modelValue": (_value: boolean) => true,
  },
  setup(props, { emit }) {
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
    const appVersion: string = appVersionLabel();

    const componentRows = computed<HAboutComponentVersion[]>(() => {
      const rows: HAboutComponentVersion[] = [];
      if (report.value?.version) {
        rows.push({ label: t("common.about.engineVersion"), value: report.value.version });
      }
      return rows;
    });

    return () => (
      <HkSettingsDialog
        modelValue={props.modelValue}
        title={t("common.chrome.settings")}
        sections={sections}
        section={section.value}
        navLabel={t("common.chrome.settings")}
        width="46rem"
        onUpdate:modelValue={(v: boolean) => emit("update:modelValue", v)}
        onUpdate:section={(key: string) => (section.value = key)}
      >
        {{
          theme: () => (
            // hikari's own theme panel content: color mode (light/dark/auto
            // with the solar cell) and the preset/custom scheme rows. The
            // brand preset from @/theme is registered at boot, so its row
            // rides the stock list.
            <HkThemePanelContent />
          ),

          language: () => (
            <HkSettingsGroup title={t("common.chrome.language")}>
              <HkLocalePicker
                locales={localeCodes.value}
                currentLocale={locale.value}
                onSelect={(code: string) => chooseLocale(code)}
              />
            </HkSettingsGroup>
          ),

          about: () => (
            // THE family about body — the same identity surface every
            // Celestia front end mounts (no decorative backdrop).
            <HkAboutBody
              appName="0721"
              version={appVersion}
              logoSrc="/logo.webp"
              tagline={t("common.about.tagline")}
              credits={credits.value}
              licenses={licenses}
              componentVersions={componentRows.value}
              copyright={t("common.about.copyright")}
            />
          ),
        }}
      </HkSettingsDialog>
    );
  },
});
