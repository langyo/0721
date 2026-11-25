import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The bucket registry page's wire contract (P2a):
//  - the upsert submit is a STRICT FULL DOCUMENT (all four mutable
//    fields present — the server's deny_unknown_fields refuses partial
//    docs; a partial wire would also mean a silently reset field);
//  - the grant_on_signup ⇒ anonymous_read coupling is enforced IN THE
//    FORM (the inconsistent combination cannot reach the wire);
//  - a bootstrap bucket shows NO delete affordance at all (R-H — not a
//    disabled button: the family roster convention is no affordance);
//    the server's three refusals surface as toasts for other rows;
//  - the grant editor saves exactly the three toggles; an all-false
//    save is the explicit deny row (revocation) — never a delete call;
//    admin targets are never offered (R-A: the server refuses them).
//
// MUT:p2a-upsert-strict-doc — assembling a partial document reddens the
// wire test. MUT:p2a-signup-coupling — breaking the flip guard reddens
// the coupling test. MUT:p2a-bootstrap-no-delete — rendering a delete
// button for the bootstrap row reddens the affordance test.
// MUT:p2a-grant-all-false-kept — collapsing an all-false save into a
// "delete row" call reddens the payload test.

const listBucketsMock = vi.fn();
const upsertBucketMock = vi.fn();
const deleteBucketMock = vi.fn();
const setDefaultBucketMock = vi.fn();
const getDefaultBucketMock = vi.fn();
const setGrantMock = vi.fn();
const listUsersMock = vi.fn();
vi.mock("@/api/buckets", () => ({
  listBuckets: (...a: unknown[]) => listBucketsMock(...a),
  upsertBucket: (...a: unknown[]) => upsertBucketMock(...a),
  deleteBucket: (...a: unknown[]) => deleteBucketMock(...a),
  getDefaultBucket: (...a: unknown[]) => getDefaultBucketMock(...a),
  setDefaultBucket: (...a: unknown[]) => setDefaultBucketMock(...a),
  setGrant: (...a: unknown[]) => setGrantMock(...a),
  readableBuckets: (rows: unknown[]) => rows,
}));
vi.mock("@/api", () => ({
  listUsers: (...a: unknown[]) => listUsersMock(...a),
}));

import BucketsView from "./BucketsView";
import { HkToast, useToast } from "@celestia-island/hikari";
import { provideActionBar } from "@/composables/useActionBar";
import { createAppI18n } from "@/i18n";
import { defineComponent, h } from "vue";

const i18n = createAppI18n();

const outlets: Array<{ unmount: () => void }> = [];
const harnesses: Array<{ unmount: () => void; element: Element & { remove?: () => void } }> = [];

function teardown() {
  outlets.splice(0).forEach((o) => o.unmount());
  harnesses.splice(0).forEach((h) => {
    h.unmount();
    h.element?.remove?.();
  });
  document.body.innerHTML = "";
  const toast = useToast();
  for (const slot of toast.toasts.slice()) toast.remove(slot.id);
}

function toastTexts(): string[] {
  return [...document.querySelectorAll(".hk-toast-message")].map((el) => el.textContent ?? "");
}

function clearToasts(): void {
  const toast = useToast();
  for (const slot of toast.toasts.slice()) toast.remove(slot.id);
}

function bucket(slug: string, over: Record<string, unknown> = {}) {
  return {
    id: `id-${slug}`,
    slug,
    display_name: slug,
    backend: "local",
    location: "loc",
    endpoint_ref: "x",
    anonymous_read: true,
    grant_on_signup: slug === "bootstrap",
    enabled: true,
    is_bootstrap: slug === "bootstrap",
    created_at: "2026-11-01T00:00:00Z",
    my_access: { read: true, write: true, share: true },
    ...over,
  };
}

const Harness = defineComponent({
  name: "BucketsHarness",
  setup(_, { expose }) {
    const bar = provideActionBar();
    expose({ bar });
    return () => [
      h("div", { class: "harness-actions" }, bar.actions.value.flatMap((r) => r())),
      h(BucketsView),
    ];
  },
});

function mountView() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const wrapper = mount(Harness, {
    attachTo: host,
    global: { plugins: [createAppI18n()] },
  });
  harnesses.push(wrapper as unknown as (typeof harnesses)[number]);
  outlets.push(mount(HkToast, { attachTo: document.body, props: { teleport: false } }));
  return wrapper;
}

function byTestId(id: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(`[data-testid="${id}"]`);
  if (!el) throw new Error(`no element with data-testid=${id}`);
  return el;
}

async function toggleSwitch(id: string) {
  const track = byTestId(id).querySelector<HTMLElement>(".hk-switch-track");
  if (!track) throw new Error(`no .hk-switch-track inside ${id}`);
  track.click();
  await flushPromises();
}

async function fillTestId(id: string, value: string) {
  const el = byTestId(id) as HTMLInputElement;
  el.value = value;
  el.dispatchEvent(new Event("input"));
  await flushPromises();
}

function footerButton(label: string): HTMLButtonElement {
  const buttons = [...document.querySelectorAll<HTMLButtonElement>(".hk-modal-footer button")];
  const found = buttons.find((b) => (b.textContent ?? "").trim() === label);
  if (!found) throw new Error(`no footer button labelled ${label}`);
  return found;
}

function member(id: string) {
  return {
    id,
    username: `u-${id}`,
    display_name: `User ${id}`,
    role: "member",
    created_at: "2026-11-01T00:00:00Z",
  };
}

beforeEach(() => {
  listBucketsMock.mockReset().mockResolvedValue({
    buckets: [bucket("bootstrap"), bucket("media", { is_bootstrap: false, grant_on_signup: false })],
  });
  upsertBucketMock.mockReset().mockResolvedValue({ buckets: [] });
  deleteBucketMock.mockReset().mockResolvedValue(undefined);
  getDefaultBucketMock.mockReset().mockResolvedValue({ bucket: null });
  setDefaultBucketMock.mockReset().mockResolvedValue(undefined);
  setGrantMock.mockReset().mockResolvedValue(undefined);
  listUsersMock.mockReset().mockResolvedValue({ total: 2, offset: 0, limit: 100, items: [member("m1"), member("m2")] });
  clearToasts();
});

afterEach(() => teardown());

describe("BucketsView upsert wire (strict full document)", () => {
  it("sends exactly slug + the four mutable fields", async () => {
    const wrapper = mountView();
    await flushPromises();
    const create = wrapper.find('.harness-actions button[aria-label="Create bucket"]');
    expect(create.exists()).toBe(true);
    await create.trigger("click");
    await flushPromises();

    await fillTestId("bucket-slug", "docs");
    await fillTestId("bucket-display-name", "Documents");
    // The create default is grant_on_signup=ON + anonymous_read=ON (the
    // locked pair). Turning the signup default OFF unlocks the
    // anonymous_read switch; then both can go off.
    await toggleSwitch("bucket-grant-on-signup");
    await toggleSwitch("bucket-anonymous-read");
    await toggleSwitch("bucket-enabled");

    footerButton("Save").click();
    await flushPromises();

    expect(upsertBucketMock).toHaveBeenCalledTimes(1);
    const [doc] = upsertBucketMock.mock.calls[0] as [Record<string, unknown>];
    // MUT:p2a-upsert-strict-doc — any missing field (or an extra one)
    // reddens this: the server refuses both shapes.
    expect(Object.keys(doc).sort()).toEqual(
      ["anonymous_read", "display_name", "enabled", "grant_on_signup", "slug"],
    );
    expect(doc).toEqual({
      slug: "docs",
      display_name: "Documents",
      anonymous_read: false,
      grant_on_signup: false,
      enabled: false,
    });
    wrapper.unmount();
  });

  it("cannot flip grant_on_signup on while anonymous_read is off", async () => {
    const wrapper = mountView();
    await flushPromises();
    const create = wrapper.find('.harness-actions button[aria-label="Create bucket"]');
    await create.trigger("click");
    await flushPromises();

    await fillTestId("bucket-slug", "private");
    // Both switches start ON (the create default: a default-open bucket
    // is public, and the pair is LOCKED — anonymous_read is pinned ON
    // while grant_on_signup is true). The anonymous switch cannot flip
    // OFF here; the signup switch CAN flip off, which then unlocks
    // anonymous_read.
    await toggleSwitch("bucket-anonymous-read");
    const anonAfter = byTestId("bucket-anonymous-read");
    expect(anonAfter.hasAttribute("data-checked")).toBe(true);
    // The grant switch flips OFF (unlocking anonymous_read).
    await toggleSwitch("bucket-grant-on-signup");
    // NOW anonymous_read is unlocked and can go off — and the grant
    // switch (now OFF) is DISABLED: the inconsistent combination can
    // never be re-created.
    await toggleSwitch("bucket-anonymous-read");
    const grantTrack = byTestId("bucket-grant-on-signup").querySelector<HTMLElement>(".hk-switch-track");
    grantTrack!.click();
    await flushPromises();
    expect(upsertBucketMock).not.toHaveBeenCalled();

    footerButton("Save").click();
    await flushPromises();
    expect(upsertBucketMock).toHaveBeenCalledTimes(1);
    const [doc] = upsertBucketMock.mock.calls[0] as [Record<string, unknown>];
    // MUT:p2a-signup-coupling — an inconsistent combination
    // (grant_on_signup=true + anonymous_read=false) reaching the wire
    // reddens this.
    expect(doc.grant_on_signup).toBe(false);
    expect(doc.anonymous_read).toBe(false);
    // The R-A constraint holds: default-open is always anonymous-readable.
    expect(doc.grant_on_signup === true && doc.anonymous_read === false).toBe(false);
    wrapper.unmount();
  });
});

describe("BucketsView delete guard (bootstrap)", () => {
  it("offers NO delete affordance on the bootstrap bucket", async () => {
    const wrapper = mountView();
    await flushPromises();
    expect(document.querySelector('[data-testid="delete-bootstrap"]')).toBeNull();
    // The sibling non-bootstrap row still has one.
    expect(document.querySelector('[data-testid="delete-media"]')).not.toBeNull();
    wrapper.unmount();
  });

  it("surfaces the server's refusal as a toast for a non-bootstrap row", async () => {
    deleteBucketMock.mockRejectedValue(new Error("the bucket still holds 3 live rows — empty it first"));
    const wrapper = mountView();
    await flushPromises();
    byTestId("delete-media").click();
    await flushPromises();
    footerButton("Delete").click();
    await flushPromises();
    expect(toastTexts().some((t) => t.includes("3 live rows"))).toBe(true);
    wrapper.unmount();
  });
});

describe("BucketsView grant editor (R-A)", () => {
  it("saves exactly the three toggles; the all-false save is the deny row, not a delete", async () => {
    const wrapper = mountView();
    await flushPromises();
    byTestId("grants-media").click();
    await flushPromises();

    // m1: read+share on, then share off and write off... then everything
    // off — the EXPLICIT DENY row (revocation = the all-false upsert).
    await toggleSwitch("grant-m1-read");
    await toggleSwitch("grant-m1-share");
    byTestId("grant-save-m1").click();
    await flushPromises();
    expect(setGrantMock).toHaveBeenCalledTimes(1);
    const [user, slug, bits] = setGrantMock.mock.calls[0] as [string, string, Record<string, boolean>];
    expect(user).toBe("m1");
    expect(slug).toBe("media");
    expect(Object.keys(bits).sort()).toEqual(["read", "share", "write"]);
    expect(bits).toEqual({ read: true, write: false, share: true });

    // The revoke path: flip everything off and save again.
    await toggleSwitch("grant-m1-read");
    await toggleSwitch("grant-m1-share");
    byTestId("grant-save-m1").click();
    await flushPromises();
    expect(setGrantMock).toHaveBeenCalledTimes(2);
    const [, , bits2] = setGrantMock.mock.calls[1] as [string, string, Record<string, boolean>];
    // MUT:p2a-grant-all-false-kept — an all-false save becoming a
    // delete-row call (or a reset-to-default) reddens this.
    expect(bits2).toEqual({ read: false, write: false, share: false });
    wrapper.unmount();
  });

  it("offers no editable row for an admin target (role-granted)", async () => {
    listUsersMock.mockResolvedValue({
      total: 2,
      offset: 0,
      limit: 100,
      items: [member("m1"), { ...member("a1"), role: "admin" }],
    });
    const wrapper = mountView();
    await flushPromises();
    byTestId("grants-media").click();
    await flushPromises();
    expect(document.querySelector('[data-testid="grant-row-m1"]')).not.toBeNull();
    // The admin target is never offered as an editable row (R-A: the
    // server refuses grant writes for admins; the UI does not even
    // reach the refusal).
    expect(document.querySelector('[data-testid="grant-row-a1"]')).toBeNull();
    wrapper.unmount();
  });
});
