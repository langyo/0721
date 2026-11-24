import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The API layer stays mocked; the clipboard contract is the subject.
const listKeysMock = vi.fn();
const createKeyMock = vi.fn();
const revokeKeyMock = vi.fn();
vi.mock("@/api", () => ({
  listKeys: (...args: unknown[]) => listKeysMock(...args),
  createKey: (...args: unknown[]) => createKeyMock(...args),
  revokeKey: (...args: unknown[]) => revokeKeyMock(...args),
}));

import KeysView from "./KeysView";
import { HkToast, useToast } from "@celestia-island/hikari";
import { provideActionBar } from "@/composables/useActionBar";
import { createAppI18n } from "@/i18n";

const i18n = createAppI18n();
import { defineComponent, h } from "vue";

/**
 * The key page's clipboard tri-state (R3 S1): idle → copied(target) →
 * back to idle, and a REJECTED clipboard write must surface visibly
 * instead of dying silently (the pattern the share dialog set).
 *
 * The failure half is a TOAST (the family feedback contract): the
 * copy-once card: the page-level variant carried a `role="alert"`
 * paragraph beside it (removed in the sweep; the invariant suite at
 * `src/__invariants__/inlineNotices.test.ts` holds that removal), and
 * buttons. The card itself is the create window's SECOND step — the
 * minted key appears inside the dialog where the action happened, with
 * as a block grown at the top of the page after the dialog closed.
 */

/** Every mounted toast outlet — cleanup owns their lifetime. */
const outlets: Array<{ unmount: () => void }> = [];

/** The text of the toast surface (empty when nothing is raised). */
function toastTexts(): string[] {
  return [...document.querySelectorAll(".hk-toast-message")].map((el) => el.textContent ?? "");
}

const KEY_ROW = {
  id: "k1",
  name: "agent",
  key_prefix: "i0721-abcd",
  last_used_at: null,
  revoked_at: null,
  created_at: "2026-11-22T00:00:00Z",
};

// The create dialog opens from the header action bar's Create slot, so
// the harness provides a real action bar (same shape as the portal test).
const Harness = defineComponent({
  name: "KeysHarness",
  setup(_, { expose }) {
    const bar = provideActionBar();
    expose({ bar });
    return () => [
      h("div", { class: "harness-actions" }, bar.actions.value.flatMap((r) => r())),
      h(KeysView),
    ];
  },
});

function mountView() {
  // The dialogs teleport to <body>, so queries go through `document`
  // (attachTo keeps the component tree mounted for the event handlers).
  const host = document.createElement("div");
  document.body.appendChild(host);
  const wrapper = mount(Harness, {
    attachTo: host,
    global: { plugins: [createAppI18n()] },
  });
  // The family toast outlet renders what this view raises (the app
  // mounts one at the root; a test mounts its own).
  outlets.push(mount(HkToast, { attachTo: document.body, props: { teleport: false } }));
  return wrapper;
}

/// The teleported element by data-testid (global, not within the wrapper).
function byTestId(id: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(`[data-testid="${id}"]`);
  if (!el) throw new Error(`no element with data-testid=${id}`);
  return el;
}

async function clickTestId(id: string) {
  byTestId(id).click();
  await flushPromises();
}

/// Open the create dialog through the CANONICAL create slot button.
async function openCreateDialog(wrapper: ReturnType<typeof mountView>) {
  // The Create slot renders as the plus icon button labelled from the
  // page-chrome contract ("Create key"); clicking it opens the dialog.
  const create = wrapper.find('.harness-actions button[aria-label="Create key"]');
  expect(create.exists()).toBe(true);
  await create.trigger("click");
  await flushPromises();
}

describe("KeysView clipboard", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    if (!navigator.clipboard) {
      Object.defineProperty(navigator, "clipboard", {
        value: { writeText: vi.fn() },
        configurable: true,
      });
    }
    listKeysMock.mockReset().mockResolvedValue([KEY_ROW]);
    createKeyMock.mockReset();
    revokeKeyMock.mockReset();
  });

  afterEach(() => {
    outlets.splice(0).forEach((o) => o.unmount());
    // The toast store is module-level (hikari state) — clear it so the
    // next case's outlet renders only ITS OWN messages.
    const toast = useToast();
    for (const slot of toast.toasts.slice()) toast.remove(slot.id);
    vi.useRealTimers();
  });

  async function openCreateCard(wrapper: ReturnType<typeof mountView>) {
    createKeyMock.mockResolvedValue({
      id: "k2",
      name: "agent",
      key: "i0721-fedcba9876543210fedcba9876543210",
    });
    await openCreateDialog(wrapper);
    const nameInput = byTestId("key-name") as HTMLInputElement;
    nameInput.value = "agent";
    nameInput.dispatchEvent(new Event("input"));
    await flushPromises();
    await clickTestId("new-key");
    // The minted key rides INSIDE the secondary window now (it used to
    // grow at the top of the page after the dialog closed — the
    // page-module insertion the user banned), so the card is teleported
    // and must be read document-scoped.
    const card = byTestId("copy-once-card");
    // Structural pin: it is dialog content rather than a page-level block.
    expect(document.querySelector('.keys-page > [data-testid="copy-once-card"]')).toBeNull();
    expect(card.closest(".hk-modal-content, .hk-sheet, [role='dialog']")).not.toBeNull();
    return card;
  }

  it("keeps the create window open on the minted key and closes on confirm", async () => {
    // MUT:copy-once-in-dialog — re-opening the page-level card (or closing
    // the dialog on success) reddens this: the credential must be shown in
    // the window where the action happened, with no page-level insertion.
    const wrapper = mountView();
    await flushPromises();
    await openCreateCard(wrapper);
    // Still open, showing the key.
    expect(byTestId("plaintext-key").textContent).toBe(
      "i0721-fedcba9876543210fedcba9876543210",
    );
    await clickTestId("copy-once-done");
    // Closed, and nothing was left behind in the page.
    expect(document.querySelector("[data-testid='plaintext-key']")).toBeNull();
    expect(document.querySelector("[data-testid='copy-once-card']")).toBeNull();
    expect(document.querySelector(".keys-page > [data-testid='copy-once-card']")).toBeNull();
    wrapper.unmount();
  });

  it("copies the plaintext key and marks the target, then returns to idle", async () => {
    const wrapper = mountView();
    await flushPromises();
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.spyOn(navigator.clipboard, "writeText").mockImplementation(writeText);
    await openCreateCard(wrapper);

    await clickTestId("copy-key");
    expect(writeText).toHaveBeenCalledWith("i0721-fedcba9876543210fedcba9876543210");
    // The button flips to its "copied" label, then back after 3 s.
    // The button flips to the localized "copied" label on the COPIED
    // target only, and the reset happens at the 3 s mark (2999 ms keeps
    // it, 3100 ms clears it — the duration is pinned, not just "some
    // timer exists").
    expect(byTestId("copy-key").textContent?.trim()).toBe("Copied!");
    expect(byTestId("copy-config").textContent?.trim()).not.toBe("Copied!");
    vi.advanceTimersByTime(2999);
    await flushPromises();
    expect(byTestId("copy-key").textContent?.trim()).toBe("Copied!");
    vi.advanceTimersByTime(1);
    await flushPromises();
    expect(byTestId("copy-key").textContent?.trim()).not.toBe("Copied!");
    vi.useRealTimers();
  });

  it("copies the config and prompt templates with the key substituted", async () => {
    const wrapper = mountView();
    await flushPromises();
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.spyOn(navigator.clipboard, "writeText").mockImplementation(writeText);
    await openCreateCard(wrapper);

    await clickTestId("copy-config");
    const config = writeText.mock.calls.at(-1)?.[0] as string;
    expect(config).toContain("i0721-fedcba9876543210fedcba9876543210");
    expect(config).toContain("/api/mcp/sse");

    await clickTestId("copy-prompt");
    const prompt = writeText.mock.calls.at(-1)?.[0] as string;
    expect(prompt).toContain("i0721-fedcba9876543210fedcba9876543210");
    // The prompt payload is NOT the config payload (the two buttons must
    // not copy the same template).
    expect(prompt).not.toBe(config);
    vi.useRealTimers();
  });

  it("surfaces a rejected clipboard write instead of dying silently", async () => {
    const wrapper = mountView();
    await flushPromises();
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(new Error("denied"));
    await openCreateCard(wrapper);

    await clickTestId("copy-key");
    // The raised message is the localized copyFailed string, and the
    // button is NOT left showing the success label.
    await vi.waitFor(() => {
      expect(document.querySelector(".hk-toast-message")?.textContent).toBe(
        "Copy failed — select the text and copy it manually.",
      );
    });
    expect(byTestId("copy-key").textContent?.trim()).not.toBe("Copied!");
    // The card itself grows nothing: the failure is a toast, not a
    // paragraph beside the buttons.
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
    expect(document.querySelector('[data-testid="copy-failed"]')).toBeNull();
  });

  it("clears a stale success mark when a later copy fails", async () => {
    const wrapper = mountView();
    await flushPromises();
    const writeText = vi
      .fn()
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error("denied"));
    vi.spyOn(navigator.clipboard, "writeText").mockImplementation(writeText);
    await openCreateCard(wrapper);

    await clickTestId("copy-key");
    expect(byTestId("copy-key").textContent?.trim()).toBe("Copied!");
    // Second copy (config) fails: the success mark on the key button must
    // not survive, and the failure must be visible.
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(
      new Error("denied"),
    );
    await clickTestId("copy-config");
    expect(byTestId("copy-key").textContent?.trim()).not.toBe("Copied!");
    await vi.waitFor(() => {
      expect(toastTexts()).toContain(
        "Copy failed — select the text and copy it manually.",
      );
    });
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
  });
});

describe("KeysView stale lane (family scaffold)", () => {
  it("keeps the rows on a failed reload and shows the stale banner", async () => {
    // The polarity pin: if the hard error ever wins while rows are on
    // screen, the body is replaced and the roster blanks.
    listKeysMock.mockReset().mockResolvedValue([KEY_ROW]);
    const wrapper = mountView();
    await flushPromises();
    expect(wrapper.findAll("tbody tr").length).toBeGreaterThan(0);

    listKeysMock.mockRejectedValueOnce(new Error("reload boom"));
    // Scoped to THIS harness host: earlier cases in this file append
    // their own hosts to the document, and a document-wide query can
    // grab a leftover button instead of this view's refresh action.
    const host = wrapper.element as HTMLElement;
    const refresh = Array.from(host.querySelectorAll("button")).find(
      (b) => b.getAttribute("aria-label") === "Refresh",
    );
    expect(refresh, "the harness must render the refresh action").toBeTruthy();
    refresh!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    await flushPromises();
    expect(listKeysMock).toHaveBeenCalledTimes(2);
    expect(wrapper.findAll("tbody tr").length).toBeGreaterThan(0);
    expect(wrapper.text()).toContain("reload boom");
    wrapper.unmount();
  });
});

describe("KeysView error surface (family scaffold)", () => {
  it("hands a list failure to the table scaffold instead of a bare paragraph", async () => {
    listKeysMock.mockRejectedValue(new Error("boom"));
    const wrapper = mount(KeysView, { global: { plugins: [i18n] } });
    await flushPromises();
    // The scaffold's alert is where a hard error belongs (the other three
    // rosters bind it too); a hand-rolled <p class="err"> was the old
    // shape and is gone from the template.
    expect(wrapper.find(".hk-alert").exists()).toBe(true);
    expect(wrapper.find(".err").exists()).toBe(false);
  });

  it("still reports a non-Error rejection instead of showing a silent empty roster", async () => {
    // A JSON-RPC/WS lane commonly rejects with a bare string; the old
    // fallback turned that into "" which renders NO alert at all, so a
    // failed cold start read as "no keys yet".
    listKeysMock.mockRejectedValue("rpc timeout");
    const wrapper = mount(KeysView, { global: { plugins: [i18n] } });
    await flushPromises();
    expect(wrapper.find(".hk-alert").exists()).toBe(true);
    expect(wrapper.text()).toContain("rpc timeout");
    wrapper.unmount();

    // And the localized fallback for anything else (null, objects) —
    // String(e) flattening would ship "undefined" to users.
    listKeysMock.mockReset().mockRejectedValue(undefined);
    const w2 = mount(KeysView, { global: { plugins: [createAppI18n()] } });
    await flushPromises();
    expect(w2.find(".hk-alert").exists()).toBe(true);
    expect(w2.text()).toContain(i18n.global.t("common.status.error"));
    w2.unmount();
  });
});
