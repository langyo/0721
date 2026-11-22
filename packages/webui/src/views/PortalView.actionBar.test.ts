import { flushPromises, mount } from "@vue/test-utils";
import { defineComponent, h } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The API layer stays mocked (the slot contract is the subject, not the
// network); the action bar itself is the REAL composable — no mocking of
// useActionBar, so a slot regression in the view fails here.
const listFilesMock = vi.fn();
const uploadFileMock = vi.fn();
vi.mock("@/api", () => ({
  listFiles: (...args: unknown[]) => listFilesMock(...args),
  uploadFile: (...args: unknown[]) => uploadFileMock(...args),
  deleteFile: vi.fn(),
}));

import PortalView from "./PortalView.vue";
import { provideActionBar } from "@/composables/useActionBar";
import { createAppI18n } from "@/i18n";

/**
 * The page-chrome contract, pinned end-to-end (R3 S3, kill M3b): the
 * portal's primary create action — the upload — registers in the header
 * action bar under the CANONICAL Create slot, so the cluster renders it
 * as the plus left of the refresh (hikari page-chrome; SLOT_RANK in
 * useActionBar is the single ranking table). A regression that rides a
 * custom slot string instead breaks the canonical order silently — this
 * test is what turns that red.
 */

const ActionBarHarness = defineComponent({
  name: "ActionBarHarness",
  setup(_, { expose }) {
    const bar = provideActionBar();
    expose({ bar });
    return () => [
      h("div", { class: "harness-actions" }, bar.actions.value.flatMap((r) => r())),
      h(PortalView),
    ];
  },
});

async function mountHarness() {
  const wrapper = mount(ActionBarHarness, {
    global: { plugins: [createAppI18n()] },
  });
  await flushPromises();
  return wrapper;
}

beforeEach(() => {
  listFilesMock.mockReset();
  uploadFileMock.mockReset();
  listFilesMock.mockResolvedValue({ total: 0, offset: 0, limit: 30, items: [] });
});

afterEach(() => {
  localStorage.clear();
});

describe("PortalView page chrome", () => {
  it("registers the upload in the canonical Create slot", async () => {
    const wrapper = await mountHarness();
    // VTU's `vm` types carry no expose() inference — cast to the shape
    // the harness exposes (the runtime contract the assertions ride).
    const vm = wrapper.vm as unknown as {
      bar: ReturnType<typeof provideActionBar>;
    };
    const registrations = vm.bar.actionRegistrations.value as {
      slot: string;
      seq: number;
    }[];
    // The upload IS registered (the plus exists at all)…
    expect(registrations.length).toBeGreaterThan(0);
    // …under the canonical Create slot — not a custom string.
    expect(
      registrations.map((r) => r.slot),
    ).toContain("create");
  });

  it("renders the create button LEFT of the refresh (slot-rank order)", async () => {
    const wrapper = await mountHarness();
    // The derived `actions` list is what the header renders — rank-sorted
    // by SLOT_RANK, independent of registration order (the view registers
    // its refresh before its create). Pin the VISUAL order through the
    // real rendered harness: the upload's accessible name is the label
    // usePageCreateAction registered ("Upload"), the refresh's is
    // "Refresh".
    const buttons = wrapper
      .findAll(".harness-actions button")
      .map((b) => b.attributes("aria-label") ?? b.attributes("title") ?? "");
    const uploadAt = buttons.findIndex((label) => label === "Upload");
    const refreshAt = buttons.findIndex((label) => label === "Refresh");
    expect(uploadAt).toBeGreaterThanOrEqual(0);
    expect(refreshAt).toBeGreaterThanOrEqual(0);
    expect(uploadAt).toBeLessThan(refreshAt);
  });
});
