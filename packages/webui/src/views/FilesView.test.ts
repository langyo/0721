import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter, type Router } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The surface-level mount pins. These exist because a wave once shipped
// both surfaces referencing an unimported component: every gate was
// green, and the two most-trafficked pages rendered NOTHING (the
// unresolved component is null, which throws inside the master-detail
// slot and takes the subtree with it). A per-surface mount that asserts
// the roster actually drew is what catches that class.

const listFilesMock = vi.fn();
const listFolderMock = vi.fn();
const listTimelineMock = vi.fn();
const renameFileMock = vi.fn();
const moveFileMock = vi.fn();
vi.mock("@/api", () => ({
  listFiles: (...a: unknown[]) => listFilesMock(...a),
  deleteFile: vi.fn(),
  uploadFile: vi.fn(),
  createFolder: vi.fn(),
  renameFile: (...a: unknown[]) => renameFileMock(...a),
  moveFile: (...a: unknown[]) => moveFileMock(...a),
  listTimeline: (...a: unknown[]) => listTimelineMock(...a),
}));
vi.mock("@/api/files", () => ({
  listFolder: (...a: unknown[]) => listFolderMock(...a),
  quotaGet: vi.fn().mockResolvedValue({ used_bytes: 2048, quota_bytes: null }),
  deleteFile: vi.fn(),
  renameFile: (...a: unknown[]) => renameFileMock(...a),
  moveFile: (...a: unknown[]) => moveFileMock(...a),
  listTimeline: (...a: unknown[]) => listTimelineMock(...a),
  shareCreate: vi.fn(),
  shareList: vi.fn(),
  shareDelete: vi.fn(),
  listQuota: vi.fn(),
}));

import FilesView from "./FilesView.vue";
import PortalView from "./PortalView.vue";
import { provideActionBar } from "@/composables/useActionBar";
import { createAppI18n } from "@/i18n";
import type { FileDto } from "@/types";

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

const folder: FileDto = file("f1", { kind: "folder", mime: "", name: "docs", original_name: "docs" });

function harness(View: typeof FilesView | typeof PortalView) {
  return {
    components: { View },
    setup() {
      const bar = provideActionBar();
      return { bar };
    },
    template:
      '<div><div class="harness-actions">' +
      '<component :is="() => bar.actions.value.flatMap((r) => r())" />' +
      "</div><View /></div>",
  };
}

function mountWithRouter(View: typeof FilesView | typeof PortalView): { wrapper: ReturnType<typeof mount>; router: Router } {
  const pinia = createPinia();
  setActivePinia(pinia);
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/", component: { template: "<div/>" } },
      { path: "/files/:folderId?", component: { template: "<div class='folder-stub'/>" } },
    ],
  });
  const wrapper = mount(harness(View), { global: { plugins: [pinia, router, i18n] } });
  return { wrapper, router };
}

let wrapper: ReturnType<typeof mount> | null = null;

beforeEach(() => {
  wrapper = null;
  window.innerWidth = 1280;
  listFilesMock.mockReset().mockResolvedValue({ total: 0, offset: 0, limit: 30, items: [] });
  listFolderMock.mockReset().mockResolvedValue({ total: 0, offset: 0, limit: 30, items: [] });
  listTimelineMock.mockResolvedValue({ total: 0, offset: 0, limit: 40, items: [] });
  renameFileMock.mockReset();
  moveFileMock.mockReset();
});

afterEach(() => {
  wrapper?.unmount();
});

describe("FilesView surface (mount pin)", () => {
  it("renders the roster — the pane component must resolve", async () => {
    listFolderMock.mockResolvedValue({ total: 2, offset: 0, limit: 30, items: [folder, file("a")], crumbs: [] });
    const { wrapper: w } = mountWithRouter(FilesView);
    wrapper = w;
    await flushPromises();
    // A missing pane import nulls the slot and throws the subtree away;
    // these two assertions are what notice.
    expect(w.find(".file-grid").exists()).toBe(true);
    // The pane's own root: a missing component leaves nothing here.
    expect(w.findAll(".file-tile").length).toBe(2);
    // The roster's own formatter must handle the bigint wire value; the
    // degraded shape is "—", so a numeric size proves the coercion ran on
    // THIS surface (the pane has a formatter of its own).
    const tileSize = w.find(".tile-size");
    expect(tileSize.exists()).toBe(true);
    expect(tileSize.text()).toMatch(/[0-9.]+\s?(B|KB|KiB|MB|MiB)/);
    // The detail slot must RESOLVE, not merely exist: open the pane and
    // demand the component's own root (a closed pane never mounts it).
    await w.findAll(".tile-body")[1]!.trigger("click");
    await flushPromises();
    expect(w.find(".file-detail").exists()).toBe(true);
  });

  it("navigates for a folder and selects for a file", async () => {
    listFolderMock.mockResolvedValue({ total: 2, offset: 0, limit: 30, items: [folder, file("a")], crumbs: [] });
    const { wrapper: w, router } = mountWithRouter(FilesView);
    wrapper = w;
    await flushPromises();

    const bodies = w.findAll(".tile-body");
    await bodies[0]!.trigger("click"); // folder
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/files/f1");

    // Still on the files route tree; the file click selects instead.
    const { wrapper: w2 } = mountWithRouter(FilesView);
    await flushPromises();
    await w2.findAll(".tile-body")[1]!.trigger("click"); // file
    await flushPromises();
    // Scoped to the pane: the roster tile renders the same string, so a
    // whole-wrapper assertion cannot tell a live pane from a dead one.
    expect(w2.find(".file-detail").exists()).toBe(true);
    expect(w2.find(".file-detail").text()).toContain("a-original.webp");
    w2.unmount();
  });

  it("re-points the pane at the fresh row after a rename", async () => {
    listFolderMock.mockResolvedValueOnce({ total: 1, offset: 0, limit: 30, items: [file("a")], crumbs: [] });
    const { wrapper: w } = mountWithRouter(FilesView);
    wrapper = w;
    await flushPromises();
    await w.findAll(".tile-body")[0]!.trigger("click");
    await flushPromises();
    expect(w.find(".file-detail").text()).toContain("a-original.webp");
    // The wire type is a bigint; a truthfully-typed fixture must format
    // (the degraded shape is "—", so a numeric size here proves the
    // coercion ran).
    expect(w.find(".file-detail").text()).toMatch(/[0-9.]+\s?(B|KB|KiB|MB|MiB|GB|GiB)/);

    // Rename through the tile's own dialog: the roster reload carries the
    // new name for the same id, and the pane must show THAT object.
    const renamed = file("a", { name: "a-RENAMED.webp", original_name: "a-RENAMED.webp" });
    listFolderMock.mockResolvedValue({ total: 1, offset: 0, limit: 30, items: [renamed], crumbs: [] });
    renameFileMock.mockResolvedValue(renamed);

    const renameBtn = w.findAll(".file-tile")[0]!.findAll(".t-action").find(
      (b) => b.attributes("title") === i18n.global.t("common.files.rename"),
    );
    expect(renameBtn).toBeTruthy();
    await renameBtn!.trigger("click");
    await flushPromises();

    // Type a name and submit the dialog's primary action (teleported).
    const input = document.querySelector<HTMLInputElement>("input");
    expect(input).toBeTruthy();
    input!.value = "a-RENAMED.webp";
    input!.dispatchEvent(new Event("input", { bubbles: true }));
    await flushPromises();
    const submit = Array.from(document.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === i18n.global.t("common.actions.save"),
    );
    expect(submit).toBeTruthy();
    submit!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    await flushPromises();

    expect(renameFileMock).toHaveBeenCalled();
    expect(w.find(".file-detail").text()).toContain("a-RENAMED.webp");
    expect(w.find(".file-detail").text()).not.toContain("a-original.webp");
  });

  it("drops the pane when a reload no longer carries the row", async () => {
    listFolderMock.mockResolvedValueOnce({ total: 1, offset: 0, limit: 30, items: [file("a")], crumbs: [] });
    const { wrapper: w } = mountWithRouter(FilesView);
    wrapper = w;
    await flushPromises();
    await w.findAll(".tile-body")[0]!.trigger("click");
    await flushPromises();
    expect(w.find(".file-detail").exists()).toBe(true);

    // The row vanished server-side; the next roster no longer holds it.
    listFolderMock.mockResolvedValue({ total: 1, offset: 0, limit: 30, items: [file("b")], crumbs: [] });
    // Drive the header's own refresh action (registered by usePageRefresh)
    // rather than reaching into setup state the harness doesn't expose.
    const refresh = w.find('.harness-actions button[aria-label="' + i18n.global.t("common.actions.refresh") + '"]');
    expect(refresh.exists(), "the harness must render the refresh action").toBe(true);
    await refresh.trigger("click");
    await flushPromises();
    expect(w.find(".file-detail").exists()).toBe(false);
  });

  it("clears the pane when navigating into another folder", async () => {
    listFolderMock.mockResolvedValueOnce({ total: 2, offset: 0, limit: 30, items: [folder, file("a")], crumbs: [] });
    const { wrapper: w, router } = mountWithRouter(FilesView);
    wrapper = w;
    await flushPromises();
    await w.findAll(".tile-body")[1]!.trigger("click");
    await flushPromises();
    expect(w.find(".file-detail").exists()).toBe(true);

    listFolderMock.mockResolvedValue({ total: 0, offset: 0, limit: 30, items: [], crumbs: [] });
    await router.push("/files/f1");
    await flushPromises();
    expect(w.find(".file-detail").exists()).toBe(false);
  });

  it("clears the pane when the selected row moves out of this folder", async () => {
    listFolderMock.mockResolvedValueOnce({
      total: 2,
      offset: 0,
      limit: 30,
      items: [folder, file("a")],
      crumbs: [],
    });
    const { wrapper: w } = mountWithRouter(FilesView);
    wrapper = w;
    await flushPromises();
    // Pick the FILE (index 1 — index 0 is the folder).
    await w.findAll(".tile-body")[1]!.trigger("click");
    await flushPromises();
    expect(w.find(".file-detail").exists()).toBe(true);

    listFolderMock.mockResolvedValue({ total: 1, offset: 0, limit: 30, items: [folder], crumbs: [] });
    moveFileMock.mockResolvedValue(undefined);
    // Scope to the FILE's own tile: .t-action spans every tile, and the
    // folder's move button comes first.
    const fileTile = w.findAll(".file-tile")[1]!;
    const moveBtn = fileTile.findAll(".t-action").find((b) => b.attributes("title") === i18n.global.t("common.files.move"));
    expect(moveBtn).toBeTruthy();
    await moveBtn!.trigger("click");
    await flushPromises();

    // The move dialog picks its target from a select of folders.
    const select = document.querySelector<HTMLSelectElement>("select");
    expect(select).toBeTruthy();
    select!.value = "f1";
    select!.dispatchEvent(new Event("change", { bubbles: true }));
    await flushPromises();
    const submit = Array.from(document.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === i18n.global.t("common.actions.confirm"),
    );
    expect(submit).toBeTruthy();
    submit!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    await flushPromises();

    expect(moveFileMock).toHaveBeenCalled();
    // The pane is gone (its root unmounts with the selection).
    expect(w.find(".file-detail").exists()).toBe(false);
  });
});

describe("PortalView surface (mount pin)", () => {
  it("renders the grid — the pane component must resolve", async () => {
    listFilesMock.mockResolvedValue({ total: 2, offset: 0, limit: 30, items: [file("a"), file("b")] });
    const { wrapper: w } = mountWithRouter(PortalView);
    wrapper = w;
    await flushPromises();
    expect(w.findAll(".portal-tile").length).toBe(2);
    expect(w.find(".hk-master-detail").exists()).toBe(true);
    await w.findAll(".tile-media")[0]!.trigger("click");
    await flushPromises();
    expect(w.find(".file-detail").exists()).toBe(true);
  });

  it("opens the pane on a tile click", async () => {
    listFilesMock.mockResolvedValue({ total: 1, offset: 0, limit: 30, items: [file("a")] });
    const { wrapper: w } = mountWithRouter(PortalView);
    wrapper = w;
    await flushPromises();
    await w.find(".tile-media").trigger("click");
    await flushPromises();
    expect(w.find(".file-detail").exists()).toBe(true);
    expect(w.find(".file-detail").text()).toContain("a-original.webp");
    // The wire type is a bigint; a truthfully-typed fixture must format
    // (the degraded shape is "—", so a numeric size here proves the
    // coercion ran).
    expect(w.find(".file-detail").text()).toMatch(/[0-9.]+\s?(B|KB|KiB|MB|MiB|GB|GiB)/);
  });
});
