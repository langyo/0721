import { defineComponent, h, ref } from "vue";
import { enableAutoUnmount, flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";

// usePageRefresh resolves its label through vue-i18n; the harness mounts no
// composer, so the key resolver is stubbed (house pattern for header-chrome
// tests).
vi.mock("vue-i18n", () => ({
  useI18n: () => ({ t: (key: string, fallback?: string) => fallback ?? key, locale: { value: "en" } }),
}));

import { provideActionBar } from "@/composables/useActionBar";
import { usePageCreateAction } from "@/composables/usePageCreateAction";
import { usePageRefresh } from "@/composables/usePageRefresh";

enableAutoUnmount(afterEach);

/**
 * The create-action contract (user direction 2026-10-05): a page's primary
 * create action is a plus ICON button in the header's top-right cluster,
 * registered at the FRONT so it always sits left of the refresh — never a
 * standalone button inside the page body.
 *
 * Exercised through a real mount with the layout's provider (no mock of the
 * module under test); the probe mirrors what AdminLayout/HeaderRight render.
 */

function makeHarness(view: ReturnType<typeof defineComponent>) {
  return defineComponent({
    name: "ActionBarHarness",
    setup() {
      const bar = provideActionBar();
      return () => [
        h("div", { class: "view-host" }, [h(view)]),
        h("div", { class: "probe-actions" }, bar.actions.value.flatMap((r) => r())),
      ];
    },
  });
}

function mountView(view: ReturnType<typeof defineComponent>) {
  return mount(makeHarness(view), { global: { stubs: { teleport: true } } });
}

function actionButtons(wrapper: ReturnType<typeof mountView>): HTMLButtonElement[] {
  const host = wrapper.element as HTMLElement;
  return Array.from(host.querySelectorAll<HTMLButtonElement>(".probe-actions button"));
}

describe("usePageCreateAction", () => {
  it("renders a ghost/28 plus icon button at the FRONT, left of a refresh registered later", async () => {
    const view = defineComponent({
      name: "CreateView",
      setup() {
        // The REFRESH registers first, and the create action second: if the
        // position came from call order (the tautology an earlier version of
        // this test accidentally exercised) the plus would land RIGHT of the
        // refresh. It must land left because its registration asks for the
        // front.
        const created = ref(0);
        usePageRefresh(() => {});
        usePageCreateAction({
          label: "Create client",
          onCreate: () => {
            created.value += 1;
          },
        });
        return () => h("div", String(created.value));
      },
    });
    const wrapper = mountView(view);
    await flushPromises();

    const buttons = actionButtons(wrapper);
    expect(buttons.length).toBe(2);
    const [create, refresh] = buttons;
    expect(create!.getAttribute("aria-label")).toBe("Create client");
    expect(create!.getAttribute("title")).toBe("Create client");
    expect(create!.className).toContain("hk-icon-button");
    // The chrome action ladder (useChromeActionSize): the rung is
    // viewport-driven, so the pin is "the ladder's class", never one
    // hard-coded step.
    const laddered = (cls: string) =>
      cls.includes("hk-icon-button-28") || cls.includes("hk-icon-button-32");
    expect(laddered(create!.className)).toBe(true);
    expect(create!.className).toContain("hk-icon-button-ghost");
    // Refresh rides the same ladder — one row of identical chrome.
    expect(laddered(refresh!.className)).toBe(true);
    expect(create!.compareDocumentPosition(refresh!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("runs the page's own create flow on click", async () => {
    const onCreate = vi.fn();
    const view = defineComponent({
      name: "CreateView2",
      setup() {
        usePageCreateAction({ label: "Add webhook", onCreate });
        return () => h("div");
      },
    });
    const wrapper = mountView(view);
    await flushPromises();
    actionButtons(wrapper)[0]!.click();
    await flushPromises();
    expect(onCreate).toHaveBeenCalledTimes(1);
  });

  it("hides the button while the page says it is not available", async () => {
    const allowed = ref(false);
    const view = defineComponent({
      name: "GatedView",
      setup() {
        usePageCreateAction({
          label: "Create client",
          onCreate: () => {},
          visible: () => allowed.value,
        });
        usePageRefresh(() => {});
        return () => h("div");
      },
    });
    const wrapper = mountView(view);
    await flushPromises();
    expect(actionButtons(wrapper).length).toBe(1); // the refresh only

    allowed.value = true;
    await flushPromises();
    const buttons = actionButtons(wrapper);
    expect(buttons.length).toBe(2);
    expect(buttons[0]!.getAttribute("aria-label")).toBe("Create client");
  });

  it("disables the button while the page reports an in-flight save", async () => {
    const busy = ref(true);
    const view = defineComponent({
      name: "BusyView",
      setup() {
        usePageCreateAction({ label: "Add", onCreate: () => {}, disabled: busy });
        return () => h("div");
      },
    });
    const wrapper = mountView(view);
    await flushPromises();
    expect(actionButtons(wrapper)[0]!.disabled).toBe(true);

    busy.value = false;
    await flushPromises();
    expect(actionButtons(wrapper)[0]!.disabled).toBe(false);
  });

  it("clears its own registration when the view unmounts", async () => {
    const view = defineComponent({
      name: "LeavingView",
      setup() {
        usePageCreateAction({ label: "Add", onCreate: () => {} });
        return () => h("div");
      },
    });
    const host = defineComponent({
      name: "Host",
      setup() {
        const show = ref(true);
        return () => h("div", [show.value ? h(view) : null, h("button", { class: "toggle", onClick: () => (show.value = false) }, "x")]);
      },
    });
    const wrapper = mountView(host);
    await flushPromises();
    expect(actionButtons(wrapper).length).toBe(1);

    await wrapper.find(".toggle").trigger("click");
    await flushPromises();
    expect(actionButtons(wrapper).length).toBe(0);
  });
});
