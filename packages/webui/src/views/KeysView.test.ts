import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The API layer stays mocked; the wire payload and the rendered states are
// the subject.
const listKeysMock = vi.fn();
const createKeyMock = vi.fn();
const updateKeyMock = vi.fn();
const revokeKeyMock = vi.fn();
vi.mock("@/api", () => ({
  listKeys: (...args: unknown[]) => listKeysMock(...args),
  createKey: (...args: unknown[]) => createKeyMock(...args),
  updateKey: (...args: unknown[]) => updateKeyMock(...args),
  revokeKey: (...args: unknown[]) => revokeKeyMock(...args),
}));

import KeysView from "./KeysView";
import { HkToast, useToast } from "@celestia-island/hikari";
import { provideActionBar } from "@/composables/useActionBar";
import { createAppI18n } from "@/i18n";
import type { ApiKeyPublic } from "@/types";

const i18n = createAppI18n();
import { defineComponent, h } from "vue";

/**
 * The key page: the clipboard tri-state, and — this wave — the per-key
 * PERMISSION set and the optional EXPIRY.
 *
 * A key now carries `ops` (read / upload / share, at least one) and an
 * optional `expires_at`, and the create/edit dialogs are one form. What
 * these cases pin (the proof of each bullet is a mutation that turns its
 * test red; the rows are in the PR's mutation table: M9 payload,
 * M20 / M21 / M11 roster, M12 no-permission tag, M14 edit clears the
 * expiry, M22 refusal message):
 *  - the create payload carries EXACTLY the toggled ops (in canonical
 *    order) and the expiry, and cannot be submitted with no permission or
 *    a blank name;
 *  - the roster renders one label per op, "never expires" when there is
 *    no expiry, and the expired tag from the SERVER's `expired` flag
 *    (not re-derived from `expires_at` and the browser clock);
 *  - a key with no permission at all — every pre-upgrade key — renders
 *    an explicit, prominent tag rather than a blank cell;
 *  - edit calls `keys.update` with the full record, and emptying the
 *    expiry field CLEARS the expiry;
 *  - a server refusal reaches the user as a non-empty message.
 *
 * Failure text is a TOAST (the family feedback contract ; the invariant
 * suite at `src/__invariants__/inlineNotices.test.ts` holds the ban on
 * in-page notices), and the copy-once card is the create window's SECOND
 * step — the minted key appears inside the dialog where the action
 * happened, never as a block grown at the top of the page.
 * MUT:M24 — a copy-once card grown at the top of the page turns the
 * clipboard tests red.
 */

/** Every mounted toast outlet — cleanup owns their lifetime. */
const outlets: Array<{ unmount: () => void }> = [];

/**
 * Every mounted harness — unmounted AND detached from the document.
 *
 * Load-bearing beyond tidiness: the dialogs TELEPORT into the shared
 * overlay root, so a previous case's dialog keeps rendering into
 * `<body>` after its wrapper is gone. A document-scoped query then reads
 * the WRONG dialog (this bit the first draft of the permission/expiry
 * cases: the form's hint was asserted against a stale minted-key card).
 * `detach()` removes the case's own host, and the overlay root goes with
 * it once nothing renders into it.
 */
const harnesses: Array<{ unmount: () => void; element: Element & { remove?: () => void } }> = [];

/** Tear everything this file mounted out of the document. */
function teardown() {
  outlets.splice(0).forEach((o) => o.unmount());
  harnesses.splice(0).forEach((h) => {
    h.unmount();
    h.element?.remove?.();
  });
  // The overlay root is hikari's, not ours: clear what is left in <body>
  // (toast outlets, teleported shells) so the next case starts empty.
  document.body.innerHTML = "";
  // The toast store is module-level (hikari state) — clear it so the next
  // case's outlet renders only ITS OWN messages.
  clearToasts();
}

/** The text of the toast surface (empty when nothing is raised). */
function toastTexts(): string[] {
  return [...document.querySelectorAll(".hk-toast-message")].map((el) => el.textContent ?? "");
}

/** The toast surface drained: hikari keeps one slot per (key, type), so a
 *  test that asserts "the message is exactly X" must start from empty. */
function clearToasts(): void {
  const toast = useToast();
  for (const slot of toast.toasts.slice()) toast.remove(slot.id);
}

/** A roster row. `ops` is canonical order, `expired` is the SERVER's
 *  verdict — the generated `ApiKeyPublic` shape. */
function keyRow(overrides: Partial<ApiKeyPublic> = {}): ApiKeyPublic {
  return {
    id: "k1",
    name: "agent",
    key_prefix: "i0721-abcd",
    ops: ["read"],
    // Note: no `expires_at` at all — the "never expires" shape.
    // MUT:M21 — blanking the "never expires" text turns the roster test red.
    expired: false,
    last_used_at: undefined,
    revoked_at: undefined,
    created_at: "2026-11-22T00:00:00Z",
    ...overrides,
  };
}

const KEY_ROW = keyRow();

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
  harnesses.push(wrapper as unknown as (typeof harnesses)[number]);
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

/// Type into a field the way a user does (the `input` event the model
/// listens for), then let the reactive graph settle.
async function fillTestId(id: string, value: string, event = "input") {
  const el = byTestId(id) as HTMLInputElement;
  el.value = value;
  el.dispatchEvent(new Event(event));
  await flushPromises();
}

/// Flip a family switch by clicking its track (HkSwitch's own toggle lane
/// emits `update:modelValue`), then settle.
async function toggleSwitch(id: string) {
  const track = byTestId(id).querySelector<HTMLElement>(".hk-switch-track");
  if (!track) throw new Error(`no .hk-switch-track inside ${id}`);
  track.click();
  await flushPromises();
}

/// The dialog footer's buttons, by their visible label (the family
/// footerActions row — the confirm action lives there, not in the body).
function footerButton(label: string): HTMLButtonElement {
  const buttons = [...document.querySelectorAll<HTMLButtonElement>(".hk-modal-footer button")];
  const found = buttons.find((b) => (b.textContent ?? "").trim() === label);
  if (!found) throw new Error(`no footer button labelled ${label}`);
  return found;
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

/** A `datetime-local` value `hours` from now, in the user's LOCAL zone
 *  (the field is timezone-naive — that is the point of the conversion
 *  lane the view routes it through). */
function localFieldValue(hours: number): string {
  return instantAsLocalField(new Date(Date.now() + hours * 3600_000).toISOString());
}

/// An RFC 3339 instant as the `datetime-local` field spelling of the same
/// moment in the user's LOCAL zone (the field drops seconds).
function instantAsLocalField(instant: string): string {
  const at = new Date(instant);
  const pad = (n: number): string => String(n).padStart(2, "0");
  return (
    `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}` +
    `T${pad(at.getHours())}:${pad(at.getMinutes())}`
  );
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
    updateKeyMock.mockReset();
    revokeKeyMock.mockReset();
    clearToasts();
  });

  afterEach(() => {
    teardown();
    vi.useRealTimers();
  });

  async function openCreateCard(wrapper: ReturnType<typeof mountView>) {
    createKeyMock.mockResolvedValue({
      id: "k2",
      name: "agent",
      key: "i0721-fedcba9876543210fedcba9876543210",
    });
    await openCreateDialog(wrapper);
    await fillTestId("key-name", "agent");
    // A key needs a permission (the form refuses an empty set), so the
    // clipboard cases grant one.
    await toggleSwitch("create-op-read");
    footerButton("Create").click();
    await flushPromises();
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

describe("KeysView create payload (permissions + expiry)", () => {
  beforeEach(() => {
    listKeysMock.mockReset().mockResolvedValue([KEY_ROW]);
    createKeyMock.mockReset().mockResolvedValue({ id: "k2", name: "agent", key: "i0721-new" });
    updateKeyMock.mockReset();
    revokeKeyMock.mockReset();
    clearToasts();
  });

  afterEach(() => {
    teardown();
  });

  it("sends exactly the toggled permissions and the expiry", async () => {
    // MUT:keys-create-ops (the K-wave M6 mutant) — ignoring the switches
    // and always sending all three ops reddens this: the payload's `ops`
    // is what the README and the server's per-tool gate act on.
    const wrapper = mountView();
    await flushPromises();
    await openCreateDialog(wrapper);
    await fillTestId("key-name", "  agent  ");

    // Toggle upload and share, leave read OFF: the payload must be the
    // subset the user picked, in the canonical order (upload, share) —
    // not all three, not the click order.
    await toggleSwitch("create-op-share");
    await toggleSwitch("create-op-upload");
    const expiry = localFieldValue(48);
    await fillTestId("key-expiry", expiry, "input");

    footerButton("Create").click();
    await flushPromises();

    expect(createKeyMock).toHaveBeenCalledTimes(1);
    const [payload] = createKeyMock.mock.calls[0] as [{ name: string; ops: string[]; expires_at?: string }];
    expect(payload.name).toBe("agent");
    expect(payload.ops).toEqual(["upload", "share"]);
    // The field is local wall-clock; the wire carries the RFC 3339
    // instant, so the two must agree as INSTANTS (a string passthrough
    // would ship "2026-11-27T10:00" and the server would read it as UTC).
    expect(payload.expires_at).toBeDefined();
    expect(new Date(payload.expires_at!).getTime()).toBe(new Date(expiry).getTime());
    wrapper.unmount();
  });

  it("omits the expiry when the field is left empty", async () => {
    const wrapper = mountView();
    await flushPromises();
    await openCreateDialog(wrapper);
    await fillTestId("key-name", "agent");
    await toggleSwitch("create-op-read");

    footerButton("Create").click();
    await flushPromises();

    const [payload] = createKeyMock.mock.calls[0] as [Record<string, unknown>];
    expect(payload).toEqual({ name: "agent", ops: ["read"] });
    expect("expires_at" in payload).toBe(false);
    wrapper.unmount();
  });

  it("cannot submit without a permission, and says why", async () => {
    // At least one permission is the server's rule; the form mirrors it by
    // DISABLING the submit (and the disabled button cannot fire the call).
    const wrapper = mountView();
    await flushPromises();
    await openCreateDialog(wrapper);
    await fillTestId("key-name", "agent");

    const submit = footerButton("Create");
    expect(submit.disabled).toBe(true);
    submit.click();
    await flushPromises();
    expect(createKeyMock).not.toHaveBeenCalled();
    // The form names the missing piece instead of leaving a dead button.
    expect(byTestId("create-ops-hint").textContent).toBe("Select at least one permission");

    // Granting one opens the submit AND retires the hint.
    await toggleSwitch("create-op-read");
    expect(footerButton("Create").disabled).toBe(false);
    expect(document.querySelector('[data-testid="create-ops-hint"]')).toBeNull();
    wrapper.unmount();
  });

  it("cannot submit a blank name", async () => {
    const wrapper = mountView();
    await flushPromises();
    await openCreateDialog(wrapper);
    // Whitespace only — `trim()` is the server's rule too.
    await fillTestId("key-name", "   ");
    await toggleSwitch("create-op-read");

    expect(footerButton("Create").disabled).toBe(true);
    footerButton("Create").click();
    await flushPromises();
    expect(createKeyMock).not.toHaveBeenCalled();

    await fillTestId("key-name", "agent");
    expect(footerButton("Create").disabled).toBe(false);
    wrapper.unmount();
  });

  it("refuses an expiry that has already passed", async () => {
    // The client-side mirror of `expires_at must be in the future`; the
    // server still owns the verdict (the case below keeps that lane
    // alive end to end).
    const wrapper = mountView();
    await flushPromises();
    await openCreateDialog(wrapper);
    await fillTestId("key-name", "agent");
    await toggleSwitch("create-op-read");
    await fillTestId("key-expiry", localFieldValue(-2), "input");

    expect(footerButton("Create").disabled).toBe(true);
    expect(byTestId("create-expiry-hint").textContent).toBe("Expiry must be in the future");
    footerButton("Create").click();
    await flushPromises();
    expect(createKeyMock).not.toHaveBeenCalled();
    wrapper.unmount();
  });
});

describe("KeysView roster columns (permissions + expiry)", () => {
  beforeEach(() => {
    listKeysMock.mockReset();
    createKeyMock.mockReset();
    updateKeyMock.mockReset();
    revokeKeyMock.mockReset();
    clearToasts();
  });

  afterEach(() => {
    teardown();
  });

  it("renders one label per permission, in canonical order", async () => {
    // The server reports canonical order already; this pins that the cell
    // prints every op as its own label (not one concatenated string) and
    // that an out-of-order producer is normalized on render.
    listKeysMock.mockResolvedValue([
      keyRow({ id: "k1", name: "reader", ops: ["read"] }),
      keyRow({ id: "k2", name: "writer", ops: ["share", "upload", "read"] }),
    ]);
    const wrapper = mountView();
    await flushPromises();

    const reader = byTestId("key-ops-k1");
    expect(reader.textContent).toContain("Read (list, info, search)");
    expect(reader.textContent).not.toContain("Upload");

    const writer = byTestId("key-ops-k2");
    const labels = [...writer.querySelectorAll(".hk-badge")].map((b) => b.textContent?.trim());
    expect(labels).toEqual([
      "Read (list, info, search)",
      "Upload",
      "Create share links",
    ]);
    wrapper.unmount();
  });

  it('renders "never expires" for a key without an expiry', async () => {
    listKeysMock.mockResolvedValue([keyRow({ id: "k1", ops: ["read"] })]);
    const wrapper = mountView();
    await flushPromises();
    const cell = byTestId("key-expiry-k1");
    expect(cell.textContent).toContain("Never expires");
    expect(cell.textContent).not.toContain("Expired");
    wrapper.unmount();
  });

  it("renders the expired tag from the SERVER's flag, not from a local clock", async () => {
    // Both rows carry an `expires_at` in the PAST and the future
    // respectively — but what decides the tag is `expired`, the field the
    // server computes (it owns the clock and the fail-closed parse).
    const past = "2020-01-01T00:00:00Z";
    const future = new Date(Date.now() + 365 * 24 * 3600_000).toISOString();
    listKeysMock.mockResolvedValue([
      // A past instant the server does NOT flag: must NOT show the tag.
      // MUT:M11 — deriving the tag from the browser clock shows it here.
      keyRow({ id: "k1", name: "live", ops: ["read"], expires_at: past, expired: false }),
      // A future instant the server DOES flag: must show the tag.
      keyRow({ id: "k2", name: "dead", ops: ["read"], expires_at: future, expired: true }),
    ]);
    const wrapper = mountView();
    await flushPromises();

    const live = byTestId("key-expiry-k1");
    expect(live.textContent).not.toContain("Expired");
    expect(live.textContent).not.toContain("Never expires");

    const dead = byTestId("key-expiry-k2");
    expect(dead.textContent).toContain("Expired");
    wrapper.unmount();
  });

  it("a_key_without_permissions_shows_the_no_permissions_tag", async () => {
    // Every key minted before this wave was normalized fail-closed to an
    // EMPTY permission set, so this is the common state right after the
    // upgrade — and such a key can do nothing on the MCP surface. It must
    // read as a loud, actionable state, never as a blank cell.
    // MUT:M12 — inverting or removing the tag condition turns this test red.
    listKeysMock.mockResolvedValue([
      keyRow({ id: "legacy", name: "legacy", ops: [] }),
      keyRow({ id: "granted", name: "granted", ops: ["read"] }),
    ]);
    const wrapper = mountView();
    await flushPromises();

    const legacy = byTestId("key-no-permissions-legacy");
    expect(legacy.textContent).toContain("No permissions — edit to grant");

    // And the inverse: a key WITH permissions shows the labels and no
    // such tag anywhere in its permissions cell.
    expect(document.querySelector('[data-testid="key-no-permissions-granted"]')).toBeNull();
    expect(byTestId("key-ops-granted").textContent).toContain("Read (list, info, search)");
    // Exactly one tag on the page, on the legacy row.
    expect(document.querySelectorAll('[data-testid^="key-no-permissions-"]').length).toBe(1);
    wrapper.unmount();
  });

  it("resolves every permission caption from the locale catalogs", async () => {
    // The captions are looked up as `common.keys.op_<op>` — a DYNAMIC key,
    // which is exactly the shape `check_i18n.py` can only whitelist by
    // wildcard, so it cannot tell a renamed locale key from a live one.
    // A missing key renders the RAW key ("common.keys.op_upload") to the
    // user, which every "contains the label" assertion above would miss.
    // This case resolves each one against the real catalogs instead.
    const { SUPPORTED_LOCALES, defaultMessages } = await import("@/i18n");
    for (const locale of SUPPORTED_LOCALES) {
      const messages = defaultMessages[locale] as { common?: { keys?: Record<string, string> } };
      for (const op of ["read", "upload", "share"] as const) {
        const caption = messages?.common?.keys?.[`op_${op}`];
        expect(typeof caption, `${locale} is missing common.keys.op_${op}`).toBe("string");
        expect((caption ?? "").trim().length).toBeGreaterThan(0);
      }
    }
    // And the rendered captions are the English catalog's, not raw keys.
    listKeysMock.mockResolvedValue([keyRow({ id: "k1", ops: ["read", "upload", "share"] })]);
    const wrapper = mountView();
    await flushPromises();
    const cell = byTestId("key-ops-k1");
    expect(cell.textContent).not.toContain("common.keys.op_");
    expect(cell.textContent).toContain("Upload");
    wrapper.unmount();
  });
});

describe("KeysView edit flow", () => {
  beforeEach(() => {
    listKeysMock.mockReset();
    createKeyMock.mockReset();
    updateKeyMock.mockReset().mockResolvedValue(undefined);
    revokeKeyMock.mockReset();
    clearToasts();
  });

  afterEach(() => {
    teardown();
  });

  it("calls keys.update with the full record and no plaintext key", async () => {
    // `keys.update` is a FULL REPLACEMENT of name / ops / expires_at, so
    // the dialog must submit all three (a partial patch would silently
    // clear whatever it omitted).
    const expiresAt = "2027-03-04T05:06:00Z";
    listKeysMock.mockResolvedValue([
      keyRow({ id: "k1", name: "agent", ops: ["read", "upload"], expires_at: expiresAt }),
    ]);
    const wrapper = mountView();
    await flushPromises();

    await clickTestId("edit-key-k1");
    // The form opens PRE-FILLED from the row.
    expect((byTestId("edit-key-name") as HTMLInputElement).value).toBe("agent");
    // No credential WHILE THE DIALOG IS OPEN — the only place it could be
    // invented: the key is stored hashed, so an edit dialog has no plaintext
    // to show and must not dress the masked prefix up as one. Checked HERE,
    // not only after Save: a successful save closes the dialog, so a later
    // query finds nothing regardless of what the dialog rendered.
    // MUT:M23
    expect(document.querySelector("[data-testid='plaintext-key']")).toBeNull();
    expect(document.querySelector("[data-testid='copy-once-card']")).toBeNull();
    const expiryField = byTestId("edit-key-expiry") as HTMLInputElement;
    expect(expiryField.getAttribute("type")).toBe("datetime-local");
    // The stored UTC instant, re-read as the user's LOCAL wall clock (the
    // field is timezone-naive) — not the raw ISO string, which would shift
    // the instant by the browser's offset.
    expect(expiryField.value).toBe(instantAsLocalField(expiresAt));

    await fillTestId("edit-key-name", "renamed");
    await toggleSwitch("edit-op-upload"); // turn upload OFF
    await toggleSwitch("edit-op-share"); // turn share ON
    await fillTestId("edit-key-expiry", localFieldValue(72), "input");

    footerButton("Save").click();
    await flushPromises();

    expect(updateKeyMock).toHaveBeenCalledTimes(1);
    const [id, payload] = updateKeyMock.mock.calls[0] as [string, {
      name: string;
      ops: string[];
      expires_at?: string;
    }];
    expect(id).toBe("k1");
    expect(payload.name).toBe("renamed");
    expect(payload.ops).toEqual(["read", "share"]);
    expect(new Date(payload.expires_at!).getTime()).toBe(
      new Date(localFieldValue(72)).getTime(),
    );
    // No credential anywhere after the save either — the same invariant as
    // the pre-save check above (which is the half that can actually catch a
    // dialog that invents one). MUT:M23
    expect(document.querySelector("[data-testid='plaintext-key']")).toBeNull();
    expect(document.querySelector("[data-testid='copy-once-card']")).toBeNull();
    // Success is a toast on this view's own slot.
    expect(toastTexts()).toContain("API key updated");
    wrapper.unmount();
  });

  it("clears the expiry when the field is emptied", async () => {
    // The way to say "this key never expires again": submit the record
    // with no `expires_at` at all. The server reads an absent field as
    // "no expiry", so the payload must DROP the key rather than send "".
    // MUT:M14 — sending "" when the field is emptied turns this test red.
    listKeysMock.mockResolvedValue([
      keyRow({ id: "k1", name: "agent", ops: ["read"], expires_at: "2027-01-02T03:04:00Z" }),
    ]);
    const wrapper = mountView();
    await flushPromises();

    await clickTestId("edit-key-k1");
    expect((byTestId("edit-key-expiry") as HTMLInputElement).value).not.toBe("");
    await fillTestId("edit-key-expiry", "", "input");
    footerButton("Save").click();
    await flushPromises();

    const [, payload] = updateKeyMock.mock.calls[0] as [string, Record<string, unknown>];
    expect("expires_at" in payload).toBe(false);
    expect(payload.ops).toEqual(["read"]);
    expect(payload.name).toBe("agent");
    wrapper.unmount();
  });

  it("keeps the edit window open when the server refuses the write", async () => {
    // A server refusal (another user's key, a revoked target, a bad
    // expiry) must reach the user as a non-empty message, and the typed
    // values must survive for the retry.
    listKeysMock.mockResolvedValue([keyRow({ id: "k1", name: "agent", ops: ["read"] })]);
    updateKeyMock.mockRejectedValue(new Error("expires_at must be in the future"));
    const wrapper = mountView();
    await flushPromises();

    await clickTestId("edit-key-k1");
    await fillTestId("edit-key-name", "still-here");
    footerButton("Save").click();
    await flushPromises();

    expect(updateKeyMock).toHaveBeenCalledTimes(1);
    const messages = toastTexts().filter((text) => text.trim().length > 0);
    expect(messages).toContain("expires_at must be in the future");
    // The window is still open, with the typed name.
    expect((byTestId("edit-key-name") as HTMLInputElement).value).toBe("still-here");
    wrapper.unmount();
  });

  it("surfaces a server validation error on create as a non-empty message", async () => {
    // The mirror above refuses the obvious cases; the SERVER is still the
    // authority, and its refusal must land on the toast surface (an empty
    // message would render no toast at all and read as "nothing
    // happened").
    listKeysMock.mockResolvedValue([KEY_ROW]);
    createKeyMock.mockRejectedValue(new Error("at least one permission is required"));
    const wrapper = mountView();
    await flushPromises();
    await openCreateDialog(wrapper);
    await fillTestId("key-name", "agent");
    await toggleSwitch("create-op-read");

    footerButton("Create").click();
    await flushPromises();

    const messages = toastTexts().filter((text) => text.trim().length > 0);
    expect(messages).toContain("at least one permission is required");
    // The window stays open (no minted-key card) so the attempt can be
    // retried.
    expect(document.querySelector("[data-testid='copy-once-card']")).toBeNull();
    wrapper.unmount();
  });

  it("falls back to a non-empty message for a message-less rejection", async () => {
    // `errorText`'s never-empty contract: a rejection carrying nothing
    // must still raise a toast (the shared factory's fallback).
    // MUT:M22 — toasting the raw (empty) rejection message turns this test red.
    listKeysMock.mockResolvedValue([keyRow({ id: "k1", name: "agent", ops: ["read"] })]);
    updateKeyMock.mockRejectedValue(undefined);
    const wrapper = mountView();
    await flushPromises();
    await clickTestId("edit-key-k1");
    footerButton("Save").click();
    await flushPromises();

    expect(toastTexts()).toContain(i18n.global.t("common.status.error"));
    wrapper.unmount();
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
