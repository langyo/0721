import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const listProvidersMock = vi.fn();
const upsertProviderMock = vi.fn();
vi.mock("@/api", () => ({
  listProviders: (...args: unknown[]) => listProvidersMock(...args),
  upsertProvider: (...args: unknown[]) => upsertProviderMock(...args),
  deleteProvider: vi.fn(),
}));

import ProvidersView from "./ProvidersView";
import { provideActionBar } from "@/composables/useActionBar";
import { createAppI18n } from "@/i18n";
import type { OAuthProviderPublic } from "@/types";

/**
 * The provider roster's family shape: the header row carries real column
 * labels (the name column is NOT titled with the page title — that read
 * as a duplicated heading), the secret renders as a mask rather than a
 * value, and the enable switch is wired to the toggle call.
 */

const i18n = createAppI18n();

function provider(over: Partial<OAuthProviderPublic> = {}): OAuthProviderPublic {
  return {
    id: "erp",
    display_name: "Celestia ID",
    type: "builtin:erp",
    enabled: true,
    has_secret: true,
    secret_tail: "1234",
    ...over,
  } as OAuthProviderPublic;
}

function mountView(): ReturnType<typeof mount> {
  const pinia = createPinia();
  setActivePinia(pinia);
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: "/", component: { template: "<div/>" } }],
  });
  const Harness = {
    components: { ProvidersView },
    setup() {
      const bar = provideActionBar();
      return { bar };
    },
    template:
      '<div><div class="harness-actions">' +
      '<component :is="() => bar.actions.value.flatMap((r) => r())" />' +
      "</div><ProvidersView /></div>",
  };
  // attachTo so the teleported dialog and the bar's buttons are reachable
  // from `document` (the dialogs mount outside the wrapper tree).
  const host = document.createElement("div");
  document.body.appendChild(host);
  return mount(Harness, { global: { plugins: [pinia, router, i18n] }, attachTo: host });
}

let wrapper: ReturnType<typeof mount> | null = null;

beforeEach(() => {
  wrapper = null;
  listProvidersMock.mockReset().mockResolvedValue([]);
  upsertProviderMock.mockReset();
});

afterEach(() => {
  wrapper?.unmount();
});

describe("ProvidersView (family table roster)", () => {
  it("titles the name column with a column label, not the page title", async () => {
    listProvidersMock.mockResolvedValue([provider()]);
    wrapper = mountView();
    await flushPromises();
    const head = wrapper.find("thead").text();
    expect(head).toContain(i18n.global.t("common.providers.displayName"));
    // The page title is the page header, never a column label.
    // MUT:provider-column-label — retitling the column with the page
    // title reddens the assertion above.
    expect(head).not.toContain(i18n.global.t("common.providers.title"));
  });

  it("masks the secret and offers a warning badge when none is stored", async () => {
    listProvidersMock.mockResolvedValue([
      provider(),
      provider({ id: "gh", display_name: "GitHub", has_secret: false }),
    ]);
    wrapper = mountView();
    await flushPromises();
    // Row-scoped on purpose: whole-document contains would pass with the
    // mask/value mapping inverted (both strings present somewhere), and
    // a secret's tail must never appear on the row that has no secret.
    // MUT:mask-inversion — flipping the badge condition reddens this
    // test with the tail expected on the wrong row.
    const rows = wrapper.findAll("tbody tr");
    expect(rows[0]!.text()).toContain("••••1234");
    expect(rows[0]!.text()).not.toContain(i18n.global.t("common.providers.noSecret"));
    expect(rows[1]!.text()).toContain(i18n.global.t("common.providers.noSecret"));
    expect(rows[1]!.text()).not.toContain("1234");
  });

  it("reports a non-Error rejection instead of a silent empty roster", async () => {
    listProvidersMock.mockRejectedValue(undefined);
    wrapper = mountView();
    await flushPromises();
    expect(wrapper.find(".hk-alert").exists()).toBe(true);
    expect(wrapper.text()).toContain(i18n.global.t("common.status.error"));
    wrapper!.unmount();

    // And the bare-string arm (a JSON-RPC lane's common shape) carries
    // the string itself.
    listProvidersMock.mockRejectedValue("rpc timeout");
    wrapper = mountView();
    await flushPromises();
    expect(wrapper.find(".hk-alert").exists()).toBe(true);
    expect(wrapper.text()).toContain("rpc timeout");
  });

  it("announces a failed toggle with the rejection text instead of snapping back silently", async () => {
    listProvidersMock.mockResolvedValue([provider({ enabled: false })]);
    upsertProviderMock.mockRejectedValue("rpc timeout");
    wrapper = mountView();
    await flushPromises();
    const toggle = wrapper.find('input[type="checkbox"]');
    await toggle.setValue(true);
    await flushPromises();
    // The toggle's own alert lane (role=alert above the table) carries
    // the string; the row's optimistic flip snaps back.
    const alert = wrapper.find('[role="alert"]');
    expect(alert.exists()).toBe(true);
    expect(alert.text()).toContain("rpc timeout");
  });

  it("picks the provider kind through the family radio", async () => {
    listProvidersMock.mockResolvedValue([provider()]);
    wrapper = mountView();
    await flushPromises();

    const create = Array.from(document.querySelectorAll<HTMLButtonElement>("button")).find(
      (b) => (b.getAttribute("aria-label") ?? "").length > 0 && b.className.includes("hk-icon-button"),
    );
    expect(create, "the harness must render the create action").toBeTruthy();
    create!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();

    // The family radio renders real inputs — the old markup was buttons
    // with role="radio" inside a bespoke .type-picker container.
    const radios = Array.from(document.querySelectorAll<HTMLInputElement>('input[type="radio"]'));
    expect(radios.length).toBe(4);
    expect(document.querySelector(".type-picker")).toBeNull();

    // Pick "oidc", fill the required fields, submit, and check the payload
    // carries the selected kind.
    radios[3]!.click();
    await flushPromises();
    upsertProviderMock.mockResolvedValue(undefined);
    const inputs = Array.from(document.querySelectorAll<HTMLInputElement>(".hk-dialog input, [role='dialog'] input"))
      .filter((i) => i.type !== "radio" && i.type !== "checkbox");
    expect(inputs.length).toBeGreaterThanOrEqual(3);
    inputs[0]!.value = "newprov";
    inputs[0]!.dispatchEvent(new Event("input", { bubbles: true }));
    inputs[1]!.value = "New provider";
    inputs[1]!.dispatchEvent(new Event("input", { bubbles: true }));
    inputs[2]!.value = "client-abc";
    inputs[2]!.dispatchEvent(new Event("input", { bubbles: true }));
    await flushPromises();
    const save = Array.from(document.querySelectorAll<HTMLButtonElement>("button.hk-btn-primary"))
      .find((b) => b.textContent?.trim() === i18n.global.t("common.actions.save"));
    expect(save).toBeTruthy();
    save!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    await flushPromises();
    // Unconditional: a save() that returned early would leave zero calls
    // and the old conditional shape passed anyway.
    expect(upsertProviderMock).toHaveBeenCalledTimes(1);
    expect(upsertProviderMock.mock.calls[0]![0]).toMatchObject({ type: "oidc" });
  });

  it("keeps the kind picker a NAMED radiogroup (a11y contract)", async () => {
    listProvidersMock.mockResolvedValue([provider()]);
    wrapper = mountView();
    await flushPromises();
    const create = Array.from(document.querySelectorAll<HTMLButtonElement>("button")).find(
      (b) => (b.getAttribute("aria-label") ?? "").length > 0 && b.className.includes("hk-icon-button"),
    );
    expect(create).toBeTruthy();
    create!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    const group = document.querySelector<HTMLElement>('[role="radiogroup"]');
    expect(group, "the kind picker must be a radiogroup").toBeTruthy();
    expect(group!.getAttribute("aria-label")).toBe(i18n.global.t("common.providers.type"));
    expect(group!.querySelectorAll('input[type="radio"]').length).toBe(4);
  });

  it("wires the enable switch to the toggle call", async () => {
    listProvidersMock.mockResolvedValue([provider({ enabled: false })]);
    upsertProviderMock.mockResolvedValue(undefined);
    wrapper = mountView();
    await flushPromises();
    const toggle = wrapper.find('input[type="checkbox"]');
    expect(toggle.exists()).toBe(true);
    await toggle.setValue(true);
    await flushPromises();
    // The row's own upsert carries the flipped flag (the provider id and
    // the rest of the row ride along unchanged).
    expect(upsertProviderMock).toHaveBeenCalledTimes(1);
    expect(upsertProviderMock.mock.calls[0]![0]).toMatchObject({ id: "erp", enabled: true });
  });
});