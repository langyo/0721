import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The chrome settings dialog had NO test, which is how an inert language
// picker shipped: the family picker emits `select`, and binding the wrong
// event name type-checks (an unhandled listener lands in $attrs and is
// ignored at runtime). This file exists so that class cannot repeat.
vi.mock("@/api", () => ({ health: vi.fn().mockRejectedValue(new Error("no net")) }));

import SettingsDialog from "./SettingsDialog.vue";
import { createAppI18n, setLocale } from "@/i18n";

const i18n = createAppI18n();

function mountDialog(): ReturnType<typeof mount> {
  const host = document.createElement("div");
  document.body.appendChild(host);
  return mount(SettingsDialog, {
    props: { modelValue: true },
    global: { plugins: [i18n] },
    attachTo: host,
  });
}

let wrapper: ReturnType<typeof mount> | null = null;

beforeEach(() => {
  wrapper = null;
  setLocale("en");
});

afterEach(() => {
  wrapper?.unmount();
});

describe("SettingsDialog", () => {
  it("switches the app locale from the family picker", async () => {
    wrapper = mountDialog();
    await flushPromises();

    // The dialog opens on the theme section; switch to the language one.
    const navRows = Array.from(document.querySelectorAll<HTMLElement>("[class*='settings'] button, .hk-modal-content nav button"));
    const languageNav = navRows.find((b) => (b.textContent ?? "").trim() === i18n.global.t("common.chrome.language"));
    expect(languageNav, `nav: ${navRows.map((n) => n.textContent?.trim()).join("|")}`).toBeTruthy();
    languageNav!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();

    // Open the locale picker's trigger, then click a language row.
    const trigger = document.querySelector<HTMLElement>(".hk-locale-picker-trigger, [aria-haspopup]");
    expect(trigger, "the language section must render the family picker").toBeTruthy();
    trigger!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();

    // setLocale writes the module's app instance and localStorage; assert
    // on the observable state rather than a local instance.
    expect(localStorage.getItem("i0721-locale") ?? "en").toBe("en");

    // The picker lists locales; pick one that is not the current locale.
    const rows = Array.from(
      document.querySelectorAll<HTMLElement>(".hk-menu-row, [role='option']"),
    );
    expect(rows.length, `rows: ${rows.map((r) => r.textContent?.trim()).join("|")}`).toBeGreaterThan(0);
    const target = rows.find((r) => (r.textContent ?? "").includes("简体中文"));
    expect(target, "the picker must offer the Chinese row").toBeTruthy();
    target!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();

    // MUT:locale-picker-event — binding the wrong event name (the shipped
    // bug: `update:current-locale` instead of `select`) leaves this red.
    expect(localStorage.getItem("i0721-locale")).toBe("zh-Hans");
    expect(document.documentElement.lang).toBe("zh-Hans");
  });
});
