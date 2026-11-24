import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const shareListMock = vi.fn();
const shareDeleteMock = vi.fn();
vi.mock("@/api/files", () => ({
  shareList: (...args: unknown[]) => shareListMock(...args),
  shareDelete: (...args: unknown[]) => shareDeleteMock(...args),
}));

import SharesView from "./SharesView";
import { HkToast, useToast } from "@celestia-island/hikari";
import { provideActionBar } from "@/composables/useActionBar";
import { createAppI18n } from "@/i18n";
import type { ShareDto } from "@/types";

/**
 * The share roster's outcome lanes: a revoke that fails must SAY so
 * (the row stays — the user can retry or investigate), a revoke that
 * succeeds must say so too, and neither may leave a note sitting over
 * the other (MUT:revoke-note-reset — deleting the reset reddens this
 * file's cases).
 *
 * Both lanes are TOASTS (the family feedback contract): the page's own
 * DOM carries no `role="alert"` / `role="status"` paragraph at all.
 */

const i18n = createAppI18n();

/** Every mounted toast outlet — cleanup owns their lifetime. */
const outlets: Array<{ unmount: () => void }> = [];

function share(token: string): ShareDto {
  return {
    token,
    file_id: "f1",
    file_name: `notes-${token}.txt`,
    file_size: 1234n,
    created_at: "2026-11-01T10:00:00Z",
    owner_id: "u1",
    allow_preview: true,
  } as ShareDto;
}

function mountView(): ReturnType<typeof mount> {
  const pinia = createPinia();
  setActivePinia(pinia);
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: "/", component: { template: "<div/>" } }],
  });
  const Harness = {
    components: { SharesView },
    setup() {
      provideActionBar();
    },
    template: "<div><SharesView /></div>",
  };
  const wrapper = mount(Harness, { global: { plugins: [pinia, router, i18n] } });
  // The family toast outlet renders what this view raises (the app
  // mounts one at the root; a test mounts its own).
  outlets.push(mount(HkToast, { attachTo: document.body, props: { teleport: false } }));
  return wrapper;
}

/** The toast slot of a type, or undefined — read off the module store
 *  (the outlet renders it; the store is the assertion surface). */
function toastOf(type: "error" | "success") {
  return useToast().toasts.find((slot) => slot.type === type);
}

/** Every message currently on the toast surface. */
function toastTexts(): string[] {
  return [...document.querySelectorAll(".hk-toast-message")].map((el) => el.textContent ?? "");
}

let wrapper: ReturnType<typeof mount> | null = null;

beforeEach(() => {
  wrapper = null;
  shareListMock.mockReset().mockResolvedValue([]);
  shareDeleteMock.mockReset();
});

afterEach(() => {
  wrapper?.unmount();
  outlets.splice(0).forEach((o) => o.unmount());
  // The toast store is module-level (hikari state) — clear it so the
  // next case's outlet renders only ITS OWN messages.
  const toast = useToast();
  for (const slot of toast.toasts.slice()) toast.remove(slot.id);
});

describe("SharesView (family table roster)", () => {
  it("renders one row per share with the truncated link", async () => {
    shareListMock.mockResolvedValue([share("tok_abcdefgh1234"), share("tok_second99999")]);
    wrapper = mountView();
    await flushPromises();
    expect(wrapper.findAll("tbody tr").length).toBe(2);
    expect(wrapper.text()).toContain("notes-tok_abcdefgh1234.txt");
    expect(wrapper.text()).toContain("/s/tok_abcdefgh…");
  });

  it("surfaces a failed revoke as a toast and keeps the row", async () => {
    shareListMock.mockResolvedValue([share("tok_abcdefgh1234")]);
    shareDeleteMock.mockRejectedValue(new Error("share_delete denied"));
    wrapper = mountView();
    await flushPromises();
    const revoke = wrapper.find(`button[aria-label="${i18n.global.t("common.files.revoke")}"]`);
    expect(revoke.exists()).toBe(true);
    await revoke.trigger("click");
    await flushPromises();
    // The row survives and the failure is announced — a silent catch
    // (MUT:revoke-failure-lane — re-silencing the catch reddens it)
    // leaves a live share on screen with no explanation.
    expect(wrapper.findAll("tbody tr").length).toBe(1);
    await vi.waitFor(() => {
      expect(document.querySelector(".hk-toast-message")?.textContent).toBe("share_delete denied");
    });
    expect(toastOf("success")).toBeUndefined();
    // The failure is NOT a paragraph beside the roster.
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
    expect(wrapper.text()).not.toContain("share_delete denied");
  });

  it("dismisses only its own success note, never another view's", async () => {
    // MUT:shares-success-key — the revoked note used to be cleared by
    // removing the first SUCCESS slot, which is the same unkeyed slot
    // every surface shares: a failed revoke would wipe an unrelated
    // view's confirmation (e.g. the settings page's "saved") off screen.
    const toast = useToast();
    // A different surface's confirmation, sitting in the shared slot.
    toast.success("Settings saved");
    shareListMock.mockResolvedValue([share("tok_abcdefgh1234")]);
    shareDeleteMock.mockRejectedValue(new Error("nope"));
    wrapper = mountView();
    await flushPromises();

    const revoke = wrapper.find(`button[aria-label="${i18n.global.t("common.files.revoke")}"]`);
    await revoke.trigger("click");
    await flushPromises();

    await vi.waitFor(() => {
      expect(toastTexts()).toContain("nope");
    });
    // The foreign success survived the failure's lane-clearing.
    expect(toastTexts()).toContain("Settings saved");
  });

  it("grows no page-body block on a revoke, success or failure", async () => {
    // MUT:shares-page-body-frozen — the review round's variant: a
    // success-grown note (or any click-grown block) inside the page's own
    // section under a neutral name. The toast is the whole report; the
    // section's element count must be frozen through both lanes.
    shareListMock.mockResolvedValue([share("tok_abcdefgh1234")]);
    shareDeleteMock.mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("nope"));
    wrapper = mountView();
    await flushPromises();

    const section = wrapper.element.querySelector("section")!;
    const frozen = section.querySelectorAll("*").length;
    const buttons = wrapper.findAll(
      `button[aria-label="${i18n.global.t("common.files.revoke")}"]`,
    );

    await buttons[0]!.trigger("click");
    await flushPromises();
    await vi.waitFor(() => {
      expect(toastTexts()).toContain(i18n.global.t("common.files.revoked"));
    });
    expect(section.querySelectorAll("*").length).toBe(frozen);

    const again = wrapper.findAll(
      `button[aria-label="${i18n.global.t("common.files.revoke")}"]`,
    );
    await again[0]!.trigger("click");
    await flushPromises();
    await vi.waitFor(() => {
      expect(toastTexts()).toContain("nope");
    });
    expect(section.querySelectorAll("*").length).toBe(frozen);
    wrapper.unmount();
  });

  it("does not let a stale success note claim a revoke that failed", async () => {
    shareListMock.mockResolvedValue([share("tok_abcdefgh1234"), share("tok_second99999")]);
    shareDeleteMock.mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("nope"));
    wrapper = mountView();
    await flushPromises();
    const buttons = wrapper.findAll(`button[aria-label="${i18n.global.t("common.files.revoke")}"]`);
    // First revoke succeeds: the success toast appears.
    await buttons[0]!.trigger("click");
    await flushPromises();
    await vi.waitFor(() => {
      expect(toastTexts()).toContain(i18n.global.t("common.files.revoked"));
    });
    // Second revoke fails: the stale success must be GONE, not sitting
    // over the failure — the success lane is dismissed on a failure (and
    // the failure itself lands on the error lane, so nothing paints the
    // old note back).
    const after = wrapper.findAll(`button[aria-label="${i18n.global.t("common.files.revoke")}"]`);
    await after[0]!.trigger("click");
    await flushPromises();
    await vi.waitFor(() => {
      expect(toastTexts()).toContain("nope");
    });
    expect(toastOf("success")).toBeUndefined();
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
    expect(wrapper.text()).not.toContain(i18n.global.t("common.files.revoked"));
  });

  it("carries a bare-string rejection instead of an empty banner", async () => {
    // The JSON-RPC lane's common rejection shape; the empty-message
    // case renders no alert at all.
    shareListMock.mockRejectedValue("rpc timeout");
    wrapper = mountView();
    await flushPromises();
    // The ROSTER fetch's failure stays the scaffold's own lane (it is the
    // table's state, not an operation's transient result).
    expect(wrapper.find(".hk-alert").exists()).toBe(true);
    expect(wrapper.text()).toContain("rpc timeout");
  });

  it("drops the row after a successful revoke reloads the roster", async () => {
    shareListMock.mockResolvedValueOnce([share("tok_abcdefgh1234"), share("tok_second99999")]);
    shareDeleteMock.mockResolvedValue(undefined);
    shareListMock.mockResolvedValueOnce([share("tok_second99999")]);
    wrapper = mountView();
    await flushPromises();
    const buttons = wrapper.findAll(`button[aria-label="${i18n.global.t("common.files.revoke")}"]`);
    await buttons[0]!.trigger("click");
    await flushPromises();
    await flushPromises();
    expect(wrapper.findAll("tbody tr").length).toBe(1);
    await vi.waitFor(() => {
      expect(document.querySelector(".hk-toast-message")?.textContent).toBe(
        i18n.global.t("common.files.revoked"),
      );
    });
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
  });
});