import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRouter, createMemoryHistory } from "vue-router";

// The API layer is mocked at module scope: these tests exercise the
// VIEW's contract (loading state, error surface, redirect), not the
// network. The mock's login reply carries the real AuthSession shape.
const loginMock = vi.fn();
vi.mock("@/api", () => ({ login: (...args: unknown[]) => loginMock(...args) }));
vi.mock("@/api/client", () => ({
  ApiError: class ApiError extends Error {
    status: number;
    constructor(status: number, message: string) {
      super(message);
      this.status = status;
    }
  },
  loadSession: () => null,
  saveSession: vi.fn(),
  onUnauthorized: vi.fn(),
}));

import LoginView from "./LoginView.vue";
import { ApiError } from "@/api/client";
import { createAppI18n } from "@/i18n";

const session = {
  token: "jwt-token",
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
  return { wrapper, router };
}

beforeEach(() => {
  loginMock.mockReset();
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
    // The store's session took hold (its persistence layer is the
    // client module, mocked here) and the router left /login.
    expect(router.currentRoute.value.path).toBe("/");
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
