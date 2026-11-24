import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createMemoryHistory, createRouter, type Router } from "vue-router";
import { createPinia, setActivePinia, type Pinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Environment note: the transport/engine probes are the shell's
// environment, not the subject — no RPC client is installed under the
// runner and the health fetch must not leave the process. Both
// composables still run for real (disconnected/null without a client).
// comment-gate-ok: describes the harness, not a behaviour the code must
// exhibit — no assertion depends on it.
vi.mock("@/api", () => ({ health: vi.fn().mockRejectedValue(new Error("no net")) }));

import { HkMenu, HkStatusBar } from "@celestia-island/hikari";

import AppShell from "./AppShell";
import { useAuthStore } from "@/stores/auth";
import { createAppI18n } from "@/i18n";
import type { UserDto } from "@/types";

/**
 * The family chrome contract (the W1 sidebar regrouping): the sidebar
 * renders the console's grouped table of contents — content / access
 * always, the system group ONLY for admins — and every item navigates
 * by route. A regression that flattens the groups or leaks the admin
 * pages to a member fails here.
 *
 * MUT:admin-gating-forced-true — flipping the shell's `if (auth.isAdmin)`
 * to `if (true)` reddens the member test on the group label; with the
 * label pins neutralized it still reddens on the sidebar group COUNT
 * (`expected 3 to be 2`), so the gating is pinned twice, independently.
 */

const i18n = createAppI18n();

function user(role: UserDto["role"]): UserDto {
  return {
    id: "u1",
    username: "tester",
    display_name: "Tester",
    role,
    created_at: "2026-11-23T00:00:00Z",
  };
}

async function mountShell(role: UserDto["role"]): Promise<VueWrapper> {
  const pinia: Pinia = createPinia();
  setActivePinia(pinia);
  const auth = useAuthStore();
  auth.$patch({ user: user(role) });
  const router: Router = createRouter({
    history: createMemoryHistory(),
    routes: [
      {
        path: "/",
        component: AppShell,
        children: [
          {
            path: "",
            component: { template: "<div class='stub-page'>portal</div>" },
            meta: { titleKey: "common.portal.title" },
          },
        ],
      },
    ],
  });
  await router.push("/");
  await router.isReady();
  return mount(AppShell, {
    global: { plugins: [pinia, router, i18n] },
  });
}

/** The SIDEBAR menu (the shell's own nav), not the header's avatar menu —
 * both are HkMenu instances. */
function sidebarGroups(wrapper: VueWrapper): number {
  const sidebar = wrapper
    .findAllComponents(HkMenu)
    .find((menu) => menu.props("variant") === "sidebar");
  expect(sidebar, "sidebar HkMenu must exist").toBeTruthy();
  return (sidebar!.props("items") as unknown[]).length;
}

let wrapper: VueWrapper | null = null;

beforeEach(() => {
  wrapper = null;
});

afterEach(() => {
  wrapper?.unmount();
  // Restored here, not at the end of the test body: a failing assertion
  // must not leak the stubbed fetch into later tests of this file.
  vi.unstubAllGlobals();
});

describe("AppShell (family grouped chrome)", () => {
  it("renders the content and access groups for a member, never the system group", async () => {
    wrapper = await mountShell("member");
    await flushPromises();
    const text = wrapper.text();
    expect(text).toContain(i18n.global.t("common.nav.groupContent"));
    expect(text).toContain(i18n.global.t("common.nav.groupAccess"));
    expect(text).not.toContain(i18n.global.t("common.nav.groupSystem"));
    // A second, independent pin on the same gating: the admin pages'
    // own labels must not render for a member either (with this, the
    // guarantee no longer rests on the group-label line alone).
    // providers.title is value-unique across all 12 locales (users.title
    // shares status.member's in de/ja/ko/zh-Hans/zh-Hant, nav.settings
    // shares chrome.settings' in ar/en/ja/ko); a colliding label would
    // let this negative assertion mis-target the wrong string.
    expect(text).not.toContain(i18n.global.t("common.providers.title"));
    // And the gating is pinned on the DATA path too, independent of any
    // label: the menu receives two groups for a member, three for an
    // admin. Label assertions above cannot cover a wiring change that
    // keeps labels out of the DOM but hands the menu a third group.
    expect(sidebarGroups(wrapper!)).toBe(2);
    // The member's own pages ride the content/access groups.
    expect(text).toContain(i18n.global.t("common.files.title"));
    expect(text).toContain(i18n.global.t("common.keys.title"));
  });

  it("mounts the system group for an admin with its three pages", async () => {
    wrapper = await mountShell("admin");
    await flushPromises();
    const text = wrapper.text();
    for (const key of [
      "common.nav.groupContent",
      "common.nav.groupAccess",
      "common.nav.groupSystem",
    ]) {
      expect(text).toContain(i18n.global.t(key));
    }
    expect(text).toContain(i18n.global.t("common.users.title"));
    expect(text).toContain(i18n.global.t("common.providers.title"));
    expect(text).toContain(i18n.global.t("common.nav.settings"));
    expect(sidebarGroups(wrapper!)).toBe(3);
  });

  it("feeds the family status bar the live engine identity and probe state", async () => {
    // The HkStatusBar prop-name seam is untyped (Vue fallthrough attrs
    // type-check clean), so the wiring is pinned behaviourally: a real
    // health payload must arrive on the bar's engineVersion prop.
    // MUT:statusbar-prop-name-typo — renaming the prop to
    // `:engine-vresion` leaves vue-tsc clean and reddens THIS test with
    // `expected null to be 'sentinel-engine-9.9.9'` (verified twice).
    // useEngineHealth reads raw fetch("/api/health") — stub the global.
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: true,
        json: async () => ({ status: "ok", version: "sentinel-engine-9.9.9", config_source: "settings" }),
      }),
    );
    wrapper = await mountShell("member");
    await flushPromises();
    const bar = wrapper.getComponent(HkStatusBar);
    expect(bar.props("engineVersion")).toBe("sentinel-engine-9.9.9");
    expect(bar.props("version")).toBe(__APP_VERSION__);
    // No RPC client in a unit test: the transport probe reads disconnected.
    expect(bar.props("connectionStatus")).toBe("disconnected");
  });

  it("renders the page into the shell's content host", async () => {
    wrapper = await mountShell("member");
    await flushPromises();
    expect(wrapper.find(".stub-page").exists()).toBe(true);
  });
});
