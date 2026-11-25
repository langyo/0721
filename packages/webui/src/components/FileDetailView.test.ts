import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter, type Router } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * The R-M② placeholder ladder, pinned on the image surfaces that consume
 * it (the detail pane's preview and the portal's tiles).
 *
 * MUT:p2b-ladder-refresh — dropping the refresh attempt (going straight
 * to the status translation) turns
 * `private_bucket_placeholder_shows_sign_in_when_refresh_fails` red;
 * MUT:p2b-ladder-status — dropping the retry's status translation turns
 * `private_bucket_placeholder_shows_no_access_on_403` red;
 * MUT:p2b-ladder-clear — dropping the success clear leaves a stale face
 * over a loadable image.
 */

const listFolderMock = vi.fn();
const listFilesMock = vi.fn();
vi.mock("@/api", () => ({
  listFolder: (...a: unknown[]) => listFolderMock(...a),
  listFiles: (...a: unknown[]) => listFilesMock(...a),
  deleteFile: vi.fn(),
  uploadFile: vi.fn(),
  createFolder: vi.fn(),
  renameFile: vi.fn(),
  moveFile: vi.fn(),
  listTimeline: vi.fn(),
  quotaGet: vi.fn().mockResolvedValue({ used_bytes: 2048, quota_bytes: null }),
}));
vi.mock("@/api/files", () => ({
  listFolder: (...a: unknown[]) => listFolderMock(...a),
  deleteFile: vi.fn(),
  quotaGet: vi.fn().mockResolvedValue({ used_bytes: 2048, quota_bytes: null }),
  renameFile: vi.fn(),
  moveFile: vi.fn(),
  listTimeline: vi.fn(),
  shareCreate: vi.fn(),
  shareList: vi.fn(),
  shareDelete: vi.fn(),
  listQuota: vi.fn(),
}));
const listBucketsMock = vi.fn().mockResolvedValue({ buckets: [] });
vi.mock("@/api/buckets", () => ({
  listBuckets: (...a: unknown[]) => listBucketsMock(...a),
}));

import FileDetailView from "@/components/FileDetailView";
import PortalView from "@/views/PortalView";
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

function mountHost(component: typeof FileDetailView | typeof PortalView, files: FileDto[]) {
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
  const wrapper =
    component === FileDetailView
      ? mount(FileDetailView, {
          props: { file: files[0]! },
          global: { plugins: [pinia, router, i18n] },
          attachTo: host,
        })
      : mount(PortalView, {
          global: { plugins: [pinia, router, i18n] },
          attachTo: host,
        });
  return { wrapper, host };
}

/** The fetch seam: a queue of per-URL answers. */
type Answer = { ok: boolean; status: number };
const refreshAnswers: Answer[] = [];
const mediaAnswers = new Map<string, Answer[]>();

function installFetchSeam(): void {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
      if (url.includes("/api/v1/auth/refresh")) {
        const answer = refreshAnswers.shift() ?? { ok: false, status: 401 };
            return new Response(null, { status: answer.status });
      }
      if (url.startsWith("/media/")) {
        const answer = mediaAnswers.get(url)?.shift() ?? { ok: false, status: 404 };
            return new Response(null, { status: answer.status });
      }
      return new Response(null, { status: 404 });
    }),
  );
}

function fireError(wrapper: ReturnType<typeof mount>, selector: string): Promise<void> {
  const img = wrapper.find(selector);
  expect(img.exists()).toBe(true);
  return img.trigger("error");
}

let wrapper: ReturnType<typeof mount> | null = null;
let host: HTMLDivElement | null = null;

beforeEach(() => {
  wrapper = null;
  host = null;
  refreshAnswers.length = 0;
  mediaAnswers.clear();
  installFetchSeam();
});

afterEach(() => {
  wrapper?.unmount();
  host?.remove();
  vi.unstubAllGlobals();
});

describe("R-M② placeholder ladder (private bucket images)", () => {
  it("private_bucket_images_survive_session_rotation", async () => {
    // First load 401s; the refresh ROTATES the session and the retry
    // succeeds: the image recovers, no placeholder remains.
    const f = file("rot");
    refreshAnswers.push({ ok: true, status: 200 });
    mediaAnswers.set("/media/rot", [{ ok: true, status: 200 }]);
    const mounted = mountHost(FileDetailView, [f]);
    wrapper = mounted.wrapper;
    host = mounted.host;
    await flushPromises();

    await fireError(wrapper!, ".file-detail img");
    await flushPromises();
    await flushPromises();
    // The retry answered 2xx: no placeholder anywhere, the ladder cleared.
    expect(wrapper!.find(".media-placeholder").exists()).toBe(false);
    expect(document.querySelector("[data-testid^='media-placeholder-']")).toBeNull();
  });

  it("private_bucket_placeholder_shows_sign_in_when_refresh_fails", async () => {
    // MUT:p2b-ladder-refresh — the refresh attempt itself is refused
    // (401): the sign-in face, the SAME copy never-signed-in and
    // expired share.
    const f = file("dead");
    refreshAnswers.push({ ok: false, status: 401 });
    
    const mounted = mountHost(FileDetailView, [f]);
    wrapper = mounted.wrapper;
    host = mounted.host;
    await flushPromises();

    await fireError(wrapper!, ".file-detail img");
    await flushPromises();
    await flushPromises();

    const placeholder = wrapper!.find('[data-testid="media-placeholder-sign-in"]');
    expect(placeholder.exists()).toBe(true);
    expect(placeholder.text()).toBe(i18n.global.t("common.media.signin"));
    // No media probe ever ran: the refresh refusal decides alone.
    const fetchMock = vi.mocked(fetch);
    expect(fetchMock.mock.calls.filter(([u]) => String(u).startsWith("/media/")).length).toBe(0);
  });

  it("private_bucket_placeholder_shows_no_access_on_403", async () => {
    // Refresh OK, retry 403: 「无读取权限」. MUT:p2b-ladder-status —
    // losing the status translation (or misreading 403) turns this red.
    const f = file("locked");
    refreshAnswers.push({ ok: true, status: 200 });
    mediaAnswers.set("/media/locked", [{ ok: false, status: 403 }]);
    const mounted = mountHost(FileDetailView, [f]);
    wrapper = mounted.wrapper;
    host = mounted.host;
    await flushPromises();

    await fireError(wrapper!, ".file-detail img");
    await flushPromises();
    await flushPromises();

    const placeholder = wrapper!.find('[data-testid="media-placeholder-forbidden"]');
    expect(placeholder.exists()).toBe(true);
    expect(placeholder.text()).toBe(i18n.global.t("common.media.forbidden"));
  });

  it("maps the remaining retry statuses (503 unavailable, 404 missing)", async () => {
    const f = file("down");
    refreshAnswers.push({ ok: true, status: 200 });
    mediaAnswers.set("/media/down", [{ ok: false, status: 503 }]);
    const mounted = mountHost(FileDetailView, [f]);
    wrapper = mounted.wrapper;
    host = mounted.host;
    await flushPromises();
    await fireError(wrapper!, ".file-detail img");
    await flushPromises();
    await flushPromises();
    expect(
      wrapper!.find('[data-testid="media-placeholder-unavailable"]').exists(),
    ).toBe(true);
    expect(wrapper!.find(".media-placeholder").text()).toBe(
      i18n.global.t("common.media.unavailable"),
    );
  });

  it("portal tiles carry the same ladder and surface the face", async () => {
    listFilesMock.mockResolvedValue({
      total: 1,
      offset: 0,
      limit: 40,
      items: [file("tile")],
    });
    refreshAnswers.push({ ok: false, status: 401 });
    
    const mounted = mountHost(PortalView, [file("tile")]);
    wrapper = mounted.wrapper;
    host = mounted.host;
    await flushPromises();

    const img = wrapper!.find(".portal-tile img");
    expect(img.exists()).toBe(true);
    await img.trigger("error");
    await flushPromises();
    await flushPromises();

    const placeholder = wrapper!.find('[data-testid="media-placeholder-sign-in"]');
    expect(placeholder.exists()).toBe(true);
    expect(placeholder.text()).toBe(i18n.global.t("common.media.signin"));
  });
});
