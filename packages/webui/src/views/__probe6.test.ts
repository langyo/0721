import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
const listKeysMock = vi.fn().mockResolvedValue([]);
const listBucketsMock2 = vi.fn().mockResolvedValue({ buckets: [{ slug: "alpha" }] });
vi.mock("@/api", () => ({ listKeys: (...a: unknown[]) => listKeysMock(...a), createKey: vi.fn(), updateKey: vi.fn(), revokeKey: vi.fn() }));
vi.mock("@/api/buckets", () => ({ listBuckets: (...a: unknown[]) => listBucketsMock2(...a), readableBuckets: (r: unknown[]) => r }));
import KeysView from "@/views/KeysView";
import { provideActionBar } from "@/composables/useActionBar";
import { createAppI18n } from "@/i18n";
import { defineComponent, h } from "vue";
describe("probe6", () => {
  it("slugs", async () => {
    const Harness = defineComponent({ setup(_, {expose}) { const bar = provideActionBar(); expose({bar}); return () => [h("div", { class: "harness-actions" }, bar.actions.value.flatMap((r) => r())), h(KeysView)]; } });
    const host = document.createElement("div"); document.body.appendChild(host);
    const w = mount(Harness, { attachTo: host, global: { plugins: [createAppI18n()] } });
    await flushPromises();
    await w.find('.harness-actions button[aria-label="Create key"]').trigger("click");
    await flushPromises();
    await flushPromises();
    console.log("buckets testids:", [...document.querySelectorAll('[data-testid^="create-bucket-"]')].map(e=>e.getAttribute("data-testid")));
    console.log("buckets-empty hint:", document.querySelector('[data-testid="create-buckets-empty"]')?.textContent ?? "none");
    expect(true).toBe(true);
  }, 8000);
});
