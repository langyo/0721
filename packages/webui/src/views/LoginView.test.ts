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
  it("makes Celestia ID a peer channel and keeps other providers as rows", async () => {
    loginMethodsMock.mockResolvedValue({
      password: true,
      providers: [
        { id: "erp", display_name: "Celestia ID", type: "builtin:erp", icon: null },
        { id: "gh", display_name: "GitHub", type: "builtin:github", icon: null },
      ],
    });
    const { wrapper } = await mountView();
    await flushPromises();

    // MUT:peer-lane — Celestia ID is the peer lane (the family's central
    // sign-in); every other provider is an alternative ROW under the
    // account card, which is where hikari puts provider names.
    const channels = wrapper.getComponent(HkLoginFlow).props("channels") as Array<{
      key: string;
      mode: string;
      methods?: Array<{ key: string; label: string }>;
    }>;
    expect(channels).toHaveLength(2);
    expect(channels[0]).toMatchObject({ key: "password", mode: "credentials" });
    expect(channels[1]).toMatchObject({ key: "erp", mode: "sso" });
    // GitHub rides the credentials card as a row, not as a segment.
    expect(channels[0]!.methods).toEqual([{ key: "gh", label: "GitHub", icon: expect.anything() }]);

    // The peer lane's row navigates the whole page to its redirect entry.
    const hrefSetter = vi.fn();
    Object.defineProperty(window, "location", {
      configurable: true,
      value: { ...window.location, set href(v: string) { hrefSetter(v); }, get href() { return "http://localhost:3000/"; } },
    });
    wrapper.getComponent(HkLoginFlow).vm.$emit("sso-click", "erp");
    await flushPromises();
    expect(hrefSetter).toHaveBeenCalledWith("/api/v1/auth/oauth/erp");
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

  it("labels the segments with the PANEL's copy, never the server's provider name", async () => {
    // hikari's locked copy rule: a server-provided provider name in the
    // segment read as "log in to ERP" instead of "log in with your
    // organization". The tab comes from our closed TYPE table; the
    // provider's own name rides the channel's row.
    loginMethodsMock.mockResolvedValue({
      password: true,
      providers: [
        // A deliberately hostile display_name: if it reaches a tab, the
        // assertion below names it.
        { id: "erp", display_name: "Acme Corp Internal", type: "builtin:erp", icon: null },
      ],
    });
    const { wrapper } = await mountView();
    await flushPromises();

    const tabs = wrapper.findAll(".hk-tabs-trigger");
    expect(tabs.length).toBe(2);
    const labels = tabs.map((tab) => tab.text().trim());
    expect(labels).toEqual(["Password", "Celestia ID"]);
    expect(labels.join(" ")).not.toContain("Acme Corp Internal");

    // The row inside the sso card is where the provider name belongs.
    const channels = wrapper.getComponent(HkLoginFlow).props("channels") as Array<{
      mode: string;
      buttonLabel?: string;
    }>;
    expect(channels[1]).toMatchObject({ mode: "sso", buttonLabel: "Acme Corp Internal" });
  });

  it("never renders a raw locale key when a provider type is unknown", async () => {
    // Contract-reachable: the Rust enum is closed at four types today, but
    // adding a fifth must not put a literal key on a tab. With no password
    // lane every provider is its own channel — including an unknown one.
    loginMethodsMock.mockResolvedValue({
      password: false,
      providers: [{ id: "x", display_name: "Mystery IdP", type: "builtin:mystery", icon: null }],
    });
    const { wrapper } = await mountView();
    await flushPromises();

    const labels = wrapper.findAll(".hk-tabs-trigger").map((tab) => tab.text().trim());
    // One channel alone → no segment group, but the card's row still names
    // the destination; nothing may show "common.auth.*".
    expect(labels.join(" ")).not.toContain("common.auth.");
    const channels = wrapper.getComponent(HkLoginFlow).props("channels") as Array<{
      label?: string;
      buttonLabel?: string;
    }>;
    expect(channels).toHaveLength(1);
    expect(channels[0]!.label).toBe("Single sign-on");   // panel copy, not a key
    expect(channels[0]!.buttonLabel).toBe("Mystery IdP"); // the row names it
  });

  it("grows the segment group only once a second channel exists", async () => {
    // One lane alone stays a segment-less card (hikari's rule: a lone
    // dead segment reads as a broken switch); the account lane plus a
    // provider is two peers and therefore a switchable group.
    loginMethodsMock.mockResolvedValue({ password: true, providers: [] });
    let ctx = await mountView();
    await flushPromises();
    expect(ctx.wrapper.getComponent(HkLoginFlow).props("channels")).toHaveLength(1);
    ctx.wrapper.unmount();

    loginMethodsMock.mockResolvedValue({
      password: true,
      providers: [{ id: "erp", display_name: "Celestia ID", type: "builtin:erp", icon: null }],
    });
    ctx = await mountView();
    await flushPromises();
    expect(ctx.wrapper.getComponent(HkLoginFlow).props("channels")).toHaveLength(2);
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
    // The kill-switch copy, NOT the 401 invalid-credentials copy. The
    // wording names a channel, not a row "below" the card — providers are
    // peers in the segment group now, so the old phrasing pointed at
    // nothing (MUT:disabled-copy — restoring it reddens here).
    expect(alert.text()).toBe("Password login is disabled — pick another sign-in channel.");
    expect(alert.text()).not.toContain("Invalid username");
  });
});
