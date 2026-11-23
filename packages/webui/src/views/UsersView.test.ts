import { flushPromises, mount } from "@vue/test-utils";
import { h } from "vue";
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
    template: "<div><div class='probe'/></div>",
    components: { UsersView },
    setup() {
      const bar = provideActionBar();
      // Render the roster and the bar's registered entries (the refresh
      // button) so the failed-reload path is reachable from a test.
      return () => [h(UsersView), h("div", { class: "probe-actions" }, bar.actions.value.flatMap((r) => r()))];
    },
  };
  return mount(Harness, { global: { plugins: [pinia, router, i18n] }, attachTo: document.body });
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
    const created = wrapper.find(".hk-date-tag");
    expect(created.exists()).toBe(true);
    expect(created.text()).toMatch(/\d{1,2}:\d{2}/);
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
      items: [user("me", "me", "admin")],
      // Two more exist off-page, so `hasMore` opens the load-more control.
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
    // The invariant behind the guard: the row the user already had is
    // still there exactly once (a guard that "dodged" re-entrancy by
    // clearing the list would satisfy a call-count assertion alone).
    expect(wrapper.findAll("tbody tr").length).toBe(1);
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

  it("keeps the rows on a failed refresh and shows the stale banner", async () => {
    // The scaffold's `error` prop replaces the whole body; `staleError`
    // is the banner for exactly this case — a failed reload must never
    // hide data the user already had.
    listUsersMock.mockResolvedValueOnce({
      items: [user("me", "me", "admin"), user("u2", "alice", "member")],
      // More rows exist off-page, so the load-more control is on screen.
      total: 5,
    });
    wrapper = mountView();
    await flushPromises();
    expect(wrapper.findAll("tbody tr").length).toBe(2);

    // Any failed load while rows are on screen rides this branch — the
    // header refresh and the load-more control both call load().
    listUsersMock.mockRejectedValueOnce(new Error("load blew up"));
    const more = wrapper.find(`button[aria-label="${i18n.global.t("common.actions.loadMore")}"]`);
    expect(more.exists()).toBe(true);
    await more.trigger("click");
    await flushPromises();

    // Rows survive, and the failure is announced as a banner.
    expect(wrapper.findAll("tbody tr").length).toBe(2);
    // The scaffold's banner carries the failure message verbatim.
    expect(wrapper.text()).toContain("load blew up");
  });

  it("reports a non-Error rejection instead of a silent empty roster", async () => {
    // A JSON-RPC lane commonly rejects with a bare string; the fallback
    // must yield a real message, or a failed cold start reads as
    // "no users yet" with no alert at all.
    listUsersMock.mockRejectedValue(undefined);
    wrapper = mountView();
    await flushPromises();
    expect(wrapper.find(".hk-alert").exists()).toBe(true);
    expect(wrapper.text()).toContain(i18n.global.t("common.status.error"));
    wrapper!.unmount();

    // And the bare-string arm (a JSON-RPC lane's common shape) carries
    // the string itself.
    listUsersMock.mockRejectedValue("rpc timeout");
    wrapper = mountView();
    await flushPromises();
    expect(wrapper.find(".hk-alert").exists()).toBe(true);
    expect(wrapper.text()).toContain("rpc timeout");
  });

  it("renders the localized fallback for an Error with an empty message", async () => {
    // The lane is falsy-gated: an Error carrying an empty message must
    // fall through to the localized text, or the failure renders no
    // alert at all.
    listUsersMock.mockRejectedValue(new Error(""));
    wrapper = mountView();
    await flushPromises();
    expect(wrapper.find(".hk-alert").exists()).toBe(true);
    expect(wrapper.text()).toContain(i18n.global.t("common.status.error"));
  });

  it("surfaces a failed delete in the dialog instead of closing silently", async () => {
    listUsersMock.mockResolvedValue({
      items: [user("me", "me", "admin"), user("u2", "alice", "member")],
      total: 5,
    });
    deleteUserMock.mockRejectedValue("rpc timeout");
    wrapper = mountView();
    await flushPromises();
    const del = wrapper.find(`button[aria-label="${i18n.global.t("common.actions.delete")}"]`);
    await del.trigger("click");
    await flushPromises();
    // Confirm inside the dialog — the adaptive dialog teleports, so the
    // footer's danger action lives at the document level.
    const confirm = Array.from(document.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === i18n.global.t("common.actions.delete"),
    );
    expect(confirm).toBeTruthy();
    confirm!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    // The dialog (teleported) stays open and carries the rejection text.
    expect(document.body.textContent).toContain("rpc timeout");
    expect(wrapper.findAll("tbody tr").length).toBe(2);
  });

  it("renders the scaffold's empty state when the roster is empty", async () => {
    listUsersMock.mockResolvedValue({ items: [], total: 0 });
    wrapper = mountView();
    await flushPromises();
    expect(wrapper.findAll("tbody tr").length).toBe(0);
    expect(wrapper.text()).toContain(i18n.global.t("common.users.empty"));
  });
});