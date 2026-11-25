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
const listBucketsMock = vi.fn();
vi.mock("@/api/buckets", () => ({
  listBuckets: (...args: unknown[]) => listBucketsMock(...args),
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
  // confirm dialog is reachable from `document`, and the outlet renders
  // the toasts this view raises (the inline notice lanes are GONE — the
  // user's hard rule).
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

/** The messages on the toast surface (the view raises no inline alert now). */
function toastTexts(): string[] {
  return [...document.querySelectorAll(".hk-toast-message")].map((el) => el.textContent ?? "");
}

beforeEach(() => {
  listFilesMock.mockReset();
  uploadFileMock.mockReset();
  deleteFileMock.mockReset();
});

afterEach(() => {
  outlets.splice(0).forEach((o) => o.unmount());
  // The toast store is module-level (hikari state) — clear it so the next
  // case's outlet renders only ITS OWN messages.
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
    // The failure is a TOAST now; the lane row keeps only the file's name.
    await vi.waitFor(() => {
      expect(toastTexts().join("|")).toContain("Upload failed");
    });
    // and the fallback wording exactly once across the whole document —
    // the toast carries the factory's message alone, so a double print
    // (label + message in two places) fails here. MUT:portal-upload-dedupe
    // — restoring the old "<label> — <message>" row reddens this count.
    expect((document.body.textContent ?? "").match(/Upload failed/g)?.length ?? 0).toBe(1);
    // Nothing was inserted beside the row: no page-level alert lane.
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

  it("drops a failed upload's row and names the file in the toast", async () => {
    listFilesMock.mockResolvedValue(page([]));
    uploadFileMock.mockRejectedValue(new Error("undecodable image: not an image"));
    const wrapper = await mountView();

    const input = wrapper.find('input[data-testid="upload-input"]');
    const file = new File(["junk"], "broken.png", { type: "image/png" });
    Object.defineProperty(input.element, "files", { value: [file] });
    await input.trigger("change");
    await flushPromises();

    // The row is GONE: a frozen ring with no message is a dead end the
    // visitor cannot dismiss, so a failed upload leaves nothing behind in
    // the lane. MUT:portal-upload-row-drop — keeping the row reddens this.
    expect(wrapper.findAll(".upload-row").length).toBe(0);
    // The toast is the whole report, and it NAMES the file — the only
    // place that association survives the toast's auto-dismiss.
    // MUT:portal-upload-name — dropping the `${file.name}: ` prefix
    // reddens this.
    await vi.waitFor(() => {
      expect(toastTexts()).toEqual(["broken.png: undecodable image: not an image"]);
    });
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
    // assertion below (and the toast would be empty).
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
      expect(toastTexts()).toEqual(["file is gone offline"]);
    });
    // The dialog's body carries the confirmation sentence and NOTHING
    // else — no error paragraph grew inside it ([role="alert"] itself now
    // belongs to the toast item, so the absence is asserted where the
    // removed block lived, scoped to that region).
    expect(wrapper.find('[role="alert"]').exists()).toBe(false);
    expect(document.querySelector(".hk-modal-content [role='alert']")).toBeNull();
    expect(document.querySelector(".hk-modal-content p")?.textContent).toBe(
      "Delete this file? Its public link stops working.",
    );
    // The row survives a failed delete.
    expect(wrapper.findAll(".portal-tile").length).toBe(1);
  });
});

describe("PortalView upload bucket dropdown (P2a / R-I)", () => {
  // The dropdown feeds from the visibility-scoped registry list; the
  // picked slug rides the upload call as its third argument (the api
  // layer turns it into `?bucket=`). MUT:B-P2A-UPLOAD-PARAM — dropping
  // the picked slug from the upload call reddens this.
  beforeEach(() => {
    listFilesMock.mockReset().mockResolvedValue({ total: 0, offset: 0, limit: 30, items: [] });
    uploadFileMock.mockReset().mockResolvedValue(undefined);
    deleteFileMock.mockReset();
    listBucketsMock.mockReset().mockResolvedValue({
      buckets: [
        { slug: "media", my_access: { read: true, write: true, share: true } },
        { slug: "archive", my_access: { read: true, write: true, share: true } },
      ],
    });
  });

  it("renders the picker when readable buckets exist", async () => {
    const wrapper = await mountView();
    expect(wrapper.find('[data-testid="upload-bucket-select"]').exists()).toBe(true);
    wrapper.unmount();
  });

  it("sends the picked slug with the upload", async () => {
    const wrapper = await mountView();
    // Pick "archive" by driving the picker's select.
    const select = wrapper.find('[data-testid="upload-bucket-select"]');
    await select.find(".hk-select-trigger").trigger("click");
    await flushPromises();
    // The popup layer carries per-instance state; a second open attempt
    // is a no-op when already open but recovers a swallowed first click.
    if (select.find(".hk-select-trigger").attributes("data-state") !== "open") {
      await select.find(".hk-select-trigger").trigger("click");
      await flushPromises();
    }

    // The panel TELEPORTS to <body> and a previous case's leftover panel
    // is not removed by wrapper.unmount — take the LAST match (the live
    // panel), not a stale leftover.
    const option = [...document.querySelectorAll(".hk-select-option")]
      .filter((el) => el.textContent?.trim() === "archive")
      .at(-1) as HTMLElement | undefined;
    expect(option).toBeDefined();

    option!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flushPromises();

    const input = wrapper.find('[data-testid="upload-input"]').element as HTMLInputElement;
    const file = new File(["x"], "a.png");
    Object.defineProperty(input, "files", { value: [file], configurable: true });
    input.dispatchEvent(new Event("change"));
    await flushPromises();

    // An earlier case's upload rode the same mock — assert THIS case's
    // call (the last one).

    const [, , bucket] = uploadFileMock.mock.calls.at(-1) as [File, unknown, string | undefined];
    expect(bucket).toBe("archive");
    wrapper.unmount();
  });
});
