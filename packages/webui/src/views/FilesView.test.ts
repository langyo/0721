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
//
// MUT:surface-mount-pin — deleting the pane component's import reddens
// these cases (the slot resolves to null and the pane root never exists).

const listFilesMock = vi.fn();
const deleteFileMock = vi.fn();
const listFolderMock = vi.fn();
const listTimelineMock = vi.fn();
const renameFileMock = vi.fn();
const moveFileMock = vi.fn();
vi.mock("@/api", () => ({
  listFiles: (...a: unknown[]) => listFilesMock(...a),
  deleteFile: (...a: unknown[]) => deleteFileMock(...a),
  uploadFile: vi.fn(),
  createFolder: vi.fn(),
  renameFile: (...a: unknown[]) => renameFileMock(...a),
  moveFile: (...a: unknown[]) => moveFileMock(...a),
  listTimeline: (...a: unknown[]) => listTimelineMock(...a),
}));
const listBucketsMock = vi.fn().mockResolvedValue({ buckets: [] });
vi.mock("@/api/buckets", () => ({
  listBuckets: (...a: unknown[]) => listBucketsMock(...a),
  readableBuckets: (rows: unknown[]) => rows,
}));
vi.mock("@/api/files", () => ({
  listFolder: (...a: unknown[]) => listFolderMock(...a),
  deleteFile: (...a: unknown[]) => deleteFileMock(...a),
  quotaGet: vi.fn().mockResolvedValue({ used_bytes: 2048, quota_bytes: null }),
  renameFile: (...a: unknown[]) => renameFileMock(...a),
  moveFile: (...a: unknown[]) => moveFileMock(...a),
  listTimeline: (...a: unknown[]) => listTimelineMock(...a),
  shareCreate: vi.fn(),
  shareList: vi.fn(),
  shareDelete: vi.fn(),
  listQuota: vi.fn(),
}));

import FilesView from "./FilesView";
import PortalView from "./PortalView";
import { HkToast, useToast } from "@celestia-island/hikari";
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
    size: 2048,
    sha256: id,
    original_name: `${id}-original.webp`,
    url: `/media/${id}`,
    created_at: "2026-11-01T10:00:00Z",
    ...over,
  };
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
  const host = document.createElement("div");
  document.body.appendChild(host);
  const wrapper = mount(harness(View), { global: { plugins: [pinia, router, i18n] }, attachTo: host });
  // The family toast outlet renders the toasts these views raise; the
  // inline `role="alert"` paragraph is GONE (the user's hard rule), so the
  // assertions below read the toast surface, not DOM inserted beside the
  // roster.
  outlets.push(mount(HkToast, { attachTo: document.body, props: { teleport: false } }));
  return { wrapper, router };
}

/** Every mounted toast outlet — cleanup owns their lifetime (the views'
 *  own wrappers are returned to the callers; outlets are harness state). */
const outlets: Array<{ unmount: () => void }> = [];

/** The toast surface's text (empty when no toast carries a message). */
function toastText(): string {
  return document.querySelector(".hk-toast-message")?.textContent ?? "";
}

let wrapper: ReturnType<typeof mount> | null = null;

beforeEach(() => {
  wrapper = null;
  window.innerWidth = 1280;
  listFilesMock.mockReset().mockResolvedValue({ total: 0, offset: 0, limit: 30, items: [] });
  listFolderMock.mockReset().mockResolvedValue({ total: 0, offset: 0, limit: 30, items: [] });
  listTimelineMock.mockResolvedValue({ total: 0, offset: 0, limit: 40, items: [] });
  renameFileMock.mockReset();
  deleteFileMock.mockReset();
  moveFileMock.mockReset();
});

afterEach(() => {
  wrapper?.unmount();
  outlets.splice(0).forEach((o) => o.unmount());
  // The toast store is module-level (hikari state) — clear it so the next
  // case's outlet renders only ITS OWN messages.
  const toast = useToast();
  for (const slot of toast.toasts.slice()) toast.remove(slot.id);
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
    // The roster's own formatter (the pane has one of its own) must render
    // a real size — the degraded shape for a non-finite value is "—".
    const tileSize = w.find(".tile-size");
    expect(tileSize.exists()).toBe(true);
    expect(tileSize.text()).toMatch(/[0-9.]+\s?(B|KB|KiB|MB|MiB)/);
    // The detail slot must RESOLVE, not merely exist: open the pane and
    // demand the component's own root (a closed pane never mounts it).
    // MUT:surface-mount-pin — removing the import leaves this assertion
    // with no `.file-detail` to find.
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
    // A real size must format (a non-finite value degrades to "—").
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

  it("offers every folder EXCEPT the one being moved as a destination", async () => {
    const target = file("fdA", { kind: "folder", mime: "", name: "alpha", original_name: "alpha" });
    const other = file("fdB", { kind: "folder", mime: "", name: "beta", original_name: "beta" });
    listFolderMock.mockResolvedValueOnce({
      total: 3,
      offset: 0,
      limit: 30,
      items: [target, other, file("a")],
      crumbs: [],
    });
    const { wrapper: w } = mountWithRouter(FilesView);
    wrapper = w;
    await flushPromises();

    // The folder's OWN move action — clicking its tile body would navigate
    // into it instead.
    const moveBtn = w.findAll(".file-tile")[0]!.findAll(".t-action").find(
      (b) => b.attributes("title") === i18n.global.t("common.files.move"),
    );
    expect(moveBtn).toBeTruthy();
    await moveBtn!.trigger("click");
    await flushPromises();

    const trigger = document.querySelector<HTMLButtonElement>(".hk-select-trigger");
    expect(trigger).toBeTruthy();
    trigger!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();

    const optionTexts = Array.from(
      document.querySelectorAll<HTMLElement>("[role='option'], .hk-select-option"),
    ).map((n) => n.textContent?.trim());
    // The moving folder is not its own destination (the server rejects a
    // folder moved into its own subtree); its sibling still is.
    expect(optionTexts).not.toContain("alpha");
    expect(optionTexts).toContain("beta");
    // Only folders are destinations: the plain file in the roster is not
    // offered.
    expect(optionTexts).not.toContain("a.webp");
  });

  it("moves to the ROOT when the root entry is picked", async () => {
    // The view normalises the picker's "" to null for the API; nothing
    // used to exercise that path.
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
    moveFileMock.mockResolvedValue(undefined);
    listFolderMock.mockResolvedValue({ total: 1, offset: 0, limit: 30, items: [folder], crumbs: [] });

    const moveBtn = w.findAll(".file-tile")[1]!.findAll(".t-action").find(
      (b) => b.attributes("title") === i18n.global.t("common.files.move"),
    );
    await moveBtn!.trigger("click");
    await flushPromises();
    // Leave the default (root) selection untouched and confirm.
    const submit = Array.from(document.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === i18n.global.t("common.actions.confirm"),
    );
    expect(submit).toBeTruthy();
    submit!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    expect(moveFileMock).toHaveBeenCalledWith("a", null);
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
    // MUT:header-refresh-lane — reverting the roster to a direct load()
    // call reddens the reload pins above.
    const refresh = w.find('.harness-actions button[aria-label="' + i18n.global.t("common.actions.refresh") + '"]');
    expect(refresh.exists(), "the harness must render the refresh action").toBe(true);
    await refresh.trigger("click");
    await flushPromises();
    expect(w.find(".file-detail").exists()).toBe(false);
  });

  it("clears the pane the moment navigation starts, before the roster resolves", async () => {
    listFolderMock.mockResolvedValueOnce({ total: 2, offset: 0, limit: 30, items: [folder, file("a")], crumbs: [] });
    const { wrapper: w, router } = mountWithRouter(FilesView);
    wrapper = w;
    await flushPromises();
    await w.findAll(".tile-body")[1]!.trigger("click");
    await flushPromises();
    expect(w.find(".file-detail").exists()).toBe(true);

    // Hold the destination roster PENDING so the pane cannot disappear by
    // roster-emptiness — the guard itself has to have cleared it. (The
    // previous version of this pin used an empty destination, which
    // unmounts HkMasterDetail wholesale and left the guard unproven.)
    let release: (v: unknown) => void = () => undefined;
    listFolderMock.mockReturnValue(new Promise((resolve) => { release = resolve; }));
    await router.push("/files/f1");
    await flushPromises();
    expect(w.find(".file-detail").exists()).toBe(false);

    release({ total: 1, offset: 0, limit: 30, items: [folder], crumbs: [] });
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

    // Hold the post-move roster PENDING: the reload-drop guard cannot
    // have fired yet, so the pane can only be gone because the move guard
    // cleared it. (The old shape was satisfied by reload-drop instead —
    // deleting the move guard left it green.)
    let release: (v: unknown) => void = () => undefined;
    listFolderMock.mockReturnValue(new Promise((resolve) => { release = resolve; }));
    moveFileMock.mockResolvedValue(undefined);
    // Scope to the FILE's own tile: .t-action spans every tile, and the
    // folder's move button comes first.
    const fileTile = w.findAll(".file-tile")[1]!;
    const moveBtn = fileTile.findAll(".t-action").find((b) => b.attributes("title") === i18n.global.t("common.files.move"));
    expect(moveBtn).toBeTruthy();
    await moveBtn!.trigger("click");
    await flushPromises();

    // The move dialog picks its target through the family select: open
    // the trigger, then pick the folder's row in the panel.
    const trigger = document.querySelector<HTMLButtonElement>(".hk-select-trigger");
    expect(trigger).toBeTruthy();
    trigger!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    const option = Array.from(document.querySelectorAll<HTMLElement>("[role='option'], .hk-select-option")).find(
      (n) => n.textContent?.trim() === "docs",
    );

    expect(option, `options: ${document.body.textContent?.slice(0, 120)}`).toBeTruthy();
    option!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    const submit = Array.from(document.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === i18n.global.t("common.actions.confirm"),
    );
    expect(submit).toBeTruthy();
    submit!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    await flushPromises();

    // The destination is the pivot of this wave (native select → family
    // panel): a view that never writes the picked value back would move
    // the file to the ROOT instead of the chosen folder.
    expect(moveFileMock).toHaveBeenCalledWith("a", "f1");
    // The pane is gone (its root unmounts with the selection).
    expect(w.find(".file-detail").exists()).toBe(false);
    release({ total: 1, offset: 0, limit: 30, items: [folder], crumbs: [] });
    await flushPromises();
    expect(w.find(".file-detail").exists()).toBe(false);
  });
});

describe("PortalView pane guards", () => {

  it("moves to the ROOT when the root entry is picked", async () => {
    // The view normalises the picker's "" to null for the API; nothing
    // used to exercise that path.
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
    moveFileMock.mockResolvedValue(undefined);
    listFolderMock.mockResolvedValue({ total: 1, offset: 0, limit: 30, items: [folder], crumbs: [] });

    const moveBtn = w.findAll(".file-tile")[1]!.findAll(".t-action").find(
      (b) => b.attributes("title") === i18n.global.t("common.files.move"),
    );
    await moveBtn!.trigger("click");
    await flushPromises();
    // Leave the default (root) selection untouched and confirm.
    const submit = Array.from(document.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === i18n.global.t("common.actions.confirm"),
    );
    expect(submit).toBeTruthy();
    submit!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
    expect(moveFileMock).toHaveBeenCalledWith("a", null);
  });

  it("drops the pane when a reload no longer carries the row", async () => {
    listFilesMock.mockResolvedValueOnce({ total: 1, offset: 0, limit: 30, items: [file("a")] });
    const { wrapper: w } = mountWithRouter(PortalView);
    wrapper = w;
    await flushPromises();
    await w.find(".tile-media").trigger("click");
    await flushPromises();
    expect(w.find(".file-detail").exists()).toBe(true);

    listFilesMock.mockResolvedValue({ total: 1, offset: 0, limit: 30, items: [file("b")] });
    const host = w.element as HTMLElement;
    const refresh = Array.from(host.querySelectorAll("button")).find(
      (b) => b.getAttribute("aria-label") === i18n.global.t("common.actions.refresh"),
    );
    expect(refresh, "the harness renders the refresh action").toBeTruthy();
    refresh!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();
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
    // A real size must format (a non-finite value degrades to "—").
    expect(w.find(".file-detail").text()).toMatch(/[0-9.]+\s?(B|KB|KiB|MB|MiB|GB|GiB)/);
  });
});

describe("FilesView failure feedback (toast, never inline)", () => {
  // The roster used to grow a `<p class="error" role="alert">` beside the
  // grid on every failed load / create / rename / move / share / delete.
  // The family feedback contract moved all of them to the toast outlet:
  // the case below pins BOTH halves — the message reaches the toast, and
  // nothing inserts an alert paragraph into the page.

  it("toasts a failed roster load and inserts no page notice", async () => {
    listFolderMock.mockReset().mockRejectedValue(new Error("rpc down"));
    const { wrapper: w } = mountWithRouter(FilesView);
    wrapper = w;
    await flushPromises();

    await vi.waitFor(() => {
      expect(toastText()).toBe("rpc down");
    });
    // The page grew no notice: no alert paragraph, no `.error` block.
    expect(w.find('[role="alert"]').exists()).toBe(false);
    expect(w.find(".error").exists()).toBe(false);
  });

  it("toasts a failed rename and leaves the dialog's text intact", async () => {
    listFolderMock.mockResolvedValue({ total: 1, offset: 0, limit: 30, items: [file("a")], crumbs: [] });
    renameFileMock.mockReset().mockRejectedValue(new Error("name taken"));
    const { wrapper: w } = mountWithRouter(FilesView);
    wrapper = w;
    await flushPromises();

    const renameBtn = w.findAll(".file-tile")[0]!.findAll(".t-action").find(
      (b) => b.attributes("title") === i18n.global.t("common.files.rename"),
    );
    await renameBtn!.trigger("click");
    await flushPromises();

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

    await vi.waitFor(() => {
      expect(toastText()).toBe("name taken");
    });
    expect(w.find('[role="alert"]').exists()).toBe(false);
    expect(w.find(".error").exists()).toBe(false);
  });
});

describe("FilesView per-bucket root (P2a)", () => {
  // The root selector renders from the visibility-scoped registry list;
  // the chosen slug's wire contract is pinned by src/api/files.test.ts.
  // MUT:B-P2A-ROOT-PARAM — dropping the loader (or the select) reddens
  // the presence pins here.
  beforeEach(() => {
    listBucketsMock.mockReset().mockResolvedValue({
      buckets: [
        { slug: "alpha", my_access: { read: true, write: true, share: true } },
        { slug: "private", my_access: { read: false, write: true, share: false } },
      ],
    });
  });

  it("renders the root selector fed by the registry and marks the unreadable bucket", async () => {
    const { wrapper: w } = mountWithRouter(FilesView);
    wrapper = w;
    await flushPromises();
    expect(w.find('[data-testid="bucket-root-select"]').exists()).toBe(true);
    expect(listBucketsMock).toHaveBeenCalledTimes(1);
    // The unreadable bucket's mark lives in the OPENED panel (§7's
    // files.list row: visible-without-read stays listed, marked).
    await w.find('[data-testid="bucket-root-select"] .hk-select-trigger').trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("no read permission");
    w.unmount();
  });
});
