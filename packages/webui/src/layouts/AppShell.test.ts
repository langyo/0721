import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { createMemoryHistory, createRouter, type Router } from "vue-router";
import { createPinia, setActivePinia, type Pinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The transport/engine probes are the shell's ENVIRONMENT, not the
// subject: the RPC singleton is never installed in a unit test, and
// the health fetch must not touch a network. Both composables still
// run for real (they degrade to disconnected/null without a client).
vi.mock("@/api", () => ({ health: vi.fn().mockRejectedValue(new Error("no net")) }));

import { HkStatusBar } from "@celestia-island/hikari";

import AppShell from "./AppShell.vue";
import { useAuthStore } from "@/stores/auth";
import { createAppI18n } from "@/i18n";
import type { UserDto } from "@/types";

/**
 * The family chrome contract (the W1 sidebar regrouping): the sidebar
 * renders the console's grouped table of contents — content / access
 * always, the system group ONLY for admins — and every item navigates
 * by route. A regression that flattens the groups or leaks the admin
 * pages to a member fails here.
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
    // providers.title is the one admin label that is alias-free in ALL
    // 12 locales (users.title collides with status.member in 5 locales,
    // nav.settings with chrome.settings in 4) — a colliding label would
    // let this negative assertion silently mis-target the wrong string.
    expect(text).not.toContain(i18n.global.t("common.providers.title"));
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
  });

  it("feeds the family status bar the live engine identity and probe state", async () => {
    // The HkStatusBar prop-name seam is untyped (Vue fallthrough attrs
    // type-check clean), so the wiring is pinned BEHAVIOURALLY: a real
    // health payload must arrive on the bar's engineVersion prop (R3 S2).
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
