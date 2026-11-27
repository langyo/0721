import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRouter, createMemoryHistory } from "vue-router";

// The API layer is mocked at module scope: these tests exercise the
// VIEW's contract (the account-first ladder, loading state, error
// surface, redirect), not the network. The mock's replies carry the real
// generated shapes (AuthSession / AccountMethods).
const loginMock = vi.fn();
const loginMethodsMock = vi.fn();
const accountMethodsMock = vi.fn();
vi.mock("@/api", () => ({
  login: (...args: unknown[]) => loginMock(...args),
  loginMethods: (...args: unknown[]) => loginMethodsMock(...args),
  accountMethods: (...args: unknown[]) => accountMethodsMock(...args),
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


import { HkEmptyState, HkLoginFlow, HkStatusBar, HkToast, useToast } from "@celestia-island/hikari";

import LoginView from "./LoginView";
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

/** The ladder's step-1 answer for an ordinary password account. */
const PASSWORD_FACTORS = { factors: [{ kind: "password" as const }], password: true };

function typeInto(input: HTMLInputElement, value: string) {
  input.value = value;
  dispatchInput(input);
}

function dispatchInput(input: HTMLInputElement) {
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

/** Every mounted toast outlet — cleanup owns their lifetime. */
const outlets: Array<{ unmount: () => void }> = [];

/** Every message currently on the toast surface. */
function toastTexts(): string[] {
  return [...document.querySelectorAll(".hk-toast-message")].map((el) => el.textContent ?? "");
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
  // The family toast outlet renders the toasts this view raises; the
  // inline error banner is GONE (the user's hard rule), so the assertions
  // below read the toast surface, not a role=alert paragraph.
  outlets.push(mount(HkToast, { attachTo: document.body, props: { teleport: false } }));
  // The login-methods descriptor resolves async — the card mounts after.
  await flushPromises();
  await flushPromises();
  return { wrapper, router };
}

/**
 * Drive step 1 (type the account name, submit) and land on step 2.
 *
 * Every ladder assertion starts here rather than reaching into the
 * component's state, so the assertion exercises the same path the user
 * does. `accountMethodsMock` must already be primed with the answer.
 */
async function walkToVerify(wrapper: Awaited<ReturnType<typeof mountView>>["wrapper"], name = "admin") {
  const inputs = wrapper.findAll("input");
  expect(inputs.length, "step 1 collects exactly one field").toBe(1);
  typeInto(inputs[0]!.element as HTMLInputElement, name);
  await flushPromises();
  (wrapper.find(".hk-btn-primary").element as HTMLElement).click();
  await flushPromises();
  await flushPromises();
  return wrapper.getComponent(HkLoginFlow);
}

/** Type the password on the verify step and submit it. */
async function provePassword(wrapper: Awaited<ReturnType<typeof mountView>>["wrapper"], password: string) {
  const flow = wrapper.getComponent(HkLoginFlow);
  expect(flow.props("step"), "the card must be on the verify step").toBe("verify");
  const field = wrapper.find('input[name="mfa-verify-password"]');
  expect(field.exists(), "the verify step proves the password in place").toBe(true);
  typeInto(field.element as HTMLInputElement, password);
  await flushPromises();
  (wrapper.find(".hk-btn-primary").element as HTMLElement).click();
  await flushPromises();
}

/**
 * MUT:verify-rows-return — hikari 0.55.494 dropped the verify-step
 * alternative rows (`verifyMethods` / `verifyMethodsDivider`) and the
 * rows-under-the-card syntax (`methods` / `methodsDivider`), so the
 * verify step must render NO channel row at all. The DOM is the pin:
 * the props themselves no longer exist to read back (R2 mutation
 * evidence — a `props("verifyMethods")` read is a no-op on an
 * undeclared key), so a resurrected ROW lane is what reddens here.
 */
function expectVerifyStepRowless(wrapper: { findAll: (s: string) => unknown[] }) {
  expect(wrapper.findAll(".hk-auth-channel-row")).toHaveLength(0);
}

/** Capture window.location.href assignments (the OAuth navigations). */
function watchHref(): { setter: ReturnType<typeof vi.fn>; restore: () => void } {
  const original = window.location.href;
  const setter = vi.fn();
  Object.defineProperty(window.location, "href", {
    set: (v: string) => setter(v),
    get: () => original,
    configurable: true,
  });
  return {
    setter,
    restore: () =>
      Object.defineProperty(window.location, "href", {
        value: original,
        writable: true,
        configurable: true,
      }),
  };
}

beforeEach(() => {
  loginMock.mockReset();
  loginMethodsMock.mockReset();
  accountMethodsMock.mockReset();
  // The login-methods descriptor: the classic password-on shape.
  loginMethodsMock.mockResolvedValue({ password: true, providers: [] });
  // The ladder's step-1 answer: an ordinary password account.
  accountMethodsMock.mockResolvedValue(PASSWORD_FACTORS);
});

afterEach(() => {
  outlets.splice(0).forEach((o) => o.unmount());
  // The toast store is module-level (hikari state) — clear it so the
  // next case's outlet renders only ITS OWN messages.
  const toast = useToast();
  for (const slot of toast.toasts.slice()) toast.remove(slot.id);
  localStorage.clear();
});

describe("LoginView account-first ladder", () => {
  it("collects only the account name on step 1 and probes the account", async () => {
    const { wrapper } = await mountView();
    // MUT:password-field — the opening card must not ask for a password:
    // account-first means the credential is proven AFTER the factors are
    // known. Re-flipping `passwordField` on the channel reddens here.
    const inputs = wrapper.findAll("input");
    expect(inputs.length).toBe(1);
    const channels = wrapper.getComponent(HkLoginFlow).props("channels") as Array<{
      key: string;
      mode: string;
      passwordField?: boolean;
    }>;
    expect(channels[0]).toMatchObject({
      key: "account",
      mode: "credentials",
      passwordField: false,
    });

    const flow = await walkToVerify(wrapper);
    // The probe carried the typed name and NOTHING else.
    expect(accountMethodsMock).toHaveBeenCalledWith("admin");
    // …and the card moved to the verify step.
    expect(flow.props("step")).toBe("verify");
    expect(flow.props("verifyAccountName")).toBe("admin");
    // MUT:verify-factors — the password factor is what the card proves;
    // an empty list renders a picker-less, proof-less card.
    expect(flow.props("verifyFactors")).toEqual(["password"]);
    // The verify step carries no alternative-row lane any more (the
    // rowless helper's DOM pin is the real gate — hikari 0.55.494 has
    // no verifyMethods prop to read back).
    expectVerifyStepRowless(wrapper);
  });

  it("proves the password on step 2 and redirects on success", async () => {
    loginMock.mockResolvedValue(session);
    const { wrapper, router } = await mountView();
    await walkToVerify(wrapper);
    await provePassword(wrapper, "secret");

    // The name the ladder was opened for is the one submitted — never a
    // re-typed or empty value.
    expect(loginMock).toHaveBeenCalledWith({ username: "admin", password: "secret" });
    // The router left /login…
    expect(router.currentRoute.value.path).toBe("/");
    // …and NOTHING session-shaped was persisted: the credential is the
    // httpOnly cookie (server-planted); the page holds no token.
    expect(localStorage.getItem("i0721-session")).toBeNull();
    expect(localStorage.getItem("i0721-token")).toBeNull();
    // (the locale preference is legitimate storage; session keys are not)
  });

  it("keeps the verify step rowless — a linked provider is a step-1 segment", async () => {
    // The state the endpoint exists for: an OAuth-only account. Its in-
    // page ladder has NO proof to offer, and hikari 0.55.494 gives it no
    // provider row either: the visitor goes BACK to step 1 and takes the
    // provider's own segment there.
    loginMethodsMock.mockResolvedValue({
      password: true,
      providers: [{ id: "erp", display_name: "Celestia ID", type: "builtin:erp", icon: null }],
    });
    accountMethodsMock.mockResolvedValue({
      factors: [{ kind: "oauth", provider: "erp", label: "Celestia ID" }],
      password: true,
    });
    const { wrapper } = await mountView();
    const flow = await walkToVerify(wrapper, "sso-only");
    expect(flow.props("step")).toBe("verify");
    // MUT:oauth-only-ladder — an OAuth-only account must not be handed a
    // password field it cannot use.
    expect(flow.props("verifyFactors")).toEqual([]);

    // MUT:verify-rows-return — no alternative rows, no divider, nothing
    // row-shaped inside the verify card.
    expectVerifyStepRowless(wrapper);

    // The provider is reachable exactly where the design says: step 1's
    // segment group, as its own sso channel after the funnel.
    const channels = wrapper.getComponent(HkLoginFlow).props("channels") as Array<{
      key: string;
      mode: string;
    }>;
    expect(channels.map((c) => c.key)).toEqual(["account", "erp"]);
    expect(channels[1]).toMatchObject({ mode: "sso" });
  });

  it("keeps the password as the only step-2 proof when a provider is also linked", async () => {
    // "Pick one of the ways through" changed shape: the password is the
    // only in-page proof; the provider is a step-1 segment, not a row
    // beside the password field. Either path still finishes the login —
    // they just live on different steps now.
    accountMethodsMock.mockResolvedValue({
      factors: [
        { kind: "password" },
        { kind: "oauth", provider: "erp", label: "Celestia ID" },
      ],
      password: true,
    });
    loginMethodsMock.mockResolvedValue({
      password: true,
      providers: [{ id: "erp", display_name: "Celestia ID", type: "builtin:erp", icon: null }],
    });
    const { wrapper } = await mountView();
    const flow = await walkToVerify(wrapper);
    expect(flow.props("verifyFactors")).toEqual(["password"]);
    // The card proves the password and carries nothing else: no row, no
    // divider, no "or continue with" strip.
    expect(wrapper.find('input[name="mfa-verify-password"]').exists()).toBe(true);
    expectVerifyStepRowless(wrapper);
  });

  it("keeps the verify step rowless for a binding the deployment no longer offers", async () => {
    // The binding is real but the provider is gone from the descriptor:
    // the old code rendered a disabled row here; the new shape renders
    // NOTHING — the flow entry would refuse anyway, and a row that says
    // "unavailable" has no step-2 home any more.
    accountMethodsMock.mockResolvedValue({
      factors: [{ kind: "oauth", provider: "gone", label: "gone" }],
      password: true,
    });
    const { wrapper } = await mountView();
    const flow = await walkToVerify(wrapper, "sso-only");
    // A stale binding is no proof: the card has nothing to offer in place
    // (and, unlike the old shape, no disabled row to show for it either).
    expect(flow.props("verifyFactors")).toEqual([]);
    expectVerifyStepRowless(wrapper);
  });

  it("surfaces an unreachable probe and stays on step 1", async () => {
    accountMethodsMock.mockRejectedValue(new Error("network unreachable"));
    const { wrapper } = await mountView();
    const inputs = wrapper.findAll("input");
    typeInto(inputs[0]!.element as HTMLInputElement, "admin");
    await flushPromises();
    (wrapper.find(".hk-btn-primary").element as HTMLElement).click();
    await flushPromises();
    await vi.waitFor(() => {
      expect(document.querySelector(".hk-toast-message")?.textContent).toBe("network unreachable");
    });
    // Still on step 1 — the ladder never opens without an answer.
    expect(wrapper.getComponent(HkLoginFlow).props("step")).toBe("account");
    expect(wrapper.find('input[name="mfa-verify-password"]').exists()).toBe(false);
    // The typed name survives in the field (a retry costs nothing).
    expect((wrapper.findAll("input")[0]!.element as HTMLInputElement).value).toBe("admin");
  });

  it("returns to the name field with it still filled when the verify step goes back", async () => {
    const { wrapper } = await mountView();
    const flow = await walkToVerify(wrapper);
    flow.vm.$emit("verify-back");
    await flushPromises();
    expect(flow.props("step")).toBe("account");
    const inputs = wrapper.findAll("input");
    expect(inputs.length).toBe(1);
    expect((inputs[0]!.element as HTMLInputElement).value).toBe("admin");
  });

  it("surfaces the localized invalid-credentials error on a 401", async () => {
    loginMock.mockRejectedValue(new ApiError(401, "invalid credentials"));
    const { wrapper } = await mountView();
    await walkToVerify(wrapper);
    await provePassword(wrapper, "wrong");
    const toastText = await vi.waitFor(() => {
      const el = document.querySelector(".hk-toast-message");
      expect(el).toBeTruthy();
      return el!.textContent ?? "";
    });
    // en baseline wording (the test i18n detects "en").
    expect(toastText).toBe("Invalid username or password");
    // The failure is NOT a paragraph beside the card.
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
  });

  it("keeps the raw message for non-401 login failures", async () => {
    loginMock.mockRejectedValue(new Error("network unreachable"));
    const { wrapper } = await mountView();
    await walkToVerify(wrapper);
    await provePassword(wrapper, "pw");
    await vi.waitFor(() => {
      expect(document.querySelector(".hk-toast-message")?.textContent).toBe("network unreachable");
    });
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
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
    // Family rule: the bar carries `<semver> <branch>:<hash7>` when the build
    // had git metadata, and the bare semver otherwise.
    const identity = __APP_BUILD_IDENTITY__;
    expect(bar.props("version")).toBe(
      identity ? `${__APP_VERSION__} ${identity}` : __APP_VERSION__,
    );
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
    await walkToVerify(wrapper);
    await provePassword(wrapper, "secret");
    const toastText = await vi.waitFor(() => {
      const el = document.querySelector(".hk-toast-message");
      expect(el).toBeTruthy();
      return el!.textContent ?? "";
    });
    expect(toastText).toBe("Something went wrong");
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
  });

  it("carries a bare-string rejection", async () => {
    loginMock.mockRejectedValue("rpc timeout");
    const { wrapper } = await mountView();
    await walkToVerify(wrapper);
    await provePassword(wrapper, "secret");
    await vi.waitFor(() => {
      expect(document.querySelector(".hk-toast-message")?.textContent).toBe("rpc timeout");
    });
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
  });
});

describe("LoginView channel segment group (family login panel)", () => {
  it("makes Celestia ID a peer channel and every other provider its own segment", async () => {
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
    // sign-in); the account funnel is the first lane; and with the rows
    // gone the generic provider is NOT a row under the card any more but
    // its own segment. Order: funnel, then the descriptor's providers,
    // peer appended last.
    const channels = wrapper.getComponent(HkLoginFlow).props("channels") as Array<{
      key: string;
      mode: string;
    }>;
    expect(channels).toHaveLength(3);
    expect(channels[0]).toMatchObject({ key: "account", mode: "credentials" });
    expect(channels[1]).toMatchObject({ key: "gh", mode: "sso" });
    expect(channels[2]).toMatchObject({ key: "erp", mode: "sso" });
    expect("methods" in (channels[0] as object)).toBe(false);

    // Both segments navigate the whole page to their redirect entries.
    const href = watchHref();
    try {
      const flow = wrapper.getComponent(HkLoginFlow);
      flow.vm.$emit("sso-click", "gh");
      await flushPromises();
      expect(href.setter).toHaveBeenCalledWith("/api/v1/auth/oauth/gh");
      flow.vm.$emit("sso-click", "erp");
      await flushPromises();
      expect(href.setter).toHaveBeenCalledWith("/api/v1/auth/oauth/erp");
    } finally {
      href.restore();
    }
  });

  it("gives the provider its own segment when the kill-switch is off", async () => {
    loginMethodsMock.mockResolvedValue({
      password: false,
      providers: [{ id: "gh", display_name: "GitHub", type: "builtin:github", icon: null }],
    });
    const { wrapper } = await mountView();
    await flushPromises();
    // The account funnel still opens (it is the ladder), and the provider
    // is its own segment — there is no rows-under-the-card lane on either
    // switch state.
    const channels = wrapper.getComponent(HkLoginFlow).props("channels") as Array<{
      key: string;
      mode: string;
    }>;
    expect(channels).toHaveLength(2);
    expect(channels[0]).toMatchObject({ key: "account", mode: "credentials" });
    expect(channels[1]).toMatchObject({ key: "gh", mode: "sso" });
    expect("methods" in (channels[0] as object)).toBe(false);
    // One field, no password: the funnel is the same on both switch states.
    expect(wrapper.findAll("input").length).toBe(1);
    expect(wrapper.findAll('input[type="password"]').length).toBe(0);
  });

  it("hides the password form when the kill-switch is off and the segments still switch", async () => {
    loginMethodsMock.mockResolvedValue({
      password: false,
      providers: [{ id: "erp", display_name: "Celestia ID", type: "builtin:erp", icon: null }],
    });
    const { wrapper } = await mountView();
    await flushPromises();
    // No password input exists anywhere: the account card asks for the
    // name only, and the password lane is off.
    expect(wrapper.findAll('input[type="password"]').length).toBe(0);
    const tabs = wrapper.findAll(".hk-tabs-trigger");
    expect(tabs.map((tab) => tab.text().trim())).toEqual(["Account", "Celestia ID"]);
    // Taking the peer segment switches the flow's channel…
    await tabs[1]!.trigger("click");
    await flushPromises();
    expect(wrapper.getComponent(HkLoginFlow).props("modelValue")).toBe("erp");
    // …and back on the account funnel the ladder proves the password it
    // announced (the server refuses it — that is the 403 copy pinned
    // below).
    await wrapper.findAll(".hk-tabs-trigger")[0]!.trigger("click");
    await flushPromises();
    accountMethodsMock.mockResolvedValue({ factors: [{ kind: "password" }], password: false });
    const flow = await walkToVerify(wrapper);
    expect(flow.props("verifyFactors")).toEqual(["password"]);
    expect(wrapper.findAll('input[name="mfa-verify-password"]').length).toBe(1);
  });

  it("renders no channel segment group when the deployment offers exactly one lane", async () => {
    // The standing user complaint: a deployment configured for one
    // sign-in method must not show a segmented switch at all — a lone
    // segment reads as a broken control. With the password off and a
    // single erp peer, the account funnel plus the peer are still two
    // lanes; the lone-lane shape is a password-only deployment.
    loginMethodsMock.mockResolvedValue({ password: true, providers: [] });
    const { wrapper } = await mountView();
    await flushPromises();
    expect(wrapper.findAll(".hk-tabs-trigger").length).toBe(0);
    expect(wrapper.getComponent(HkLoginFlow).props("channels")).toHaveLength(1);

    // And the seam stays exactly where hikari draws it: a SECOND lane
    // brings the segment group back.
    wrapper.unmount();
    loginMethodsMock.mockResolvedValue({
      password: false,
      providers: [{ id: "gh", display_name: "GitHub", type: "builtin:github", icon: null }],
    });
    const again = await mountView();
    await flushPromises();
    expect(again.wrapper.findAll(".hk-tabs-trigger").map((tab) => tab.text().trim())).toEqual([
      "Account",
      "Single sign-on",
    ]);
  });

  it("labels the segments with the PANEL's copy, never the server's provider name", async () => {
    // hikari's locked copy rule: a server-provided provider name in the
    // segment read as "log in to ERP" instead of "log in with your
    // organization". The tab comes from our closed TYPE table; the
    // provider's own name rides the channel's button.
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
    // MUT:channel-rename — the account lane is named by the localized
    // `common.auth.accountChannel` key ("Account" in en), not by the
    // retired password wording.
    expect(labels).toEqual(["Account", "Celestia ID"]);
    expect(labels.join(" ")).not.toContain("Acme Corp Internal");

    // The sso card's own button is where the provider name belongs.
    const channels = wrapper.getComponent(HkLoginFlow).props("channels") as Array<{
      mode: string;
      buttonLabel?: string;
    }>;
    expect(channels[1]).toMatchObject({ mode: "sso", buttonLabel: "Acme Corp Internal" });
  });

  it("never renders a raw locale key when a provider type is unknown", async () => {
    // Contract-reachable: the Rust enum is closed at four types today, but
    // adding a fifth must not put a literal key on a tab.
    loginMethodsMock.mockResolvedValue({
      password: false,
      providers: [{ id: "x", display_name: "Mystery IdP", type: "builtin:mystery", icon: null }],
    });
    const { wrapper } = await mountView();
    await flushPromises();

    const labels = wrapper.findAll(".hk-tabs-trigger").map((tab) => tab.text().trim());
    // Two lanes (the funnel + the unknown provider's own lane); no tab
    // may show a raw "common.auth.*" key — the destination's name belongs
    // on the sso button, never on a tab. comment-gate-ok: the assertions below are that claim
    expect(labels.join(" ")).not.toContain("common.auth.");
    const channels = wrapper.getComponent(HkLoginFlow).props("channels") as Array<{
      label?: string;
      buttonLabel?: string;
    }>;
    expect(channels).toHaveLength(2);
    expect(channels[0]!.label).toBe("Account"); // panel copy, not a key
    expect(channels[1]).toMatchObject({ label: "Single sign-on", buttonLabel: "Mystery IdP" });
  });

  it("numbers repeated tabs and pins the exact panel copy", async () => {
    // Two OIDC providers are a real deployment (multi-IdP). The sso
    // buttons name the destinations; the TABS must be tellable apart, and
    // the family's copy rule forbids borrowing the server's name for
    // that — so the panel numbers them. The expected strings are spelled
    // out: a mangled ordinal (missing separator, wrong base, skipped
    // index) is a copy regression, not a stylistic choice.
    loginMethodsMock.mockResolvedValue({
      password: false,
      providers: [
        { id: "o1", display_name: "Keycloak", type: "builtin:oidc", icon: null },
        { id: "o2", display_name: "Auth0", type: "builtin:oidc", icon: null },
      ],
    });
    const { wrapper } = await mountView();
    await flushPromises();

    const labels = wrapper.findAll(".hk-tabs-trigger").map((tab) => tab.text().trim());
    // MUT:ordinal-copy — any change to these three strings reddens here.
    expect(labels).toEqual(["Account", "Single sign-on · 1", "Single sign-on · 2"]);
    // Still panel copy: neither server name leaks into a tab…
    expect(labels.join(" ")).not.toContain("Keycloak");
    expect(labels.join(" ")).not.toContain("Auth0");
    // …and both destinations stay reachable, each named on its own sso
    // button.
    const channels = wrapper.getComponent(HkLoginFlow).props("channels") as Array<{
      key: string;
      buttonLabel?: string;
    }>;
    expect(channels[1]).toMatchObject({ key: "o1", buttonLabel: "Keycloak" });
    expect(channels[2]).toMatchObject({ key: "o2", buttonLabel: "Auth0" });
  });

  it("treats a duplicated peer type as ordinary lanes, not one winner", async () => {
    // ProvidersView defaults a new provider to builtin:erp, and the server
    // accepts any number of them — so two erp providers are reachable. The
    // peer lane is singular by contract: neither may hijack the
    // "Celestia ID" tab while the other is demoted to generic copy.
    loginMethodsMock.mockResolvedValue({
      password: false,
      providers: [
        { id: "e1", display_name: "Celestia ID", type: "builtin:erp", icon: null },
        { id: "e2", display_name: "Acme ERP", type: "builtin:erp", icon: null },
      ],
    });
    const { wrapper } = await mountView();
    await flushPromises();

    const labels = wrapper.findAll(".hk-tabs-trigger").map((tab) => tab.text().trim());
    expect(labels).toEqual(["Account", "Single sign-on · 1", "Single sign-on · 2"]);
    expect(labels).not.toContain("Celestia ID");

    // Both destinations stay reachable, each named on its own sso button.
    const channels = wrapper.getComponent(HkLoginFlow).props("channels") as Array<{
      key: string;
      buttonLabel?: string;
    }>;
    expect(channels[1]).toMatchObject({ key: "e1", buttonLabel: "Celestia ID" });
    expect(channels[2]).toMatchObject({ key: "e2", buttonLabel: "Acme ERP" });
  });

  it("navigates when a provider segment is taken", async () => {
    // The segment group is the only provider surface: taking a provider's
    // tab switches the flow to that channel, and the channel's sso event
    // carries the full-page redirect. Without a working sso path the
    // segments render but go nowhere — a regression this branch shipped
    // once. The assertion is at the emit seam, so neutering the handler
    // body reddens it (a tab-level assertion stays green through that).
    loginMethodsMock.mockResolvedValue({
      password: false,
      providers: [{ id: "gh", display_name: "GitHub", type: "builtin:github", icon: null }],
    });
    const { wrapper } = await mountView();
    await flushPromises();

    // MUT:sso-handler — emptying onSsoClick's body leaves the segment
    // switchable but goes nowhere; this assertion catches it.
    const href = watchHref();
    try {
      await wrapper.findAll(".hk-tabs-trigger")[1]!.trigger("click");
      await flushPromises();
      expect(wrapper.getComponent(HkLoginFlow).props("modelValue")).toBe("gh");
      wrapper.getComponent(HkLoginFlow).vm.$emit("sso-click", "gh");
      await flushPromises();
      expect(href.setter).toHaveBeenCalledWith("/api/v1/auth/oauth/gh");
    } finally {
      href.restore();
    }
  });

  it("grows the segment group only once a second channel exists", async () => {
    // One lane alone stays a segment-less card (hikari's rule: a lone
    // dead segment reads as a broken switch); the account lane plus a
    // peer is two lanes and therefore a switchable group.
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

  it("declares the account channel as a name-only, account-first form", async () => {
    loginMethodsMock.mockResolvedValue({ password: true, providers: [] });
    const { wrapper } = await mountView();
    await flushPromises();
    // The channel OBJECT is the contract the panel renders from: flipping
    // passwordField back on would silently restore the classic two-field
    // card, and dropping submitLabel would leave a nameless button.
    const channels = wrapper.getComponent(HkLoginFlow).props("channels") as Array<{
      key: string;
      mode: string;
      passwordField?: boolean;
      submitLabel?: string;
      usernameType?: string;
    }>;
    expect(channels.length).toBe(1);
    expect(channels[0]!.mode).toBe("credentials");
    expect(channels[0]!.passwordField).toBe(false);
    expect(channels[0]!.submitLabel).toBeTruthy();
    expect(channels[0]!.usernameType).toBe("text");
    // The alternative-rows syntax is gone from hikari: the channel must
    // not declare a methods block (it would be dead weight at best).
    expect("methods" in (channels[0] as object)).toBe(false);
    expect("methodsDivider" in (channels[0] as object)).toBe(false);
  });

  it("keeps three lanes for funnel + generic sso + peer when the switch is off", async () => {
    loginMethodsMock.mockResolvedValue({
      password: false,
      providers: [
        { id: "erp", display_name: "Celestia ID", type: "builtin:erp", icon: null },
        { id: "gh", display_name: "GitHub", type: "builtin:github", icon: null },
      ],
    });
    const { wrapper } = await mountView();
    await flushPromises();
    // Three redirect targets, one segment each: the funnel, the generic
    // "Single sign-on" lane, and the peer lane appended last. Each
    // provider's own name stays on its sso BUTTON, never on a tab. comment-gate-ok: the tab-label assertion below is that claim
    const tabs = wrapper.findAll(".hk-tabs-trigger");
    expect(tabs.map((tab) => tab.text().trim())).toEqual([
      "Account",
      "Single sign-on",
      "Celestia ID",
    ]);
    // No row lives under the account card on this switch state either.
    expect(wrapper.findAll(".hk-auth-channel-row").length).toBe(0);
    // Taking the peer segment switches to it…
    const peerTab = tabs.find((tab) => tab.text().trim() === "Celestia ID")!;
    await peerTab.trigger("click");
    await flushPromises();
    expect(wrapper.getComponent(HkLoginFlow).props("modelValue")).toBe("erp");
    // …and both segments navigate to their own redirect entries.
    const href = watchHref();
    try {
      const flow = wrapper.getComponent(HkLoginFlow);
      flow.vm.$emit("sso-click", "erp");
      await flushPromises();
      expect(href.setter).toHaveBeenCalledWith("/api/v1/auth/oauth/erp");
      flow.vm.$emit("sso-click", "gh");
      await flushPromises();
      expect(href.setter).toHaveBeenCalledWith("/api/v1/auth/oauth/gh");
    } finally {
      href.restore();
    }
  });
});

describe("LoginView sso lane (family panel)", () => {
  it("navigates the whole page from the peer segment when the password lane is off", async () => {
    // The peer segment is the only other handler besides the funnel, so a
    // wrong URL there would ship silently without this pin.
    loginMethodsMock.mockResolvedValue({
      password: false,
      providers: [{ id: "erp", display_name: "Celestia ID", type: "builtin:erp", icon: null }],
    });
    const { wrapper } = await mountView();
    await flushPromises();
    await wrapper.findAll(".hk-tabs-trigger")[1]!.trigger("click");
    await flushPromises();
    expect(wrapper.getComponent(HkLoginFlow).props("modelValue")).toBe("erp");
    const href = watchHref();
    try {
      wrapper.getComponent(HkLoginFlow).vm.$emit("sso-click", "erp");
      await flushPromises();
      expect(href.setter).toHaveBeenCalledWith("/api/v1/auth/oauth/erp");
    } finally {
      href.restore();
    }
  });

  it("falls back to the account funnel when the descriptor is unreachable", async () => {
    // A rejected descriptor must not render a dead page: the local default
    // (password ON, no providers) gives a usable ladder, and the server
    // still enforces its own switch.
    loginMethodsMock.mockRejectedValue(new Error("boom"));
    const { wrapper } = await mountView();
    await flushPromises();
    expect(wrapper.findAll("input").length).toBe(1);
    expect(wrapper.find(".hk-auth-channel-row").exists()).toBe(false);
    expect(wrapper.getComponent(HkLoginFlow).props("channels")).toHaveLength(1);
  });
});

describe("LoginView redirect handling", () => {
  it("honours the redirect query after a successful sign-in", async () => {
    loginMock.mockResolvedValue(session);
    const { wrapper, router } = await mountView({ redirect: "/files" });
    await walkToVerify(wrapper);
    await provePassword(wrapper, "secret");
    expect(router.currentRoute.value.path).toBe("/files");
  });
});

describe("LoginView dead state (descriptor offers nothing)", () => {
  it("still opens the account funnel rather than a dead page", async () => {
    // The funnel is unconditional — it is what discovers whether the name
    // has a password at all — so "no password and no provider" is no
    // longer a dead end. The guidance copy stays reachable through
    // HkEmptyState's own path (no channels at all), which the guard below
    // still proves cannot be rendered accidentally.
    loginMethodsMock.mockResolvedValue({ password: false, providers: [] });
    const { wrapper } = await mountView();
    await flushPromises();
    const flow = wrapper.findComponent(HkLoginFlow);
    expect(flow.exists()).toBe(true);
    expect(flow.props("channels")).toHaveLength(1);
    // Exactly one field (the name) and no provider row: the funnel cannot
    // turn into the classic two-field card or an SSO-only page here.
    expect(wrapper.findAll("input").length).toBe(1);
    expect(wrapper.findAll('input[type="password"]').length).toBe(0);
    expect(wrapper.find(".hk-auth-channel-row").exists()).toBe(false);
  });
});

describe("LoginView password kill-switch (W4 closing)", () => {
  it("surfaces the disabled message on a 403, distinct from bad credentials", async () => {
    loginMethodsMock.mockResolvedValue({ password: true, providers: [] });
    loginMock.mockRejectedValue(new ApiError(403, "password login is disabled"));
    const { wrapper } = await mountView();
    await walkToVerify(wrapper);
    await provePassword(wrapper, "secret");
    const toastText = await vi.waitFor(() => {
      const el = document.querySelector(".hk-toast-message");
      expect(el).toBeTruthy();
      return el!.textContent ?? "";
    });
    // The kill-switch copy, NOT the 401 invalid-credentials copy. The
    // wording names a channel, not a row "below" the card — providers are
    // peers in the segment group now, so the old phrasing pointed at
    // nothing (MUT:disabled-copy — restoring it reddens here).
    expect(toastText).toBe("Password login is disabled — pick another sign-in channel.");
    expect(toastTexts().join("\n")).not.toContain("Invalid username");
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
  });

  it("refuses to prove a password when the ladder never named an account", async () => {
    // Defensive seat: `onVerify` submits `accountName`, so an empty value
    // must refuse rather than POST an empty user name. Reached here by
    // emitting the verify event before any probe has answered.
    const { wrapper } = await mountView();
    wrapper.getComponent(HkLoginFlow).vm.$emit("verify", "password", "secret");
    await flushPromises();
    expect(loginMock).not.toHaveBeenCalled();
  });
});

describe("LoginView federated mode (password:false)", () => {
  /** A policy payload with the password lane switched off. */
  function federated(providers: Array<{ id: string; display_name: string; type: string; icon: string | null }>) {
    return {
      password: false,
      providers,
      policy: { version: 1, mode: "federated", password: false, methods: [] },
    };
  }

  it("renders no account channel and seeds the first sso segment", async () => {
    // MUT:federated-funnel-gone — with `policy.password === false` the
    // account funnel must not render at all: the server 403s its password
    // arm, so a tab whose only exit is a 403 is a dead switch (this is
    // the res.celestia.world shape). The policy's own flag is what the
    // legacy top-level `password` boolean is NOT consulted for here —
    // dropping the `policy` read or flipping the fail-open default
    // reddens this case.
    loginMethodsMock.mockResolvedValue(
      federated([
        { id: "gh", display_name: "GitHub", type: "builtin:github", icon: null },
        { id: "erp", display_name: "erp", type: "builtin:erp", icon: null },
      ]),
    );
    const { wrapper } = await mountView();
    await flushPromises();

    const flow = wrapper.getComponent(HkLoginFlow);
    const channels = flow.props("channels") as Array<{
      key: string;
      mode: string;
      passwordField?: boolean;
    }>;
    expect(channels.map((c) => c.key)).toEqual(["gh", "erp"]);
    expect(channels.some((c) => c.mode === "credentials")).toBe(false);
    // No name field anywhere: the funnel is not merely hidden, it is gone.
    expect(wrapper.findAll("input").length).toBe(0);
    // The seeded channel is the FIRST sso segment, not the account key —
    // a stale "account" seed would leave the flow on a channel that
    // no longer exists.
    expect(flow.props("modelValue")).toBe("gh");
    // The peer segment still lands last.
    expect(channels[1]).toMatchObject({ key: "erp", mode: "sso" });
  });

  it("uses the built-in name and mark when the peer's display_name degenerated to its id", async () => {
    // The res.celestia.world form: the operator never named the provider,
    // so `renderable_rows` either dropped the empty name or the operator
    // set it to the literal id — both read as "unnamed" through
    // `hkProviderLabelIsId`, and the family's built-in name fills in.
    loginMethodsMock.mockResolvedValue(
      federated([{ id: "erp", display_name: "erp", type: "builtin:erp", icon: null }]),
    );
    const { wrapper } = await mountView();
    await flushPromises();

    // MUT:central-auth-name — the built-in name belongs to the central
    // identity peer ONLY (en baseline wording pinned verbatim); a generic
    // provider degenerating to its id must NEVER take it.
    const channels = wrapper.getComponent(HkLoginFlow).props("channels") as Array<{
      key: string;
      buttonLabel?: string;
    }>;
    expect(channels[0]!.buttonLabel).toBe("Celestia Unified Authentication Platform");

    // MUT:central-auth-mark — the peer's icon is the built-in 16×16 data
    // URI <img>, not the neutral key glyph; a KeyRound resurrection or a
    // dropped width/height reddens here.
    const peerIconNode = (
      wrapper.getComponent(HkLoginFlow).props("channels") as Array<{
        icon?: { props?: { src?: string; width?: number; height?: number } };
      }>
    )[0]!.icon;
    expect(peerIconNode?.props?.src).toMatch(/^data:image\/webp;base64,/);
    expect(peerIconNode?.props?.width).toBe(16);
    expect(peerIconNode?.props?.height).toBe(16);
  });

  it("keeps an operator-set peer display_name verbatim", async () => {
    // Precedence rule 1: nothing an operator configures is overridden —
    // a portal named "门户" keeps its own name on the button (and the
    // built-in name must not displace it).
    loginMethodsMock.mockResolvedValue(
      federated([{ id: "erp", display_name: "门户", type: "builtin:erp", icon: null }]),
    );
    const { wrapper } = await mountView();
    await flushPromises();

    const channels = wrapper.getComponent(HkLoginFlow).props("channels") as Array<{
      buttonLabel?: string;
      icon?: unknown;
    }>;
    expect(channels[0]!.buttonLabel).toBe("门户");
    // The icon rule still applies regardless of the name: no operator
    // icon was configured, so the built-in mark fills the gap.
    const icon = (channels[0]!.icon as { props?: { src?: string } }) ?? undefined;
    expect(icon?.props?.src).toMatch(/^data:image\/webp;base64,/);
  });

  it("falls to the honest dead state when the password lane is off and nothing else is offered", async () => {
    // A federated deployment whose providers are all gone has literally
    // no entrance left: the empty funnel no longer papers over it, and
    // the HkEmptyState dead state is the honest render.
    loginMethodsMock.mockResolvedValue(federated([]));
    const { wrapper } = await mountView();
    await flushPromises();

    expect(wrapper.findComponent(HkLoginFlow).exists()).toBe(false);
    expect(wrapper.findComponent(HkEmptyState).exists()).toBe(true);
    expect(wrapper.text()).toContain(
      "No login method is available — an administrator must enable a provider or password login.",
    );
  });

  it("keeps the master shape when the password lane is on (regression)", async () => {
    // MUT:funnel-default — the fail-open default and the password-on path
    // must stay byte-identical to master: funnel first, peer last, and
    // the seeded channel is the account key. A `password:false` refactor
    // that leaked into the funnel-on shape reddens here.
    loginMethodsMock.mockResolvedValue({
      password: true,
      providers: [{ id: "erp", display_name: "Acme ERP", type: "builtin:erp", icon: null }],
      policy: { version: 1, mode: "local", password: true, methods: [] },
    });
    const { wrapper } = await mountView();
    await flushPromises();

    const flow = wrapper.getComponent(HkLoginFlow);
    const channels = flow.props("channels") as Array<{ key: string; mode: string; buttonLabel?: string; icon?: unknown }>;
    expect(channels.map((c) => c.key)).toEqual(["account", "erp"]);
    expect(channels[0]).toMatchObject({ mode: "credentials", passwordField: false });
    expect(flow.props("modelValue")).toBe("account");
    // A named operator peer keeps its name, and the peer still carries the
    // built-in mark: the mark rides the PEER lane on both switch states,
    // it is not a password-off-only swap.
    expect(channels[1]!.buttonLabel).toBe("Acme ERP");
    expect(
      ((channels[1]!.icon as { props?: { src?: string } }).props?.src ?? "").startsWith(
        "data:image/webp;base64,",
      ),
    ).toBe(true);
  });
});
