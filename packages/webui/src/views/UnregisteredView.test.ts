import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The unregistered-object page's wire contract (P3):
//  - the show switch rides `storage.unregistered.get/put` (R-E) — the
//    OFF default is the SERVER's decision, the view only mirrors it;
//  - the listing calls `storage.unregistered.list` with the chosen
//    slug (the server answers -32006 when the switch is off; the view
//    surfaces the error, it does not retry);
//  - the adopt action sends bucket+key plus the optional owner/parent/
//    name fields — and there is NO delete affordance anywhere (the
//    adopt-then-delete-only rule, §7.5);
//  - the ipfs "cannot be scanned" refusal renders the distinct notice.
//
// MUT:p3-adopt-wire — dropping a field (or sending `show` state on the
// adopt call) reddens the payload test. MUT:p3-no-delete — rendering any
// delete affordance for an unregistered row reddens the affordance test.

const getShowMock = vi.fn();
const setShowMock = vi.fn();
const listMock = vi.fn();
const adoptMock = vi.fn();
const listBucketsMock = vi.fn();
const listUsersMock = vi.fn();
vi.mock("@/api/unregistered", () => ({
  getUnregisteredShow: (...a: unknown[]) => getShowMock(...a),
  setUnregisteredShow: (...a: unknown[]) => setShowMock(...a),
  listUnregistered: (...a: unknown[]) => listMock(...a),
  adoptUnregistered: (...a: unknown[]) => adoptMock(...a),
}));
vi.mock("@/api/buckets", () => ({
  listBuckets: (...a: unknown[]) => listBucketsMock(...a),
}));
vi.mock("@/api", () => ({
  listUsers: (...a: unknown[]) => listUsersMock(...a),
}));

import UnregisteredView from "./UnregisteredView";
import { HkToast, useToast } from "@celestia-island/hikari";
import { createAppI18n } from "@/i18n";

const i18n = createAppI18n();

const harnesses: Array<{ unmount: () => void; element: Element & { remove?: () => void } }> = [];
const outlets: Array<{ unmount: () => void }> = [];

function teardown() {
  harnesses.splice(0).forEach((h) => {
    h.unmount();
    h.element?.remove?.();
  });
  outlets.splice(0).forEach((o) => o.unmount());
  document.body.innerHTML = "";
  const toast = useToast();
  for (const slot of toast.toasts.slice()) toast.remove(slot.id);
}

const ORPHAN = {
  key: "a".repeat(64),
  size: 2048n,
  modified_at: "unknown",
  hinted_mime: "application/octet-stream",
};

function mountView() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const wrapper = mount(UnregisteredView, {
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

beforeEach(() => {
  getShowMock.mockReset().mockResolvedValue({ show: false });
  setShowMock.mockReset().mockResolvedValue(undefined);
  listMock.mockReset().mockResolvedValue({ total: 1, offset: 0, limit: 50, items: [ORPHAN] });
  adoptMock.mockReset().mockResolvedValue(undefined);
  listBucketsMock.mockReset().mockResolvedValue({
    buckets: [{ id: "b1", slug: "media", backend: "local", enabled: true }],
  });
  listUsersMock.mockReset().mockResolvedValue({
    total: 1,
    items: [{ id: "u1", username: "member", display_name: "Member", role: "member" }],
  });
});

afterEach(teardown);

describe("the unregistered page (P3)", () => {
  it("mirrors the server's show state and PUTs the flip", async () => {
    mountView();
    await flushPromises();
    expect(getShowMock).toHaveBeenCalled();

    const track = byTestId("unreg-show").querySelector<HTMLElement>(".hk-switch-track");
    track!.click();
    await flushPromises();
    expect(setShowMock).toHaveBeenCalledWith(true);
  });

  it("lists the chosen bucket's orphans through storage.unregistered.list", async () => {
    mountView();
    await flushPromises();
    expect(listMock).toHaveBeenCalledWith("media", 0, 200);
    const body = document.body.textContent ?? "";
    expect(body).toContain(ORPHAN.key);
  });

  it("sends the adopt wire shape with the optional fields present", async () => {
    mountView();
    await flushPromises();
    const adoptButton = byTestId(`adopt-${ORPHAN.key}`);
    adoptButton.click();
    await flushPromises();

    const submit = byTestId("unreg-adopt-submit");
    (submit as HTMLButtonElement).click();
    await flushPromises();
    expect(adoptMock).toHaveBeenCalledWith({
      bucket: "media",
      key: ORPHAN.key,
      owner_id: undefined,
      parent_id: undefined,
      name: undefined,
    });
  });

  it("never renders a delete affordance for an unregistered row", async () => {
    mountView();
    await flushPromises();
    const text = (document.body.textContent ?? "").toLowerCase();
    expect(text).not.toContain("delete");
  });

  it("shows the scan-refusal notice for an ipfs bucket", async () => {
    listMock.mockRejectedValueOnce(new Error("this backend cannot be scanned"));
    mountView();
    await flushPromises();
    expect(byTestId("unreg-unsupported").textContent).toContain("cannot be scanned");
  });
});
