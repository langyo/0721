import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// API mocked at module scope; the reply shapes mirror the real wire
// contract (PageResult<FileDto> from the ts-rs bindings).
const listFilesMock = vi.fn();
const uploadFileMock = vi.fn();
const deleteFileMock = vi.fn();
vi.mock("@/api", () => ({
  listFiles: (...args: unknown[]) => listFilesMock(...args),
  uploadFile: (...args: unknown[]) => uploadFileMock(...args),
  deleteFile: (...args: unknown[]) => deleteFileMock(...args),
}));

import PortalView from "./PortalView";
import { HkToast, useToast } from "@celestia-island/hikari";
import { createAppI18n } from "@/i18n";

const png = (id: string, name: string) => ({
  id,
  owner_id: "0b5a0a48-8cb0-4b8c-9a4f-5d4d3a2f1c00",
  name,
  kind: "image",
  mime: "image/webp",
  size: 4096,
  sha256: id,
  original_name: name,
  url: `/media/${id}`,
  created_at: "2026-11-20T10:00:00Z",
});

function page(items: ReturnType<typeof png>[]) {
  return { total: items.length, offset: 0, limit: 30, items };
}

/** Every mounted toast outlet — cleanup owns their lifetime. */
const outlets: Array<{ unmount: () => void }> = [];

async function mountView() {
  // attachTo keeps the tree in the live document so the teleported
  // confirm dialog is reachable from `document` (a detached mount hides
  // it), and the outlet renders the toasts this view raises.
  const host = document.createElement("div");
  document.body.appendChild(host);
  const wrapper = mount(PortalView, {
    global: { plugins: [createAppI18n(), createPinia()] },
    attachTo: host,
  });
  outlets.push(mount(HkToast, { attachTo: document.body, props: { teleport: false } }));
  await flushPromises();
  return wrapper;
}

/** The toast surface text (the view raises no inline alert any more). */
function toastText(): string {
  return document.querySelector(".hk-toast-message")?.textContent ?? "";
}

beforeEach(() => {
  listFilesMock.mockReset();
  uploadFileMock.mockReset();
  deleteFileMock.mockReset();
});

afterEach(() => {
  outlets.splice(0).forEach((o) => o.unmount());
  // The toast store is module-level (hikari state) — clear it so the
  // next case's outlet renders only ITS OWN messages.
  const toast = useToast();
  for (const slot of toast.toasts.slice()) toast.remove(slot.id);
  localStorage.clear();
});

describe("PortalView failure lane (never-empty)", () => {
  it("carries a bare-string rejection instead of an empty description", async () => {
    // MUT:portal-errortext-lane — reverting this lane to the inline
    // narrowing ternary reddens here (a bare string used to yield an
    // empty description, which HkEmptyState drops entirely).
    listFilesMock.mockRejectedValue("rpc timeout");
    const wrapper = await mountView();
    expect(wrapper.text()).toContain("rpc timeout");
    wrapper.unmount();
  });

  it("falls back for an empty-message Error", async () => {
    listFilesMock.mockRejectedValue(new Error(""));
    const wrapper = await mountView();
    expect(wrapper.text()).toContain("Something went wrong");
    wrapper.unmount();
  });

  it("keeps the upload lane's own fallback wording end-to-end", async () => {
    // MUT:portal-upload-fallback-key — this lane and the settings save
    // lanes are the ones carrying non-default fallback keys; this pin is
    // the end-to-end proof THIS view passes its key (a slip back to the
    // default key would keep every factory unit test green). The save
    // lanes' pin lands with the settings UI wave (issue #31 pattern).
    listFilesMock.mockResolvedValue(page([]));
    uploadFileMock.mockRejectedValue(new Error(""));
    const wrapper = await mountView();
    const input = wrapper.find('input[type="file"]');
    expect(input.exists()).toBe(true);
    Object.defineProperty(input.element, "files", {
      value: [new File(["x"], "a.png", { type: "image/png" })],
      configurable: true,
    });
    await input.trigger("change");
    await flushPromises();
    // The failure is a TOAST; the lane row keeps only the file's name.
    await vi.waitFor(() => {
      expect(toastText()).toContain("Upload failed");
    });
    // and the fallback wording exactly once — the toast carries the
    // factory's message alone, so a double print (label + message)
    // fails here. MUT:portal-upload-dedupe — restoring the old
    // "<label> — <message>" row reddens this count.
    expect(
      [document.body.textContent ?? ""].join(" ").match(/Upload failed/g)?.length ?? 0,
    ).toBe(1);
    // Nothing was inserted beside the row.
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
    wrapper.unmount();
  });
});

describe("PortalView", () => {
  it("renders the timeline grid from the list call", async () => {
    listFilesMock.mockResolvedValue(page([png("aa", "cat.webp"), png("bb", "dog.webp")]));
    const wrapper = await mountView();
    expect(listFilesMock).toHaveBeenCalledWith(0, 30);
    const tiles = wrapper.findAll(".portal-tile");
    expect(tiles.length).toBe(2);
    // The image tile's thumbnail rides the media endpoint's ?thumbnail=true.
    const img = wrapper.find(".portal-tile img");
    expect(img.attributes("src")).toBe("/media/aa?thumbnail=true");
  });

  it("shows the empty state when the instance holds nothing", async () => {
    listFilesMock.mockResolvedValue(page([]));
    const wrapper = await mountView();
    expect(wrapper.find(".portal-tile").exists()).toBe(false);
    expect(wrapper.text()).toContain("No files yet");
  });

  it("uploads picked files through the multipart lane and refreshes", async () => {
    listFilesMock.mockResolvedValue(page([]));
    uploadFileMock.mockResolvedValue(undefined);
    const wrapper = await mountView();

    const input = wrapper.find('input[data-testid="upload-input"]');
    const file = new File(["bytes"], "photo.png", { type: "image/png" });
    Object.defineProperty(input.element, "files", { value: [file] });
    await input.trigger("change");
    await flushPromises();

    expect(uploadFileMock).toHaveBeenCalledTimes(1);
    expect(uploadFileMock.mock.calls[0]![0]).toBeInstanceOf(File);
    // A finished upload clears the queue and reloads the first page.
    expect(listFilesMock).toHaveBeenLastCalledWith(0, 30);
    expect(wrapper.findAll(".upload-row").length).toBe(0);
  });

  it("toasts a failed upload once and leaves its row error-free", async () => {
    listFilesMock.mockResolvedValue(page([]));
    uploadFileMock.mockRejectedValue(new Error("undecodable image: not an image"));
    const wrapper = await mountView();

    const input = wrapper.find('input[data-testid="upload-input"]');
    const file = new File(["junk"], "broken.png", { type: "image/png" });
    Object.defineProperty(input.element, "files", { value: [file] });
    await input.trigger("change");
    await flushPromises();

    // The row stays in the lane (the user can read which file failed)…
    expect(wrapper.findAll(".upload-row").length).toBe(1);
    expect(wrapper.find(".upload-name").text()).toBe("broken.png");
    // …but carries no error of its own: the message is a toast, raised
    // once (MUT:portal-upload-toast — a leak into the row reddens below).
    await vi.waitFor(() => {
      expect(toastText()).toBe("undecodable image: not an image");
    });
    expect(document.querySelectorAll(".hk-toast-message").length).toBe(1);
    expect(wrapper.find(".upload-row").text()).not.toContain("undecodable image");
    expect(wrapper.find(".upload-error").exists()).toBe(false);
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
  });

  it("removes the tile after a confirmed delete", async () => {
    listFilesMock.mockResolvedValue(page([png("aa", "cat.webp")]));
    deleteFileMock.mockResolvedValue(undefined);
    const wrapper = await mountView();

    await wrapper.find(".tile-action.danger").trigger("click");
    await flushPromises();
    // The confirm dialog teleports to <body>, so the footer's danger
    // action is located in the document, not under the view root.
    const buttons = Array.from(document.body.querySelectorAll("button"));
    const danger = buttons.filter((b) => (b.textContent ?? "").trim() === "Delete");
    expect(danger.length).toBeGreaterThan(0);
    danger[danger.length - 1]!.click();
    await flushPromises();

    expect(deleteFileMock).toHaveBeenCalledWith("aa");
    expect(wrapper.findAll(".portal-tile").length).toBe(0);
  });

  it("toasts a failed delete instead of growing a note in the dialog", async () => {
    // MUT:portal-delete-toast — the old dialog paragraph is gone; a
    // harness that reverts to an inline note reddens the negative
    // assertion below.
    listFilesMock.mockResolvedValue(page([png("aa", "cat.webp")]));
    deleteFileMock.mockRejectedValue(new Error("file is gone offline"));
    const wrapper = await mountView();

    await wrapper.find(".tile-action.danger").trigger("click");
    await flushPromises();
    const buttons = Array.from(document.body.querySelectorAll("button"));
    const danger = buttons.filter((b) => (b.textContent ?? "").trim() === "Delete");
    expect(danger.length).toBeGreaterThan(0);
    danger[danger.length - 1]!.click();
    await flushPromises();

    await vi.waitFor(() => {
      expect(toastText()).toBe("file is gone offline");
    });
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
    // The row survives a failed delete.
    expect(wrapper.findAll(".portal-tile").length).toBe(1);
  });
});
