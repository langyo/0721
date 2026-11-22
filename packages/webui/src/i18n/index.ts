import { setLocale as setHikariLocale } from "@celestia-island/hikari";
import { detectLocale as detectLocaleShared } from "@celestia-island/hikari/shared";
import { computed, type ComputedRef } from "vue";
import { createI18n, type DefineLocaleMessage, type I18n } from "vue-i18n";

/**
 * Auto-discovered locale messages (the chest i18n shape).
 *
 * Every JSON file under ``res/i18n/locales/<lang>/`` is loaded eagerly and
 * deep-merged into ``defaultMessages[lang]``: a flat file like
 * ``en/common.json`` becomes ``messages.en.common``. Adding a namespace
 * needs no edit here — just drop the file in. ``SUPPORTED_LOCALES``
 * stays explicit because it backs the locale picker UI.
 */
const modules = import.meta.glob("../../../../res/i18n/locales/**/*.json", {
  eager: true,
  import: "default",
}) as Record<string, Record<string, unknown>>;

function setPath(root: Record<string, unknown>, dotted: string, value: unknown): void {
  const parts = dotted.split(".");
  let node = root;
  for (let i = 0; i < parts.length - 1; i += 1) {
    const key = parts[i]!;
    if (typeof node[key] !== "object" || node[key] === null || Array.isArray(node[key])) {
      node[key] = {};
    }
    node = node[key] as Record<string, unknown>;
  }
  node[parts[parts.length - 1]!] = value;
}

export const defaultMessages: Record<string, Record<string, unknown>> = {};
for (const [path, data] of Object.entries(modules)) {
  const rel = path.replace(/^.*\/res\/i18n\/locales\//, "").replace(/\.json$/, "");
  const [lang, ...parts] = rel.split("/");
  if (!lang || parts.length === 0) continue;
  if (!defaultMessages[lang]) defaultMessages[lang] = {};
  setPath(defaultMessages[lang]!, parts.join("."), data);
}

/** The family's 11 locales plus the retained Italian catalog (12th dir). */
export const SUPPORTED_LOCALES = [
  "en",
  "zh-Hans",
  "zh-Hant",
  "ja",
  "ko",
  "de",
  "fr",
  "es",
  "pt",
  "ar",
  "ru",
  "it",
] as const;

export type Locale = (typeof SUPPORTED_LOCALES)[number];

/** Native, self-labelled names for the picker (never translated — a
 * language must be findable by someone who cannot read the UI yet). */
export const LOCALE_LABELS: Record<Locale, string> = {
  en: "English",
  "zh-Hans": "简体中文",
  "zh-Hant": "繁體中文",
  ja: "日本語",
  ko: "한국어",
  de: "Deutsch",
  fr: "Français",
  es: "Español",
  pt: "Português",
  ar: "العربية",
  ru: "Русский",
  it: "Italiano",
};

export const LOCALE_OPTIONS: ComputedRef<{ code: Locale; label: string }[]> = computed(() =>
  SUPPORTED_LOCALES.map((code) => ({ code, label: LOCALE_LABELS[code] })),
);

export function isValidLocale(v: string): v is Locale {
  return (SUPPORTED_LOCALES as readonly string[]).includes(v);
}

const LOCALE_STORAGE_KEY = "i0721-locale";

function detectLocale(): string {
  // An explicit ?locale= query override wins — the recovery path when the
  // stored value is wrong or the picker is unreachable.
  if (typeof window !== "undefined") {
    const query = new URLSearchParams(window.location.search).get("locale");
    if (query && isValidLocale(query)) {
      localStorage.setItem(LOCALE_STORAGE_KEY, query);
      return query;
    }
  }
  const detected = detectLocaleShared(SUPPORTED_LOCALES, { storageKey: LOCALE_STORAGE_KEY });
  try {
    if (!localStorage.getItem(LOCALE_STORAGE_KEY)) {
      localStorage.setItem(LOCALE_STORAGE_KEY, detected);
    }
  } catch {
    // Storage can be unavailable (private mode); detection still works.
  }
  return detected;
}

/** Is this value a vue-i18n message tree (strings / nested dicts only)? */
export function isMessageTree(value: unknown): value is DefineLocaleMessage {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  return Object.entries(value).every(
    ([, v]) => typeof v === "string" || isMessageTree(v),
  );
}

export function createAppI18n(): I18n<Record<string, unknown>, Record<string, unknown>, Record<string, unknown>, string, false> {
  const messages: Record<string, DefineLocaleMessage> = {};
  for (const [locale, msgs] of Object.entries(defaultMessages)) {
    if (!isMessageTree(msgs)) {
      console.warn(`[i18n] locale "${locale}" is not a message tree; skipped`);
      continue;
    }
    messages[locale] = { ...msgs };
  }
  return createI18n({
    legacy: false,
    locale: detectLocale(),
    fallbackLocale: "en",
    messages,
    missingWarn: false,
    fallbackWarn: false,
  });
}

// ── Module-level bridge ────────────────────────────────────────────────
// Non-component code (the api layer, the router guard) translates through
// the registered instance instead of receiving a composer by injection.
// `Legacy = false` pins the global to the Composer API (createAppI18n
// always uses `legacy: false`), so `.global.t` / `.global.locale.value`
// resolve to a single callable shape.
let globalI18n: I18n<
  Record<string, unknown>,
  Record<string, unknown>,
  Record<string, unknown>,
  string,
  false
> | null = null;

export function setGlobalI18n(i18n: I18n): void {
  globalI18n = i18n;
}

export function currentLocale(): string {
  if (globalI18n) return globalI18n.global.locale.value;
  try {
    return localStorage.getItem(LOCALE_STORAGE_KEY) ?? "en";
  } catch {
    return "en";
  }
}

/** Translate outside a component. Falls back to the key if unavailable. */
export function gt(key: string, params?: Record<string, unknown>): string {
  if (!globalI18n) return key;
  return globalI18n.global.t(key, params ?? {});
}

/**
 * Change the active locale — the single source of truth for locale
 * mutation: writes localStorage, updates the reactive i18n instance, and
 * mirrors the locale into hikari's own i18n context so the upstream
 * components (sign-in card, theme panel, settings dialog…) follow the
 * app locale instead of staying English.
 */
export function setLocale(locale: string): void {
  try {
    localStorage.setItem(LOCALE_STORAGE_KEY, locale);
  } catch {
    // Private mode: the choice survives for this session only.
  }
  if (globalI18n) {
    globalI18n.global.locale.value = locale;
  }
  document.documentElement.lang = locale;
  document.documentElement.dir = locale === "ar" ? "rtl" : "ltr";
  void setHikariLocale(locale);
}

export type { I18n };
