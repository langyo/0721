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

import ProvidersView from "./ProvidersView.vue";
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
      provideActionBar();
    },
    template: "<div><ProvidersView /></div>",
  };
  return mount(Harness, { global: { plugins: [pinia, router, i18n] } });
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
    expect(head).not.toContain(i18n.global.t("common.providers.title"));
  });

  it("masks the secret and offers a warning badge when none is stored", async () => {
    listProvidersMock.mockResolvedValue([
      provider(),
      provider({ id: "gh", display_name: "GitHub", has_secret: false, secret_tail: null }),
    ]);
    wrapper = mountView();
    await flushPromises();
    expect(wrapper.text()).toContain("••••1234");
    expect(wrapper.text()).toContain(i18n.global.t("common.providers.noSecret"));
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