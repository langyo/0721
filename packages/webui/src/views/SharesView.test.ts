import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const shareListMock = vi.fn();
const shareDeleteMock = vi.fn();
vi.mock("@/api/files", () => ({
  shareList: (...args: unknown[]) => shareListMock(...args),
  shareDelete: (...args: unknown[]) => shareDeleteMock(...args),
}));

import SharesView from "./SharesView.vue";
import { provideActionBar } from "@/composables/useActionBar";
import { createAppI18n } from "@/i18n";
import type { ShareDto } from "@/types";

/**
 * The share roster's failure lanes: a revoke that fails must SAY so
 * (the row stays — the user can retry or investigate), and the success
 * note must never survive into a later failed attempt claiming a revoke
 * (MUT:revoke-note-reset — deleting the reset reddens this file's case)
 * that did not happen.
 */

const i18n = createAppI18n();

function share(token: string): ShareDto {
  return {
    token,
    file_id: "f1",
    file_name: `notes-${token}.txt`,
    file_size: 1234n,
    created_at: "2026-11-01T10:00:00Z",
    owner_id: "u1",
    allow_preview: true,
  } as ShareDto;
}

function mountView(): ReturnType<typeof mount> {
  const pinia = createPinia();
  setActivePinia(pinia);
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: "/", component: { template: "<div/>" } }],
  });
  const Harness = {
    components: { SharesView },
    setup() {
      provideActionBar();
    },
    template: "<div><SharesView /></div>",
  };
  return mount(Harness, { global: { plugins: [pinia, router, i18n] } });
}

let wrapper: ReturnType<typeof mount> | null = null;

beforeEach(() => {
  wrapper = null;
  shareListMock.mockReset().mockResolvedValue([]);
  shareDeleteMock.mockReset();
});

afterEach(() => {
  wrapper?.unmount();
});

describe("SharesView (family table roster)", () => {
  it("renders one row per share with the truncated link", async () => {
    shareListMock.mockResolvedValue([share("tok_abcdefgh1234"), share("tok_second99999")]);
    wrapper = mountView();
    await flushPromises();
    expect(wrapper.findAll("tbody tr").length).toBe(2);
    expect(wrapper.text()).toContain("notes-tok_abcdefgh1234.txt");
    expect(wrapper.text()).toContain("/s/tok_abcdefgh…");
  });

  it("surfaces a failed revoke and keeps the row", async () => {
    shareListMock.mockResolvedValue([share("tok_abcdefgh1234")]);
    shareDeleteMock.mockRejectedValue(new Error("share_delete denied"));
    wrapper = mountView();
    await flushPromises();
    const revoke = wrapper.find(`button[aria-label="${i18n.global.t("common.files.revoke")}"]`);
    expect(revoke.exists()).toBe(true);
    await revoke.trigger("click");
    await flushPromises();
    // The row survives and the failure is announced — a silent catch
    // (MUT:revoke-failure-lane — re-silencing the catch reddens it).
    // — a silent catch
    // leaves a live share on screen with no explanation.
    expect(wrapper.findAll("tbody tr").length).toBe(1);
    expect(wrapper.text()).toContain("share_delete denied");
    expect(wrapper.text()).not.toContain(i18n.global.t("common.files.revoked"));
  });

  it("does not let a stale success note claim a revoke that failed", async () => {
    shareListMock.mockResolvedValue([share("tok_abcdefgh1234"), share("tok_second99999")]);
    shareDeleteMock.mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("nope"));
    wrapper = mountView();
    await flushPromises();
    const buttons = wrapper.findAll(`button[aria-label="${i18n.global.t("common.files.revoke")}"]`);
    // First revoke succeeds: the note appears.
    await buttons[0]!.trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain(i18n.global.t("common.files.revoked"));
    // Second revoke fails: the note must be GONE, not sitting over the
    // failure.
    const after = wrapper.findAll(`button[aria-label="${i18n.global.t("common.files.revoke")}"]`);
    await after[0]!.trigger("click");
    await flushPromises();
    expect(wrapper.text()).not.toContain(i18n.global.t("common.files.revoked"));
    expect(wrapper.text()).toContain("nope");
  });

  it("carries a bare-string rejection instead of an empty banner", async () => {
    // The JSON-RPC lane's common rejection shape; the empty-message
    // case renders no alert at all.
    shareListMock.mockRejectedValue("rpc timeout");
    wrapper = mountView();
    await flushPromises();
    expect(wrapper.find(".hk-alert").exists()).toBe(true);
    expect(wrapper.text()).toContain("rpc timeout");
  });

  it("drops the row after a successful revoke reloads the roster", async () => {
    shareListMock.mockResolvedValueOnce([share("tok_abcdefgh1234"), share("tok_second99999")]);
    shareDeleteMock.mockResolvedValue(undefined);
    shareListMock.mockResolvedValueOnce([share("tok_second99999")]);
    wrapper = mountView();
    await flushPromises();
    const buttons = wrapper.findAll(`button[aria-label="${i18n.global.t("common.files.revoke")}"]`);
    await buttons[0]!.trigger("click");
    await flushPromises();
    await flushPromises();
    expect(wrapper.findAll("tbody tr").length).toBe(1);
    expect(wrapper.text()).toContain(i18n.global.t("common.files.revoked"));
  });
});