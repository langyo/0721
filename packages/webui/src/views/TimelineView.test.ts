import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter, type Router } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const listTimelineMock = vi.fn();
vi.mock("@/api/files", () => ({
  listTimeline: (...args: unknown[]) => listTimelineMock(...args),
}));

import TimelineView from "./TimelineView";
import FilesView from "./FilesView";
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
    size: 2048,
    sha256: id,
    original_name: `${id}-original.webp`,
    url: `/media/${id}`,
    created_at: "2026-11-01T10:00:00Z",
    ...over,
  };
}

function withHarness(View: typeof TimelineView | typeof FilesView) {
  return {
    components: { View },
    setup() {
      const bar = provideActionBar();
      return { bar };
    },
    // The bar's registered entries render above the view, so the refresh
    // action is a real button (the reload pins drive it).
    template:
      '<div><div class="harness-actions">' +
      '<component :is="() => bar.actions.value.flatMap((r) => r())" />' +
      "</div><View /></div>",
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

  it("drops the pane when a reload no longer carries the row", async () => {
    window.innerWidth = 1280;
    listTimelineMock.mockResolvedValue({ total: 1, offset: 0, limit: 40, items: [file("a")] });
    const { wrapper: w } = mountTimeline();
    wrapper = w;
    await flushPromises();
    await w.findAll(".tile-btn")[0]!.trigger("click");
    await flushPromises();
    expect(w.find(".file-detail").exists()).toBe(true);

    listTimelineMock.mockResolvedValue({ total: 1, offset: 0, limit: 40, items: [file("b")] });
    const host = w.element as HTMLElement;
    const refresh = Array.from(host.querySelectorAll("button")).find(
      (b) => b.getAttribute("aria-label") === i18n.global.t("common.actions.refresh"),
    );
    expect(refresh, "the harness renders the refresh action").toBeTruthy();
    refresh!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    expect(w.find(".file-detail").exists()).toBe(false);
  });

  it("on a narrow viewport the pane is a drawer that only exists while open", async () => {
    // The family contract: below the split the roster owns the full
    // width and the detail mounts ONLY while a detail is open.
    // MUT:setup-timing — set the width BEFORE any component instance
    // exists. `useBreakpoint` reads `window.innerWidth` at SETUP, so a
    // width assigned after the mount never reaches the split (the
    // harness's own resize listener only re-reads on a real resize
    // event). Nothing may mount between this line and `mountTimeline`.
    //
    // 700, not 800: hikari 0.55.488 moved HkMasterDetail's default
    // `splitAt` from **1100** to **768** (the family phone breakpoint), so
    // the narrow form starts BELOW 768 — a width in 768..1100, which used
    // to be narrow, is the wide console now.
    window.innerWidth = 700;
    listTimelineMock.mockResolvedValue({ total: 1, offset: 0, limit: 40, items: [file("a")] });
    const { wrapper: w } = mountTimeline();
    wrapper = w;
    await flushPromises();
    expect(w.find(".hk-master-detail").attributes("data-split")).toBe("single");
    expect(w.text()).not.toContain("a-original.webp");

    await w.findAll(".tile-btn")[0]!.trigger("click");
    await flushPromises();
    // hikari 0.55.488 splits the narrow form in two: a host that binds
    // `@close` (this view does) gets the WINDOW overlay, which teleports
    // to the body — so the detail lives in the DOCUMENT, not inside this
    // wrapper's subtree. The roster stays in place behind it.
    expect(document.body.textContent).toContain("a-original.webp");
    expect(w.find(".hk-master-detail").attributes("data-open")).toBe("true");
    expect(w.findAll(".tile-btn").length).toBe(1);

    // Closing the window routes through the host's own listener, which
    // drops the selection: the overlay unmounts and the name is gone.
    const close = Array.from(document.body.querySelectorAll("button")).find(
      (b) => b.getAttribute("aria-label") === i18n.global.t("common.actions.cancel"),
    );
    expect(close, "the narrow window carries a close affordance").toBeTruthy();
    close!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    expect(document.body.textContent).not.toContain("a-original.webp");
  });
});
