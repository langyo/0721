import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The site-settings surface had no test at all until this wave rewrote its
// language control; these cases pin the round trip the rewrite risks.
const getSiteConfigMock = vi.fn();
const putSiteConfigMock = vi.fn();
const getPasswordLoginMock = vi.fn();
const putPasswordLoginMock = vi.fn();
vi.mock("@/api", () => ({
  getSiteConfig: (...a: unknown[]) => getSiteConfigMock(...a),
  putSiteConfig: (...a: unknown[]) => putSiteConfigMock(...a),
  getPasswordLogin: (...a: unknown[]) => getPasswordLoginMock(...a),
  putPasswordLogin: (...a: unknown[]) => putPasswordLoginMock(...a),
}));

import SettingsView from "./SettingsView";
import { provideActionBar } from "@/composables/useActionBar";
import { createAppI18n } from "@/i18n";

const i18n = createAppI18n();

/** The wire form the view loads and saves (only the fields it reads). */
function siteConfig(language = "zh_hans") {
  return {
    portal: {
      "title-suffix": " · 0721",
      "footer-banner": [],
      language,
      timezone: 8,
    },
    router: {
      "media-entry-path": "/media",
      "limit-referrer-host": [],
    },
    upload: {
      "image-size-limit": 10485760,
      "webp-auto-convert": true,
      "use-source-file-name": false,
      "default-quota-bytes": null,
    },
  };
}

function mountView(): ReturnType<typeof mount> {
  const pinia = createPinia();
  setActivePinia(pinia);
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: "/", component: { template: "<div/>" } }],
  });
  const Harness = {
    components: { SettingsView },
    setup() {
      const bar = provideActionBar();
      return { bar };
    },
    template:
      '<div><div class="harness-actions">' +
      '<component :is="() => bar.actions.value.flatMap((r) => r())" />' +
      "</div><SettingsView /></div>",
  };
  const host = document.createElement("div");
  document.body.appendChild(host);
  return mount(Harness, { global: { plugins: [pinia, router, i18n] }, attachTo: host });
}

let wrapper: ReturnType<typeof mount> | null = null;

beforeEach(() => {
  wrapper = null;
  getSiteConfigMock.mockReset().mockResolvedValue(siteConfig());
  putSiteConfigMock.mockReset().mockResolvedValue(undefined);
  getPasswordLoginMock.mockReset().mockResolvedValue(true);
  putPasswordLoginMock.mockReset().mockResolvedValue(undefined);
});

afterEach(() => {
  wrapper?.unmount();
});

describe("SettingsView site config (language control)", () => {
  it("loads the roster and offers the family select, not a native one", async () => {
    wrapper = mountView();
    await flushPromises();
    expect(getSiteConfigMock).toHaveBeenCalled();
    // The native select this wave replaced is gone from the markup.
    expect(document.querySelector("select")).toBeNull();
    expect(document.querySelector(".hk-select-trigger")).toBeTruthy();
  });

  it("saves the language picked through the family select", async () => {
    wrapper = mountView();
    await flushPromises();

    // Open the family select and pick a non-default language from its
    // real option rows.
    const triggers = Array.from(document.querySelectorAll<HTMLButtonElement>(".hk-select-trigger"));
    expect(triggers.length).toBeGreaterThan(0);
    triggers[0]!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    const options = Array.from(document.querySelectorAll<HTMLElement>(".hk-select-option"));
    const japanese = options.find((n) => n.textContent?.trim() === "ja");
    expect(japanese, `options: ${options.map((o) => o.textContent?.trim()).join(",")}`).toBeTruthy();
    japanese!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();

    // Save through the header's action bar (the page's own save entry).
    const save = Array.from(document.querySelectorAll<HTMLButtonElement>("button")).find(
      (b) => b.textContent?.trim() === i18n.global.t("common.actions.save"),
    );
    expect(save, "the page must expose a save action").toBeTruthy();
    save!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    await flushPromises();

    expect(putSiteConfigMock).toHaveBeenCalledTimes(1);
    const payload = putSiteConfigMock.mock.calls[0]![0] as { portal: { language: string } };
    // The picked code must be the one shipped — a control that never
    // wrote back would send the loaded value instead.
    expect(payload.portal.language).toBe("ja");
  });
});