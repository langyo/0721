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
import { HkToast, useToast } from "@celestia-island/hikari";
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

/** Every mounted toast outlet — cleanup owns their lifetime. */
const outlets: Array<{ unmount: () => void }> = [];

function mountView(): ReturnType<typeof mount> {
  const pinia = createPinia();
  setActivePinia(pinia);
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: "/", component: { template: "<div/>" } }],
  });
  const Harness = {
    components: { SettingsView },
    template: "<div><SettingsView /></div>",
  };
  const host = document.createElement("div");
  document.body.appendChild(host);
  const mounted = mount(Harness, { global: { plugins: [pinia, router, i18n] }, attachTo: host });
  // The family toast outlet renders the toasts this view raises (the
  // inline `save-error` span is gone — the user's hard rule).
  outlets.push(mount(HkToast, { attachTo: document.body, props: { teleport: false } }));
  return mounted;
}

/** The latest message on the toast surface (empty when nothing is shown). */
function toastText(): string {
  const nodes = document.querySelectorAll(".hk-toast-message");
  return nodes.length > 0 ? (nodes[nodes.length - 1]!.textContent ?? "") : "";
}

/** A page button by its visible label. */
function buttonByText(label: string): HTMLButtonElement | null {
  return (
    Array.from(document.querySelectorAll<HTMLButtonElement>("button")).find(
      (b) => b.textContent?.trim() === label,
    ) ?? null
  );
}

/** The teleported dialogs' footer action carrying this label — a dialog
 *  confirm shares its label with the page button that opened it, so the
 *  scope (inside `.hk-modal-content`) is what disambiguates. */
function dialogAction(label: string): HTMLButtonElement | null {
  return (
    Array.from(
      document.querySelectorAll<HTMLButtonElement>(".hk-modal-content button"),
    ).find((b) => b.textContent?.trim() === label) ?? null
  );
}

/** Fill a family input found by its data-testid (its real DOM node). */
async function typeIntoTestId(testId: string, value: string): Promise<void> {
  const input = document.querySelector<HTMLInputElement>(`[data-testid="${testId}"]`);
  expect(input, `input ${testId} must be on screen`).toBeTruthy();
  input!.value = value;
  input!.dispatchEvent(new Event("input", { bubbles: true }));
  await flushPromises();
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
  outlets.splice(0).forEach((o) => o.unmount());
  // The toast store is module-level (hikari state) — clear it so the next
  // case's outlet renders only ITS OWN messages.
  const toast = useToast();
  for (const slot of toast.toasts.slice()) toast.remove(slot.id);
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

    // Save through the page's own save button.
    const save = buttonByText(i18n.global.t("common.actions.save"));
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

describe("SettingsView save feedback (toast, not an inline span)", () => {
  it("confirms a successful document save with a toast, not a page flash", async () => {
    // MUT:settings-saved-flash — the SFC grew an inline `role="status"`
    // span beside the Save button and retired it on a 2.4 s timer; that
    // is the page-inserted transient notice the user banned. Restoring
    // the span reddens this case.
    putSiteConfigMock.mockResolvedValue({});
    wrapper = mountView();
    await flushPromises();

    const save = buttonByText(i18n.global.t("common.actions.save"));
    expect(save).toBeTruthy();
    save!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();

    await vi.waitFor(() => {
      expect(toastText()).toBe(i18n.global.t("common.settings.saved"));
    });
    // And no such span exists in the page body any more.
    expect(document.querySelector(".save-flash")).toBeNull();
    expect(document.querySelector(".site-settings [role='status']")).toBeNull();
  });

  it("raises a failed document save as a toast", async () => {
    putSiteConfigMock.mockRejectedValue(new Error("rpc timeout"));
    wrapper = mountView();
    await flushPromises();

    const save = buttonByText(i18n.global.t("common.actions.save"));
    expect(save).toBeTruthy();
    save!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();

    await vi.waitFor(() => {
      expect(toastText()).toBe("rpc timeout");
    });
    // The success flash slot renders for successes only; the error span it
    // used to alternate with is gone from the page body. ([role="alert"]
    // itself is the TOAST's now, so the absence is asserted in the body,
    // where the removed span used to grow — never document-wide.)
    expect(document.querySelector(".site-settings .save-error")).toBeNull();
    expect(document.querySelector(".site-settings [role='alert']")).toBeNull();
    expect(wrapper.find(".save-error").exists()).toBe(false);
  });

  it("raises a failed password kill-switch write as a toast", async () => {
    putPasswordLoginMock.mockRejectedValue(new Error("write refused"));
    wrapper = mountView();
    await flushPromises();

    const toggle = document.querySelector<HTMLElement>('[data-testid="password-login-switch"]');
    expect(toggle).toBeTruthy();
    const trigger = toggle!.querySelector<HTMLElement>("button, [role='switch'], input") ?? toggle!;
    trigger.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();

    expect(putPasswordLoginMock).toHaveBeenCalledTimes(1);
    await vi.waitFor(() => {
      expect(toastText()).toBe("write refused");
    });
    expect(document.querySelector(".site-settings .save-error")).toBeNull();
    expect(document.querySelector(".site-settings [role='alert']")).toBeNull();
  });
});

describe("SettingsView footer banner (dialog, never an inline row)", () => {
  it("collects one banner in a dialog and appends only on confirm", async () => {
    wrapper = mountView();
    await flushPromises();
    // The list starts empty and NOTHING in it is editable.
    expect(document.querySelectorAll(".banner-row").length).toBe(0);

    const add = buttonByText(i18n.global.t("common.settings.addBanner"));
    expect(add, "the banner entry point must be on screen").toBeTruthy();
    add!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();

    // The dialog opened — and the page body did NOT grow an input row.
    expect(document.querySelector('[data-testid="banner-dialog-text"]')).toBeTruthy();
    expect(document.querySelectorAll(".banner-row").length).toBe(0);

    await typeIntoTestId("banner-dialog-text", "Built with care");
    await typeIntoTestId("banner-dialog-url", "https://celestia.world");

    const confirm = dialogAction(i18n.global.t("common.actions.add"));
    expect(confirm, "the dialog must carry its own confirm action").toBeTruthy();
    confirm!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();

    // The value landed in the FORM list, rendered read-only.
    const rows = document.querySelectorAll(".banner-row");
    expect(rows.length).toBe(1);
    // The row is DISPLAY: no input inside it (an inline edit control would
    // be exactly the DOM growth the rule forbids).
    expect(rows[0]!.querySelector("input")).toBeNull();
    expect(document.querySelector('[data-testid="banner-text-0"]')?.textContent).toBe("Built with care");
    expect(document.querySelector('[data-testid="banner-url-0"]')?.textContent).toBe("https://celestia.world");
    // The dialog closed after the confirm.
    expect(document.querySelector('[data-testid="banner-dialog-text"]')).toBeNull();

    // And the entry ships through the ordinary whole-document save.
    const save = buttonByText(i18n.global.t("common.actions.save"));
    save!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    const payload = putSiteConfigMock.mock.calls[0]![0] as {
      portal: { "footer-banner": Array<{ text: string; url?: string }> };
    };
    expect(payload.portal["footer-banner"]).toEqual([
      { text: "Built with care", url: "https://celestia.world" },
    ]);
  });

  it("refuses to append an empty banner and deletes a row from the list", async () => {
    wrapper = mountView();
    await flushPromises();

    buttonByText(i18n.global.t("common.settings.addBanner"))!.dispatchEvent(
      new MouseEvent("click", { bubbles: true }),
    );
    await flushPromises();
    // Confirm with no text: the dialog's primary action is disabled.
    const primary = dialogAction(i18n.global.t("common.actions.add"));
    expect(primary).toBeTruthy();
    expect(primary!.disabled).toBe(true);
    primary!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    expect(document.querySelectorAll(".banner-row").length).toBe(0);

    // Cancel closes without appending.
    buttonByText(i18n.global.t("common.actions.cancel"))!.dispatchEvent(
      new MouseEvent("click", { bubbles: true }),
    );
    await flushPromises();
    expect(document.querySelector('[data-testid="banner-dialog-text"]')).toBeNull();

    // Now a real entry, then its per-row delete.
    buttonByText(i18n.global.t("common.settings.addBanner"))!.dispatchEvent(
      new MouseEvent("click", { bubbles: true }),
    );
    await flushPromises();
    await typeIntoTestId("banner-dialog-text", "Only one");
    dialogAction(i18n.global.t("common.actions.add"))!.dispatchEvent(
      new MouseEvent("click", { bubbles: true }),
    );
    await flushPromises();
    expect(document.querySelectorAll(".banner-row").length).toBe(1);

    const remove = document.querySelector<HTMLButtonElement>(
      `.banner-row button[aria-label="${i18n.global.t("common.actions.remove")}"]`,
    );
    expect(remove).toBeTruthy();
    remove!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    expect(document.querySelectorAll(".banner-row").length).toBe(0);
  });
});

describe("SettingsView referrer-host allow-list (dialog, never an inline row)", () => {
  it("collects one host in a dialog and appends only on confirm", async () => {
    wrapper = mountView();
    await flushPromises();
    expect(document.querySelectorAll(".banner-row").length).toBe(0);

    // The referrer section's own "Add" button.
    const add = buttonByText(i18n.global.t("common.actions.add"));
    expect(add, "the host entry point must be on screen").toBeTruthy();
    add!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();

    expect(document.querySelector('[data-testid="host-dialog-input"]')).toBeTruthy();
    expect(document.querySelectorAll(".banner-row").length).toBe(0);

    await typeIntoTestId("host-dialog-input", "images.example.org");
    dialogAction(i18n.global.t("common.actions.add"))!.dispatchEvent(
      new MouseEvent("click", { bubbles: true }),
    );
    await flushPromises();

    const rows = document.querySelectorAll(".banner-row");
    expect(rows.length).toBe(1);
    expect(rows[0]!.querySelector("input")).toBeNull();
    expect(document.querySelector('[data-testid="referrer-host-0"]')?.textContent).toBe("images.example.org");
    expect(document.querySelector('[data-testid="host-dialog-input"]')).toBeNull();

    const save = buttonByText(i18n.global.t("common.actions.save"));
    save!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    const payload = putSiteConfigMock.mock.calls[0]![0] as {
      router: { "limit-referrer-host": string[] };
    };
    expect(payload.router["limit-referrer-host"]).toEqual(["images.example.org"]);
  });

  it("grows no input row anywhere in the page body when a list grows", async () => {
    // The hard user rule, pinned structurally: adding an entry must never
    // insert an editable control beside the button. The form's OWN fields
    // all live in `.field`; a banner/host row holds text and a delete only.
    wrapper = mountView();
    await flushPromises();
    const before = document.querySelectorAll(".site-settings input").length;

    buttonByText(i18n.global.t("common.actions.add"))!.dispatchEvent(
      new MouseEvent("click", { bubbles: true }),
    );
    await flushPromises();
    // While the dialog is open, the page body's input count is unchanged
    // (the dialog's field lives in the teleported overlay).
    expect(document.querySelectorAll(".site-settings input").length).toBe(before);

    await typeIntoTestId("host-dialog-input", "cdn.example.org");
    dialogAction(i18n.global.t("common.actions.add"))!.dispatchEvent(
      new MouseEvent("click", { bubbles: true }),
    );
    await flushPromises();

    // After the confirm the list grew by exactly one READ-ONLY row: still
    // zero new inputs in the page body.
    expect(document.querySelectorAll(".site-settings .banner-row").length).toBe(1);
    expect(document.querySelectorAll(".site-settings input").length).toBe(before);
  });
});