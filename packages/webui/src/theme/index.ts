import { themePresets, type ThemePreset } from "@celestia-island/hikari";

/**
 * The 0721 brand theme line.
 *
 * hikari's theme engine exposes no preset-registration API beyond its
 * exported `themePresets` table (the chest pattern), so the brand preset
 * is installed by extending that table. Registration must run BEFORE
 * `initTheme()` so the first `applyTheme()` already resolves it.
 *
 * Unlike chest (which REPLACES the whole preset line with its locked
 * brand), 0721 KEEPS hikari's stock `default` preset and adds the brand
 * scheme beside it: the deployment (res.celestia.world) gets its own
 * identity, while the factory scheme stays available for personal
 * installs of the open-source tree.
 *
 * The palette samples the site's own brand surfaces: the deployment
 * candidate's sky/sea teal family on a pure white / near-black floor.
 */
const rgb = (r: number, g: number, b: number) => ({ r, g, b });

export const RES_THEME_ID = "res";

export const brandPresets: Record<string, ThemePreset> = {
  [RES_THEME_ID]: {
    id: RES_THEME_ID,
    // A literal display name (not a hikari:: key): the brand is one line,
    // not a per-locale concept — hikari's `t()` passes non-key strings
    // through unchanged, so the row reads "0721" in every locale.
    name: "0721",
    dark: {
      primary: rgb(45, 212, 191),      // teal-300, lifted for dark bg
      secondary: rgb(125, 211, 252),   // sky-300
      accent: rgb(52, 211, 153),       // emerald-400
      text: rgb(236, 254, 255),
      muted: rgb(148, 163, 184),
      border: rgb(255, 255, 255),
      focusedBorder: rgb(45, 212, 191),
      background: rgb(4, 12, 16),      // near-black sea floor
      surface: rgb(15, 32, 39),
      selectedBackground: rgb(19, 52, 54),
      selectedText: rgb(240, 253, 250),
      statusBarBackground: rgb(10, 24, 29),
      success: rgb(47, 191, 113),
      error: rgb(241, 90, 90),
      warning: rgb(217, 138, 43),
      info: rgb(56, 189, 248),
      onSolidText: rgb(3, 20, 22),
      onSolidIcon: rgb(3, 20, 22),
    },
    light: {
      primary: rgb(13, 148, 136),      // teal-600
      secondary: rgb(2, 132, 199),     // sky-600
      accent: rgb(5, 150, 105),        // emerald-600
      text: rgb(15, 35, 40),
      muted: rgb(87, 104, 110),
      border: rgb(15, 23, 42),
      focusedBorder: rgb(13, 148, 136),
      background: rgb(255, 255, 255),  // pure white floor
      surface: rgb(255, 255, 255),
      selectedBackground: rgb(204, 251, 241),
      selectedText: rgb(13, 55, 51),
      statusBarBackground: rgb(240, 253, 250),
      success: rgb(22, 128, 61),
      error: rgb(200, 30, 30),
      warning: rgb(180, 83, 9),
      info: rgb(14, 122, 189),
      onSolidText: rgb(255, 255, 255),
      onSolidIcon: rgb(255, 255, 255),
    },
  },
};

/**
 * Install the brand presets into hikari's preset table. Idempotent; call
 * before `initTheme()`. Stock presets are kept (see the module doc).
 */
/**
 * Declare the deployment default through hikari's page seam
 * (`window.__celestiaDefaultTheme`, the same field the family boot
 * loaders honor): a selection-less visitor lands on the brand, while a
 * stored or user-chosen selection always wins and the factory scheme
 * stays selectable in the picker for personal installs. Idempotent;
 * call before `initTheme()` (see main.ts).
 */
export function declareBrandDefault(): void {
  (window as unknown as { __celestiaDefaultTheme?: string }).__celestiaDefaultTheme =
    RES_THEME_ID;
}

export function registerBrandPresets(): void {
  const table = themePresets as Record<string, ThemePreset>;
  for (const preset of Object.values(brandPresets)) {
    table[preset.id] = preset;
  }
}
