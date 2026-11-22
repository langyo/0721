import { defineComponent, h, ref } from "vue";
import { enableAutoUnmount, flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";

import {
  orderActionRegistrations,
  PageActionSlot,
  provideActionBar,
  useActionBar,
  type ActionBarRegistration,
} from "@/composables/useActionBar";

enableAutoUnmount(afterEach);

/**
 * The unified action bar's registration contract, exercised through real
 * mounts (no mock of the module under test):
 * - a descendant's setActions/setCenter render into the layout's slots;
 * - the registration is per-component-scope — unmounting the registering
 *   view clears BOTH slots so a stale page's refresh button or filter
 *   group can never linger under the next page (the property the whole
 *   header-chrome design leans on).
 */

function makeView(chrome: { actions?: string; center?: string; hud?: string }) {
  return defineComponent({
    name: "ChromeView",
    setup() {
      const bar = useActionBar();
      if (chrome.actions) {
        bar.setActions(() => [h("button", { class: "reg-action" }, chrome.actions!)], "custom");
      }
      if (chrome.center) {
        bar.setCenter(() => [h("nav", { class: "reg-center" }, chrome.center!)]);
      }
      if (chrome.hud) {
        bar.setHud(() => [h("span", { class: "reg-hud" }, chrome.hud!)]);
      }
      return () => h("div", { class: "view-body" }, "view");
    },
  });
}

function makeMarkedView(tag: string) {
  return defineComponent({
    name: `ChromeView-${tag}`,
    setup() {
      const bar = useActionBar();
      bar.setActions(() => [h("button", { class: `reg-action reg-${tag}` }, tag)], "custom");
      return () => h("div", { class: `view-body view-${tag}` }, "view");
    },
  });
}

async function mountLayout(view: ReturnType<typeof makeView>) {
  const Layout = defineComponent({
    name: "Layout",
    setup() {
      const bar = provideActionBar();
      return () => [
        h("div", { class: "layout-actions" }, bar.actions.value.flatMap((r) => r())),
        h(view),
        h("div", { class: "layout-center" }, bar.center.value?.() ?? []),
        h("div", { class: "layout-hud" }, bar.hud.value.flatMap((r) => r())),
      ];
    },
  });
  const wrapper = mount(Layout);
  await flushPromises();
  return wrapper;
}

describe("useActionBar", () => {
  it("renders a descendant's registered actions and centre group", async () => {
    const wrapper = await mountLayout(makeView({ actions: "Refresh", center: "Filter" }));
    expect(wrapper.find(".layout-actions .reg-action").text()).toBe("Refresh");
    expect(wrapper.find(".layout-center .reg-center").text()).toBe("Filter");
    // The view body renders too — the probes mirror, never replace.
    expect(wrapper.find(".view-body").exists()).toBe(true);
  });

  it("leaves both slots empty when the view registers nothing", async () => {
    const wrapper = await mountLayout(makeView({}));
    expect(wrapper.find(".layout-actions .reg-action").exists()).toBe(false);
    expect(wrapper.find(".layout-center .reg-center").exists()).toBe(false);
  });

  it("clears the registration when the registering view unmounts", async () => {
    const show = ref(true);
    // Built ONCE outside the render fn: a component created per render
    // would remount (and re-register) on every layout update — an
    // infinite register/update loop, not the contract under test.
    const view = makeView({ actions: "Refresh", center: "Filter" });
    const Layout = defineComponent({
      name: "Layout",
      setup() {
        const bar = provideActionBar();
        return () => [
          h("div", { class: "layout-actions" }, bar.actions.value.flatMap((r) => r())),
          show.value ? h(view) : null,
          h("div", { class: "layout-center" }, bar.center.value?.() ?? []),
        ];
      },
    });
    const wrapper = mount(Layout);
    await flushPromises();
    expect(wrapper.find(".reg-action").exists()).toBe(true);
    expect(wrapper.find(".reg-center").exists()).toBe(true);

    // The page switches: the view unmounts and its chrome must go with
    // it — both slots, not just the actions.
    show.value = false;
    await flushPromises();
    expect(wrapper.find(".reg-action").exists()).toBe(false);
    expect(wrapper.find(".reg-center").exists()).toBe(false);
  });

  it("renders one page's several affordances in registration order (filter left of refresh)", async () => {
    // The 2026-09-29 composition contract: usePageFilter + usePageRefresh
    // on one page both land in the slot, filter FIRST (left of refresh).
    const view = defineComponent({
      name: "DualView",
      setup() {
        const bar = useActionBar();
        bar.setActions(() => [h("button", { class: "reg-action reg-filter" }, "filter")], PageActionSlot.Filter);
        bar.setActions(() => [h("button", { class: "reg-action reg-refresh" }, "refresh")], PageActionSlot.Refresh);
        return () => h("div", { class: "view-body" }, "view");
      },
    });
    const wrapper = await mountLayout(view);
    const texts = wrapper.findAll(".layout-actions .reg-action").map((b) => b.text());
    expect(texts).toEqual(["filter", "refresh"]);
  });

  it("is a silent no-op outside any layout provider", async () => {
    // Views render in tests (and anywhere without a layout) without a
    // provider: registering must neither throw nor render anything.
    const Bare = makeView({ actions: "Refresh", center: "Filter", hud: "62 FPS" });
    const wrapper = mount(Bare);
    await flushPromises();
    expect(wrapper.find(".reg-action").exists()).toBe(false);
    expect(wrapper.find(".reg-center").exists()).toBe(false);
    expect(wrapper.find(".reg-hud").exists()).toBe(false);
    expect(wrapper.find(".view-body").exists()).toBe(true);
  });

  // ── The hud slot (2026-10-02): the header-left readout cluster ──────
  // Same contract as actions — additive, per-scope, render-fn based — so
  // the 3D twin's FPS tag is owned by the scene that produces the number
  // instead of the layout hardcoding it.

  it("renders a descendant's registered hud readout", async () => {
    const wrapper = await mountLayout(makeView({ hud: "62 FPS" }));
    expect(wrapper.find(".layout-hud .reg-hud").text()).toBe("62 FPS");
  });

  it("keeps hud registration when only the hud was registered", async () => {
    const wrapper = await mountLayout(makeView({}));
    expect(wrapper.find(".layout-hud .reg-hud").exists()).toBe(false);
  });

  it("clears the hud registration when the registering view unmounts", async () => {
    const show = ref(true);
    const view = makeView({ hud: "62 FPS" });
    const Layout = defineComponent({
      name: "Layout",
      setup() {
        const bar = provideActionBar();
        return () => [
          h("div", { class: "layout-hud" }, bar.hud.value.flatMap((r) => r())),
          show.value ? h(view) : null,
        ];
      },
    });
    const wrapper = mount(Layout);
    await flushPromises();
    expect(wrapper.find(".layout-hud .reg-hud").exists()).toBe(true);

    show.value = false;
    await flushPromises();
    expect(wrapper.find(".layout-hud .reg-hud").exists()).toBe(false);
  });

  it("renders several hud readouts in registration order (additive)", async () => {
    const view = defineComponent({
      name: "DualHudView",
      setup() {
        const bar = useActionBar();
        bar.setHud(() => [h("span", { class: "reg-hud reg-hud-a" }, "62 FPS")]);
        bar.setHud(() => [h("span", { class: "reg-hud reg-hud-b" }, "temp 41°C")]);
        return () => h("div", { class: "view-body" }, "view");
      },
    });
    const wrapper = await mountLayout(view);
    const texts = wrapper.findAll(".layout-hud .reg-hud").map((s) => s.text());
    expect(texts).toEqual(["62 FPS", "temp 41°C"]);
  });

  it("keeps the SURVIVING registration when an overlapped view unmounts (identity guard)", async () => {
    // The HkCrossfade property the whole header design leans on: during a
    // page transition the entering and leaving views are alive at the
    // same instant, so the entering view registers OVER the leaving one
    // and then the leaving one disposes. The per-scope clear must fire
    // only when ITS OWN renderer is still the current one — an
    // unconditional clear would wipe the entering page's chrome the
    // moment the leaving view unmounts (R1 mutation d proved the
    // existing single-view clear test does NOT pin this).
    const showA = ref(true);
    const showB = ref(true);
    const viewA = makeMarkedView("a");
    const viewB = makeMarkedView("b");
    const Layout = defineComponent({
      name: "Layout",
      setup() {
        const bar = provideActionBar();
        return () => [
          h("div", { class: "layout-actions" }, bar.actions.value.flatMap((r) => r())),
          showA.value ? h(viewA) : null,
          showB.value ? h(viewB) : null,
        ];
      },
    });
    const wrapper = mount(Layout);
    await flushPromises();
    // Both alive (the crossfade instant): registrations are ADDITIVE —
    // both pages' buttons render. Both declare the CUSTOM slot, so the
    // stuffing rule orders them right-to-left: first-registered (a) is
    // RIGHTMOST → visual order [b, a] (2026-10-11 rule).
    expect(wrapper.findAll(".layout-actions .reg-action").length).toBe(2);
    const order = wrapper.findAll(".layout-actions .reg-action").map((b) => b.text());
    expect(order).toEqual(["b", "a"]);

    // The LEAVING view (a — registered first) unmounts after b took
    // over: b's chrome must survive a's disposal (the identity guard —
    // an unconditional clear here wipes the entering page's header).
    showA.value = false;
    await flushPromises();
    expect(wrapper.find(".layout-actions .reg-b").exists()).toBe(true);
    expect(wrapper.findAll(".layout-actions .reg-action").length).toBe(1);

    // Then b leaves too: only now does the slot go empty.
    showB.value = false;
    await flushPromises();
    expect(wrapper.find(".layout-actions .reg-action").exists()).toBe(false);
  });
});

describe("orderActionRegistrations (slot ranks + right-to-left customs)", () => {
  function reg(slot: string, seq: number): ActionBarRegistration {
    return { renderer: () => [], slot, seq };
  }

  it("orders the canonical slots by rank regardless of registration order", () => {
    const out = orderActionRegistrations([
      reg("refresh", 0),
      reg("custom-x", 1),
      reg("filter", 2),
      reg("create", 3),
    ]);
    expect(out.map((r) => r.slot)).toEqual(["create", "filter", "refresh", "custom-x"]);
  });

  it("ranks the window slot right of the refresh, left of customs (2026-11-04)", () => {
    // The secondary-window opener (usePageWindowAction) is canonical:
    // [create][filter][search][refresh][window] — regardless of the
    // order the page registered them in.
    const out = orderActionRegistrations([
      reg("window", 0),
      reg("refresh", 1),
      reg("search", 2),
      reg("custom-x", 3),
      reg("filter", 4),
      reg("create", 5),
    ]);
    expect(out.map((r) => r.slot)).toEqual([
      "create",
      "filter",
      "search",
      "refresh",
      "window",
      "custom-x",
    ]);
  });

  it("stuffs customs right-to-left: the FIRST-registered custom is rightmost", () => {
    // The customs zone grows LEFTWARD (user rule 2026-10-11): the right
    // edge takes the first arrival, later customs land to its left.
    const out = orderActionRegistrations([
      reg("create", 0),
      reg("first", 1),
      reg("second", 2),
      reg("third", 3),
    ]);
    expect(out.map((r) => r.slot)).toEqual(["create", "third", "second", "first"]);
  });

  it("keeps same-slot registrations stable by sequence", () => {
    const out = orderActionRegistrations([reg("create", 5), reg("create", 2)]);
    expect(out.map((r) => r.seq)).toEqual([2, 5]);
  });
});
