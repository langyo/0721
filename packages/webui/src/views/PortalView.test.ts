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

import PortalView from "./PortalView.vue";
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

async function mountView() {
  const wrapper = mount(PortalView, {
    global: { plugins: [createAppI18n(), createPinia()] },
  });
  await flushPromises();
  return wrapper;
}

beforeEach(() => {
  listFilesMock.mockReset();
  uploadFileMock.mockReset();
  deleteFileMock.mockReset();
});

afterEach(() => {
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

  it("keeps a failed upload in the lane with its error", async () => {
    listFilesMock.mockResolvedValue(page([]));
    uploadFileMock.mockRejectedValue(new Error("undecodable image: not an image"));
    const wrapper = await mountView();

    const input = wrapper.find('input[data-testid="upload-input"]');
    const file = new File(["junk"], "broken.png", { type: "image/png" });
    Object.defineProperty(input.element, "files", { value: [file] });
    await input.trigger("change");
    await flushPromises();

    expect(wrapper.findAll(".upload-row").length).toBe(1);
    expect(wrapper.find(".upload-error").text()).toContain("undecodable image");
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
});
