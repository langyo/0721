import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter, type Router } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const listTimelineMock = vi.fn();
vi.mock("@/api/files", () => ({
  listTimeline: (...args: unknown[]) => listTimelineMock(...args),
}));

import TimelineView from "./TimelineView.vue";
import FilesView from "./FilesView.vue";
import { provideActionBar } from "@/composables/useActionBar";
import { createAppI18n } from "@/i18n";
import type { FileDto } from "@/types";

/**
 * The master-detail contract on the three list surfaces: picking a row
 * opens the pane (the shared detail renders the row's identity), closing
 * returns the pane to its hint, and — on the files surface — a folder
 * click still NAVIGATES while a file click selects.
 */

const i18n = createAppI18n();

function file(id: string, over: Partial<FileDto> = {}): FileDto {
  return {
    id,
    owner_id: "u1",
    name: `${id}.webp`,
    kind: "image",
    mime: "image/webp",
    size: 2048n,
    sha256: id,
    original_name: `${id}-original.webp`,
    url: `/media/${id}`,
    created_at: "2026-11-01T10:00:00Z",
    ...over,
  } as FileDto;
}

function withHarness(View: typeof TimelineView | typeof FilesView) {
  return {
    components: { View },
    setup() {
      provideActionBar();
    },
    template: "<div><View /></div>",
  };
}

function mountTimeline(): { wrapper: ReturnType<typeof mount>; router: Router } {
  const pinia = createPinia();
  setActivePinia(pinia);
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/", component: { template: "<div/>" } },
      { path: "/files/:folderId?", name: "files-folder", component: { template: "<div/>" } },
    ],
  });
  const wrapper = mount(withHarness(TimelineView), { global: { plugins: [pinia, router, i18n] } });
  return { wrapper, router };
}

let wrapper: ReturnType<typeof mount> | null = null;

beforeEach(() => {
  wrapper = null;
  listTimelineMock.mockReset().mockResolvedValue({ total: 0, offset: 0, limit: 40, items: [] });
});

afterEach(() => {
  wrapper?.unmount();
});

describe("TimelineView (master-detail)", () => {
  it("opens the detail pane with the picked row's identity", async () => {
    // Wide viewport: the two-column split renders the pane beside the
    // roster, so the closed pane shows its hint.
    window.innerWidth = 1280;
    listTimelineMock.mockResolvedValue({ total: 2, offset: 0, limit: 40, items: [file("a"), file("b")] });
    const { wrapper: w } = mountTimeline();
    wrapper = w;
    await flushPromises();
    expect(w.find(".hk-master-detail").attributes("data-split")).toBe("two");
    expect(w.text()).toContain(i18n.global.t("common.files.pickHint"));

    await w.findAll(".tile-btn")[0]!.trigger("click");
    await flushPromises();
    // The shared detail carries the row's identity and metadata.
    expect(w.text()).toContain("a-original.webp");
    expect(w.text()).toContain(i18n.global.t("common.files.uploaded"));
  });

  it("returns to the hint after close", async () => {
    window.innerWidth = 1280;
    listTimelineMock.mockResolvedValue({ total: 1, offset: 0, limit: 40, items: [file("a")] });
    const { wrapper: w } = mountTimeline();
    wrapper = w;
    await flushPromises();
    await w.findAll(".tile-btn")[0]!.trigger("click");
    await flushPromises();
    expect(w.text()).toContain("a-original.webp");

    const close = w.find(`button[aria-label="${i18n.global.t("common.actions.cancel")}"]`);
    expect(close.exists()).toBe(true);
    await close.trigger("click");
    await flushPromises();
    expect(w.text()).toContain(i18n.global.t("common.files.pickHint"));
    expect(w.text()).not.toContain("a-original.webp");
  });

  it("on a narrow viewport the pane is a drawer that only exists while open", async () => {
    // The family contract: below the split the roster owns the full
    // width and the detail mounts ONLY while a detail is open.
    window.innerWidth = 800;
    listTimelineMock.mockResolvedValue({ total: 1, offset: 0, limit: 40, items: [file("a")] });
    const { wrapper: w } = mountTimeline();
    wrapper = w;
    await flushPromises();
    expect(w.find(".hk-master-detail").attributes("data-split")).toBe("single");
    expect(w.text()).not.toContain("a-original.webp");

    await w.findAll(".tile-btn")[0]!.trigger("click");
    await flushPromises();
    expect(w.text()).toContain("a-original.webp");

    const close = w.find(`button[aria-label="${i18n.global.t("common.actions.cancel")}"]`);
    await close.trigger("click");
    await flushPromises();
    expect(w.text()).not.toContain("a-original.webp");
  });
});