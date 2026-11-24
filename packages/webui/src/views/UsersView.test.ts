import { flushPromises, mount } from "@vue/test-utils";
import { h } from "vue";
import { createPinia, setActivePinia, type Pinia } from "pinia";
import { createMemoryHistory, createRouter, type Router } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The API layer is the environment here; the roster's SHAPE is the
// subject (the family table anatomy replaced the hand-built list).
const listUsersMock = vi.fn();
const deleteUserMock = vi.fn();
const createUserMock = vi.fn();
vi.mock("@/api", () => ({
  listUsers: (...args: unknown[]) => listUsersMock(...args),
  deleteUser: (...args: unknown[]) => deleteUserMock(...args),
  createUser: (...args: unknown[]) => createUserMock(...args),
}));

import UsersView from "./UsersView";
import { HkToast, useToast } from "@celestia-island/hikari";
import { ApiError } from "@/api/client";
import { provideActionBar } from "@/composables/useActionBar";
import { createAppI18n } from "@/i18n";
import { useAuthStore } from "@/stores/auth";
import type { UserDto } from "@/types";

/**
 * The roster's family shape: rows render through the table scaffold with
 * the account/role/created columns, the roster reports the server's total,
 * and the signed-in admin gets no delete affordance on their own row
 * (deleting yourself would drop the admin surface mid-session).
 *
 * Operation RESULTS (create / delete failures) are TOASTS: the inline
 * `role="alert"` paragraphs inside the dialogs are gone (the user's hard
 * rule — no DOM inserted beside a control), so the assertions below read
 * the toast surface and pin the paragraphs' ABSENCE.
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

/** Every mounted toast outlet — cleanup owns their lifetime (the view's
 *  own wrapper is returned to the caller; the outlet is harness state). */
const outlets: Array<{ unmount: () => void }> = [];

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
      // Render the roster and the bar's registered entries (the create
      // plus + the refresh button) so both paths are reachable from a test.
      return () => [h(UsersView), h("div", { class: "probe-actions" }, bar.actions.value.flatMap((r) => r()))];
    },
  };
  // attachTo keeps the tree in the live document — the dialogs teleport to
  // the overlay root, and a detached mount hides them.
  const host = document.createElement("div");
  document.body.appendChild(host);
  const mounted = mount(Harness, { global: { plugins: [pinia, router, i18n] }, attachTo: host });
  // The family toast outlet renders the toasts this view raises.
  outlets.push(mount(HkToast, { attachTo: document.body, props: { teleport: false } }));
  return mounted;
}

/** The latest message on the toast surface (empty when nothing is shown). */
function toastText(): string {
  const nodes = document.querySelectorAll(".hk-toast-message");
  return nodes.length > 0 ? (nodes[nodes.length - 1]!.textContent ?? "") : "";
}

/** The header's create plus (the action-bar registration). */
function createButton(): HTMLButtonElement | null {
  const scope = document.querySelector<HTMLElement>(".probe-actions");
  if (!scope) return null;
  return (
    Array.from(scope.querySelectorAll<HTMLButtonElement>("button")).find(
      (b) => b.getAttribute("aria-label") === i18n.global.t("common.users.createTitle"),
    ) ?? null
  );
}

let wrapper: ReturnType<typeof mount> | null = null;

beforeEach(() => {
  wrapper = null;
  listUsersMock.mockReset();
  deleteUserMock.mockReset();
  createUserMock.mockReset().mockResolvedValue(undefined);
});

afterEach(() => {
  wrapper?.unmount();
  outlets.splice(0).forEach((o) => o.unmount());
  // The toast store is module-level (hikari state) — clear it so the next
  // case's outlet renders only ITS OWN messages.
  const toast = useToast();
  for (const slot of toast.toasts.slice()) toast.remove(slot.id);
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
    // is the banner for exactly this case (MUT:stale-error-branch —
    // binding the hard error unconditionally reddens this case with the
    // rows gone) — a failed reload must never
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

  it("keeps the role picker a NAMED radiogroup (a11y contract)", async () => {
    listUsersMock.mockResolvedValue({ items: [user("me", "me", "admin")], total: 1 });
    wrapper = mountView();
    await flushPromises();
    const create = createButton();
    expect(create).toBeTruthy();
    create!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();

    // The family radio ships no group role/name of its own, so the host
    // wraps it — dropping that wrapper loses the name the old markup had.
    const group = document.querySelector<HTMLElement>('[role="radiogroup"]');
    expect(group, "the role picker must be a radiogroup").toBeTruthy();
    expect(group!.getAttribute("aria-label")).toBe(i18n.global.t("common.users.role"));
    // And it carries real radio inputs (not the old role="radio" buttons).
    expect(group!.querySelectorAll('input[type="radio"]').length).toBe(2);
  });

  it("renders the scaffold's empty state when the roster is empty", async () => {
    listUsersMock.mockResolvedValue({ items: [], total: 0 });
    wrapper = mountView();
    await flushPromises();
    expect(wrapper.findAll("tbody tr").length).toBe(0);
    expect(wrapper.text()).toContain(i18n.global.t("common.users.empty"));
  });
});

describe("UsersView operation feedback (toasts, not inline blocks)", () => {
  it("raises a failed delete as a toast and grows nothing inside the dialog", async () => {
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
    // The rejection reaches the TOAST surface…
    await vi.waitFor(() => {
      expect(toastText()).toBe("rpc timeout");
    });
    // …the dialog stays open (the target survives the failure)…
    expect(document.body.textContent).toContain(
      i18n.global.t("common.users.deleteConfirm", { name: "ALICE" }),
    );
    // …no row disappeared…
    expect(wrapper.findAll("tbody tr").length).toBe(2);
    // …and NO paragraph grew inside the dialog (the hard rule). Scoped to
    // the dialog's own body: [role="alert"] itself is legitimate now — the
    // toast item carries it — so the absence is asserted where the removed
    // blocks used to live.
    expect(document.querySelector(".dialog-error")).toBeNull();
    expect(document.querySelector(".hk-modal-content [role='alert']")).toBeNull();
    expect(document.querySelector(".hk-modal-content p")?.textContent).toBe(
      i18n.global.t("common.users.deleteConfirm", { name: "ALICE" }),
    );
  });

  it("raises a failed create as a toast with the 409 copy, keeping the form filled", async () => {
    listUsersMock.mockResolvedValue({ items: [user("me", "me", "admin")], total: 1 });
    createUserMock.mockRejectedValue(new ApiError(409, "taken"));
    wrapper = mountView();
    await flushPromises();

    const create = createButton();
    expect(create, "the header must expose the create plus").toBeTruthy();
    create!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();

    const username = document.querySelector<HTMLInputElement>('[data-testid="create-username"]');
    const password = document.querySelector<HTMLInputElement>('[data-testid="create-password"]');
    expect(username).toBeTruthy();
    expect(password).toBeTruthy();
    for (const [input, value] of [
      [username!, "alice"],
      [password!, "secret"],
    ] as Array<[HTMLInputElement, string]>) {
      input.value = value;
      input.dispatchEvent(new Event("input", { bubbles: true }));
    }
    await flushPromises();

    // The dialog's primary footer action ("Create").
    const submit = Array.from(document.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === i18n.global.t("common.actions.create"),
    );
    expect(submit).toBeTruthy();
    submit!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();

    expect(createUserMock).toHaveBeenCalledTimes(1);
    await vi.waitFor(() => {
      expect(toastText()).toBe(i18n.global.t("common.users.conflict"));
    });
    // The dialog stays open — the typed credentials survive the retry.
    expect(document.querySelector<HTMLInputElement>('[data-testid="create-username"]')?.value).toBe("alice");
    // The inline note is gone for good.
    expect(document.querySelector(".dialog-error")).toBeNull();
    expect(document.querySelector(".hk-modal-content [role='alert']")).toBeNull();
  });

  it("keeps a single toast slot for repeated failures of the same operation", async () => {
    // The toast store is module-level and a keyed slot would otherwise
    // stack one paragraph per attempt — the wall-of-text the inline block
    // never produced. Two identical failures must read as ONE message.
    listUsersMock.mockResolvedValue({
      items: [user("me", "me", "admin"), user("u2", "alice", "member")],
      total: 5,
    });
    deleteUserMock.mockRejectedValue("rpc timeout");
    wrapper = mountView();
    await flushPromises();
    const del = wrapper.find(`button[aria-label="${i18n.global.t("common.actions.delete")}"]`);

    for (let attempt = 0; attempt < 2; attempt += 1) {
      await del.trigger("click");
      await flushPromises();
      const confirm = Array.from(document.querySelectorAll("button")).find(
        (b) => b.textContent?.trim() === i18n.global.t("common.actions.delete"),
      );
      confirm!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await flushPromises();
    }
    await vi.waitFor(() => {
      expect(toastText()).toBe("rpc timeout");
    });
    expect(document.querySelectorAll(".hk-toast-message").length).toBe(1);
  });
});