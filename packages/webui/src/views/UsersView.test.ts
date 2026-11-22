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
    // The Created column keeps the time-of-day (master rendered
    // toLocaleString; the date tag's `time` flag is what preserves it).
    expect(wrapper.text()).toMatch(/\d{1,2}:\d{2}/);
  });

  it("reports the SERVER's total, which is not the page length", async () => {
    // Distinct from the row count on purpose: a subtitle that rendered
    // the page length instead of `total` used to pass unnoticed because
    // both were 2.
    listUsersMock.mockResolvedValue({
      items: [user("me", "me", "admin"), user("u2", "alice", "member")],
      total: 42,
    });
    wrapper = mountView();
    await flushPromises();
    expect(wrapper.text()).toContain("42");
    expect(wrapper.findAll("tbody tr").length).toBe(2);
  });

  it("refuses a second load-more while a page is still in flight", async () => {
    // The paging offset is read from the live list, so two overlapping
    // calls would append the same page twice.
    listUsersMock.mockResolvedValueOnce({
      items: [user("me", "me", "dummy" as "member")],
      total: 3,
    });
    wrapper = mountView();
    await flushPromises();
    let release: (value: unknown) => void = () => undefined;
    const pending = new Promise((resolve) => {
      release = resolve;
    });
    listUsersMock.mockReturnValueOnce(pending);
    const more = wrapper.find(`button[aria-label="${i18n.global.t("common.actions.loadMore")}"]`);
    expect(more.exists()).toBe(true);
    await more.trigger("click");
    await more.trigger("click");
    // Two presses, one request: the guard refuses the second.
    expect(listUsersMock).toHaveBeenCalledTimes(2); // initial + one more
    release({ items: [], total: 3 });
    await flushPromises();
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