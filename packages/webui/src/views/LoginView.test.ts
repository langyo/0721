import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRouter, createMemoryHistory } from "vue-router";

// The API layer is mocked at module scope: these tests exercise the
// VIEW's contract (loading state, error surface, redirect), not the
// network. The mock's login reply carries the real AuthSession shape.
const loginMock = vi.fn();
const loginMethodsMock = vi.fn();
vi.mock("@/api", () => ({
  login: (...args: unknown[]) => loginMock(...args),
  loginMethods: (...args: unknown[]) => loginMethodsMock(...args),
}));
vi.mock("@/api/client", () => ({
  ApiError: class ApiError extends Error {
    status: number;
    constructor(status: number, message: string) {
      super(message);
      this.status = status;
    }
  },
  onUnauthorized: vi.fn(),
}));


import { HkLoginFlow, HkStatusBar } from "@celestia-island/hikari";

import LoginView from "./LoginView.vue";
import { ApiError } from "@/api/client";
import { createAppI18n } from "@/i18n";

const session = {
  expires_at: "2026-12-01T00:00:00Z",
  user: {
    id: "0b5a0a48-8cb0-4b8c-9a4f-5d4d3a2f1c00",
    username: "admin",
    display_name: "Admin",
    role: "admin",
    created_at: "2026-11-01T00:00:00Z",
  },
};

function typeInto(input: HTMLInputElement, value: string) {
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

async function mountView(query: Record<string, string> = {}) {
  const i18n = createAppI18n();
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/", name: "portal", component: { template: "<div/>" } },
      { path: "/files", name: "files", component: { template: "<div/>" } },
      { path: "/login", name: "login", component: LoginView },
    ],
  });
  await router.push({ path: "/login", query });
  // attachTo keeps the tree in the live document — teleported panels and
  // document-scoped queries reach it (a detached mount hides them).
  const host = document.createElement("div");
  document.body.appendChild(host);
  const wrapper = mount(LoginView, {
    global: { plugins: [i18n, router, createPinia()] },
    attachTo: host,
  });
  // The login-methods descriptor resolves async — the card mounts after.
  await flushPromises();
  await flushPromises();
  return { wrapper, router };
}

beforeEach(() => {
  loginMock.mockReset();
  loginMethodsMock.mockReset();
  // The login-methods descriptor: the classic password-on shape.
  loginMethodsMock.mockResolvedValue({ password: true, providers: [] });
});

afterEach(() => {
  localStorage.clear();
});

describe("LoginView", () => {
  it("submits the credentials and redirects on success", async () => {
    loginMock.mockResolvedValue(session);
    const { wrapper, router } = await mountView();
    const inputs = wrapper.findAll("input");
    typeInto(inputs[0]!.element as HTMLInputElement, "admin");
    typeInto(inputs[1]!.element as HTMLInputElement, "secret");
    await flushPromises();
    (wrapper.find(".hk-btn-primary").element as HTMLElement).click();
    await flushPromises();
    expect(loginMock).toHaveBeenCalledWith({ username: "admin", password: "secret" });
    // The router left /login…
    expect(router.currentRoute.value.path).toBe("/");
    // …and NOTHING session-shaped was persisted: the credential is the
    // httpOnly cookie (server-planted); the page holds no token.
    expect(localStorage.getItem("i0721-session")).toBeNull();
    expect(localStorage.getItem("i0721-token")).toBeNull();
    // (the locale preference is legitimate storage; session keys are not)
  });

  it("surfaces the localized invalid-credentials error on 401", async () => {
    loginMock.mockRejectedValue(new ApiError(401, "invalid credentials"));
    const { wrapper } = await mountView();
    const inputs = wrapper.findAll("input");
    typeInto(inputs[0]!.element as HTMLInputElement, "admin");
    typeInto(inputs[1]!.element as HTMLInputElement, "wrong");
    await flushPromises();
    (wrapper.find(".hk-btn-primary").element as HTMLElement).click();
    await flushPromises();
    const alert = wrapper.find('[role="alert"]');
    expect(alert.exists()).toBe(true);
    // en baseline wording (the test i18n detects "en").
    expect(alert.text()).toBe("Invalid username or password");
  });

  it("keeps the raw message for non-401 failures", async () => {
    loginMock.mockRejectedValue(new Error("network unreachable"));
    const { wrapper } = await mountView();
    const inputs = wrapper.findAll("input");
    typeInto(inputs[0]!.element as HTMLInputElement, "admin");
    typeInto(inputs[1]!.element as HTMLInputElement, "pw");
    await flushPromises();
    (wrapper.find(".hk-btn-primary").element as HTMLElement).click();
    await flushPromises();
    expect(wrapper.find('[role="alert"]').text()).toBe("network unreachable");
  });
});

describe("LoginView status strip (family footer)", () => {
  it("feeds the family status bar the panel version and the engine identity", async () => {
    // MUT:login-statusbar-seam — the HkStatusBar prop-name seam is untyped
    // (fallthrough attrs type-check clean), so the wiring is pinned the
    // same way as the shell's: a real health payload must reach
    // engineVersion, and the sessionless face must report `idle`.
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: true,
        json: async () => ({ status: "ok", version: "sentinel-engine-9.9.9", config_source: "settings" }),
      }),
    );
    const { wrapper: w } = await mountView();
    await flushPromises();
    const bar = w.getComponent(HkStatusBar);
    expect(bar.props("engineVersion")).toBe("sentinel-engine-9.9.9");
    expect(bar.props("version")).toBe(__APP_VERSION__);
    expect(bar.props("connectionStatus")).toBe("idle");
    // The strip is this face's own footer: standalone must stay OFF, or
    // the bar renders its own fixed <footer> and nests inside ours.
    expect(bar.props("standalone")).toBe(false);
    expect(w.findAll("footer.s-status-bar").length).toBe(0);
    vi.unstubAllGlobals();
  });
});

describe("LoginView dead state (W4 R2)", () => {
  it("shows the localized fallback for an Error with an empty message", async () => {
    // MUT:login-errortext-lane — the old narrowing ternary rendered ""
    // here, and the alert paragraph is falsy-gated: a failed sign-in
    // read as an untouched page.
    loginMock.mockRejectedValue(new Error(""));
    const { wrapper } = await mountView();
    const inputs = wrapper.findAll("input");
    typeInto(inputs[0]!.element as HTMLInputElement, "admin");
    typeInto(inputs[1]!.element as HTMLInputElement, "secret");
    await flushPromises();
    (wrapper.find(".hk-btn-primary").element as HTMLElement).click();
    await flushPromises();
    const alert = wrapper.find('[role="alert"]');
    expect(alert.exists()).toBe(true);
    expect(alert.text()).toBe("Something went wrong");
  });

  it("carries a bare-string rejection", async () => {
    loginMock.mockRejectedValue("rpc timeout");
    const { wrapper } = await mountView();
    const inputs = wrapper.findAll("input");
    typeInto(inputs[0]!.element as HTMLInputElement, "admin");
    typeInto(inputs[1]!.element as HTMLInputElement, "secret");
    await flushPromises();
    (wrapper.find(".hk-btn-primary").element as HTMLElement).click();
    await flushPromises();
    expect(wrapper.find('[role="alert"]').text()).toBe("rpc timeout");
  });
});

describe("LoginView dual-mode (family login panel)", () => {
  it("renders one full-width row per provider and navigates to its OAuth entry", async () => {
    loginMethodsMock.mockResolvedValue({
      password: true,
      providers: [
        { id: "erp", display_name: "Celestia ID", type: "builtin:erp", icon: null },
        { id: "gh", display_name: "GitHub", type: "builtin:github", icon: null },
      ],
    });
    const { wrapper } = await mountView();
    // The password card is the opening channel; the providers ride the
    // card's methods list as family rows (name in plain sight).
    const rows = wrapper.findAll(".hk-auth-channel-row");
    expect(rows.length).toBe(2);
    expect(rows[0]!.text()).toContain("Celestia ID");
    expect(rows[1]!.text()).toContain("GitHub");
    // The rows sit under a labelled divider ("or continue with") — an
    // unlabelled list of destinations reads as part of the form.
    expect(wrapper.text()).toContain("or continue with");

    // Navigation is a FULL page load (the flow plants an httpOnly state
    // cookie and 302s) — pin the href, not a router push.
    const original = window.location.href;
    const hrefSetter = vi.fn();
    Object.defineProperty(window, "location", {
      value: { ...window.location, href: original },
      writable: false,
    });
    try {
      Object.defineProperty(window.location, "href", {
        set: (v: string) => hrefSetter(v),
        get: () => original,
        configurable: true,
      });
      await rows[0]!.trigger("click");
      expect(hrefSetter).toHaveBeenCalledWith("/api/v1/auth/oauth/erp");
    } finally {
      Object.defineProperty(window.location, "href", {
        value: original,
        writable: true,
        configurable: true,
      });
    }
  });

  it("hides the password form and shows the provider row alone when the kill-switch is off", async () => {
    loginMethodsMock.mockResolvedValue({
      password: false,
      providers: [{ id: "erp", display_name: "Celestia ID", type: "builtin:erp", icon: null }],
    });
    const { wrapper } = await mountView();
    await flushPromises();
    // No credential inputs at all — the card is not mounted under a
    // password-off descriptor (the switch is what mounts it).
    expect(wrapper.findAll("input").length).toBe(0);
    // …one full-width provider row instead.
    expect(wrapper.findAll(".hk-auth-channel-row").length).toBe(1);
  });

  it("renders no channel segment group for a single channel (the Celestia-ID-only shape)", async () => {
    // The standing user complaint: a deployment configured for one
    // sign-in method must not show a segmented switch at all — a lone
    // segment reads as a broken control.
    loginMethodsMock.mockResolvedValue({
      password: false,
      providers: [{ id: "erp", display_name: "Celestia ID", type: "builtin:erp", icon: null }],
    });
    const { wrapper } = await mountView();
    await flushPromises();
    expect(wrapper.findAll(".hk-tabs-trigger").length).toBe(0);
  });

  it("keeps ONE channel when the password lane carries the providers as rows", async () => {
    loginMethodsMock.mockResolvedValue({
      password: true,
      providers: [{ id: "erp", display_name: "Celestia ID", type: "builtin:erp", icon: null }],
    });
    const { wrapper } = await mountView();
    await flushPromises();
    // Providers are alternative ROWS under the form, not channel tabs
    // (the family copy rule: a provider name in the segment reads as
    // "log in to ERP" rather than "log in with your organization"), so
    // the panel stays a single channel with no segment group.
    expect(wrapper.findAll(".hk-tabs-trigger").length).toBe(0);
    expect(wrapper.findAll(".hk-auth-channel-row").length).toBe(1);
  });

  it("declares the credentials channel as a two-field account+password form", async () => {
    loginMethodsMock.mockResolvedValue({ password: true, providers: [] });
    const { wrapper } = await mountView();
    await flushPromises();
    // The channel OBJECT is the contract the panel renders from: flipping
    // passwordField would silently turn the card into an account-first
    // funnel, and dropping submitLabel would leave a nameless button.
    const channels = wrapper.getComponent(HkLoginFlow).props("channels") as Array<{
      key: string;
      mode: string;
      passwordField?: boolean;
      submitLabel?: string;
      usernameType?: string;
    }>;
    expect(channels.length).toBe(1);
    expect(channels[0]!.mode).toBe("credentials");
    expect(channels[0]!.passwordField).toBe(true);
    expect(channels[0]!.submitLabel).toBeTruthy();
    expect(channels[0]!.usernameType).toBe("text");
  });

  it("grows the segment group only for a multi-provider password-off deployment", async () => {
    loginMethodsMock.mockResolvedValue({
      password: false,
      providers: [
        { id: "erp", display_name: "Celestia ID", type: "builtin:erp", icon: null },
        { id: "gh", display_name: "GitHub", type: "builtin:github", icon: null },
      ],
    });
    const { wrapper } = await mountView();
    await flushPromises();
    // Two redirect-only identities need a chooser; the provider names are
    // then the honest labels for that choice.
    expect(wrapper.findAll(".hk-tabs-trigger").length).toBe(2);
    expect(wrapper.findAll(".hk-auth-channel-row").length).toBe(1);
  });
});

describe("LoginView sso lane (family panel)", () => {
  it("navigates the whole page from the sso row when the password lane is off", async () => {
    // The password-off lane has exactly ONE handler — the sso row — so a
    // wrong URL there would ship silently without this pin.
    loginMethodsMock.mockResolvedValue({
      password: false,
      providers: [{ id: "erp", display_name: "Celestia ID", type: "builtin:erp", icon: null }],
    });
    const { wrapper } = await mountView();
    await flushPromises();
    const original = window.location.href;
    const hrefSetter = vi.fn();
    try {
      Object.defineProperty(window.location, "href", {
        set: (v: string) => hrefSetter(v),
        get: () => original,
        configurable: true,
      });
      await wrapper.find(".hk-auth-channel-row").trigger("click");
      expect(hrefSetter).toHaveBeenCalledWith("/api/v1/auth/oauth/erp");
    } finally {
      Object.defineProperty(window.location, "href", {
        value: original,
        writable: true,
        configurable: true,
      });
    }
  });

  it("falls back to the password lane when the descriptor is unreachable", async () => {
    // A rejected descriptor must not render a dead page: the classic
    // card is the fallback (the server still enforces the switch).
    loginMethodsMock.mockRejectedValue(new Error("boom"));
    const { wrapper } = await mountView();
    await flushPromises();
    expect(wrapper.findAll("input").length).toBeGreaterThan(0);
    expect(wrapper.find(".hk-auth-channel-row").exists()).toBe(false);
  });
});

describe("LoginView redirect handling", () => {
  it("honours the redirect query after a successful sign-in", async () => {
    loginMock.mockResolvedValue(session);
    const { wrapper, router } = await mountView({ redirect: "/files" });
    const inputs = wrapper.findAll("input");
    typeInto(inputs[0]!.element as HTMLInputElement, "admin");
    typeInto(inputs[1]!.element as HTMLInputElement, "secret");
    await flushPromises();
    (wrapper.find(".hk-btn-primary").element as HTMLElement).click();
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/files");
  });
});

describe("LoginView dead state (descriptor offers nothing)", () => {
  it("renders the guidance when no login method exists", async () => {
    loginMethodsMock.mockResolvedValue({ password: false, providers: [] });
    const { wrapper } = await mountView();
    await flushPromises();
    // Pin absence with selectors the CURRENT markup emits: the password
    // lane would mount a form field and the sso lane a provider row, so
    // either appearing here means the dead-state guard failed.
    expect(wrapper.findAll("input").length).toBe(0);
    expect(wrapper.find(".hk-auth-channel-row").exists()).toBe(false);
    expect(wrapper.text()).toContain("No login method is available");
  });
});

describe("LoginView password kill-switch (W4 closing)", () => {
  it("surfaces the disabled message on a 403, distinct from bad credentials", async () => {
    loginMethodsMock.mockResolvedValue({ password: true, providers: [] });
    loginMock.mockRejectedValue(new ApiError(403, "password login is disabled"));
    const { wrapper } = await mountView();
    const inputs = wrapper.findAll("input");
    typeInto(inputs[0]!.element as HTMLInputElement, "admin");
    typeInto(inputs[1]!.element as HTMLInputElement, "secret");
    await flushPromises();
    (wrapper.find(".hk-btn-primary").element as HTMLElement).click();
    await flushPromises();
    const alert = wrapper.find('[role="alert"]');
    expect(alert.exists()).toBe(true);
    // The kill-switch copy, NOT the 401 invalid-credentials copy.
    expect(alert.text()).toBe("Password login is disabled — use a provider below.");
    expect(alert.text()).not.toContain("Invalid username");
  });
});
