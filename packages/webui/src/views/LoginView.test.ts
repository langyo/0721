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

async function mountView() {
  const i18n = createAppI18n();
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/", name: "portal", component: { template: "<div/>" } },
      { path: "/login", name: "login", component: LoginView },
    ],
  });
  await router.push("/login");
  const wrapper = mount(LoginView, {
    global: { plugins: [i18n, router, createPinia()] },
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

describe("LoginView dual-mode (W4)", () => {
  it("renders provider buttons that navigate to the OAuth entry", async () => {
    loginMethodsMock.mockResolvedValue({
      password: true,
      providers: [
        { id: "erp", display_name: "ERP", type: "builtin:erp" },
        { id: "gh", display_name: "GitHub", type: "builtin:github" },
      ],
    });
    const { wrapper } = await mountView();
    const buttons = wrapper.findAll(".provider-button");
    expect(buttons.length).toBe(2);
    expect(buttons[0]!.text()).toContain("ERP");

    // Navigation is a FULL page load (the flow sets an httpOnly cookie +
    // 302s) — pin the href, not a router push.
    const original = window.location.href;
    const hrefSetter = vi.fn();
    Object.defineProperty(window, "location", {
      value: { ...window.location, href: original },
      writable: false,
    });
    // jsdom-style href write throws on read-only; assert via click side
    // effect instead: patch then click.
    try {
      Object.defineProperty(window.location, "href", {
        set: (v: string) => hrefSetter(v),
        get: () => original,
        configurable: true,
      });
      await buttons[0]!.trigger("click");
      expect(hrefSetter).toHaveBeenCalledWith("/api/v1/auth/oauth/erp");
    } finally {
      // restore a plain href
      Object.defineProperty(window.location, "href", {
        value: original,
        writable: true,
        configurable: true,
      });
    }
  });

  it("hides the password form when the kill-switch is off", async () => {
    loginMethodsMock.mockResolvedValue({
      password: false,
      providers: [{ id: "erp", display_name: "ERP", type: "builtin:erp" }],
    });
    const { wrapper } = await mountView();
    await flushPromises();
    // No credential inputs — the providers-only page.
    expect(wrapper.find(".s-auth-card").exists()).toBe(false);
    expect(wrapper.findAll(".provider-button").length).toBe(1);
  });
});
