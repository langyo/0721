import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia, type Pinia } from "pinia";
import { createMemoryHistory, createRouter, type Router } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The API layer is the environment here; the roster's SHAPE is the
// subject (the family table anatomy replaced the hand-built list).
const listUsersMock = vi.fn();
const deleteUserMock = vi.fn();
vi.mock("@/api", () => ({
  listUsers: (...args: unknown[]) => listUsersMock(...args),
  deleteUser: (...args: unknown[]) => deleteUserMock(...args),
  createUser: vi.fn(),
}));

import UsersView from "./UsersView.vue";
import { provideActionBar } from "@/composables/useActionBar";
import { createAppI18n } from "@/i18n";
import { useAuthStore } from "@/stores/auth";
import type { UserDto } from "@/types";

/**
 * The roster's family shape: rows render through the table scaffold with
 * the account/role/created columns, the roster reports the server's total,
 * and the signed-in admin gets no delete affordance on their own row
 * (deleting yourself would drop the admin surface mid-session).
 */

const i18n = createAppI18n();

function user(id: string, username: string, role: "admin" | "member"): UserDto {
  return {
    id,
    username,
    display_name: username.toUpperCase(),
    role,
    created_at: "2026-11-01T10:00:00Z",
  };
}

function mountView(): ReturnType<typeof mount> {
  const pinia: Pinia = createPinia();
  setActivePinia(pinia);
  const auth = useAuthStore();
  auth.$patch({ user: user("me", "me", "admin") });
  const router: Router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: "/", component: { template: "<div/>" } }],
  });
  const Harness = {
    template: "<div><div class='probe'/><UsersView /></div>",
    components: { UsersView },
    setup() {
      provideActionBar();
    },
  };
  return mount(Harness, { global: { plugins: [pinia, router, i18n] } });
}

let wrapper: ReturnType<typeof mount> | null = null;

beforeEach(() => {
  wrapper = null;
  listUsersMock.mockReset();
  deleteUserMock.mockReset();
});

afterEach(() => {
  wrapper?.unmount();
});

describe("UsersView (family table roster)", () => {
  it("renders one row per user with the account and role cells", async () => {
    listUsersMock.mockResolvedValue({
      items: [user("me", "me", "admin"), user("u2", "alice", "member")],
      total: 2,
    });
    wrapper = mountView();
    await flushPromises();
    const rows = wrapper.findAll("tbody tr");
    expect(rows.length).toBe(2);
    expect(wrapper.text()).toContain("ALICE");
    expect(wrapper.text()).toContain("@alice");
    // The server's total is the readout, not the page length.
    expect(wrapper.text()).toContain(String(2));
  });

  it("offers no delete affordance on the signed-in admin's own row", async () => {
    listUsersMock.mockResolvedValue({
      items: [user("me", "me", "admin"), user("u2", "alice", "member")],
      total: 2,
    });
    wrapper = mountView();
    await flushPromises();
    const deleteButtons = wrapper.findAll(`button[aria-label="${i18n.global.t("common.actions.delete")}"]`);
    // Two rows, one delete button: the self row carries none.
    expect(deleteButtons.length).toBe(1);
  });

  it("renders the scaffold's empty state when the roster is empty", async () => {
    listUsersMock.mockResolvedValue({ items: [], total: 0 });
    wrapper = mountView();
    await flushPromises();
    expect(wrapper.findAll("tbody tr").length).toBe(0);
    expect(wrapper.text()).toContain(i18n.global.t("common.users.empty"));
  });
});