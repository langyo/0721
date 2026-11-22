import { defineComponent, h, ref, type Ref } from "vue";
import { enableAutoUnmount, flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";

const { iconButtonAttrs } = vi.hoisted(() => ({
  iconButtonAttrs: [] as Array<Record<string, unknown>>,
}));

// HkIconButton stub captures its props; the rest of hikari stays real.
vi.mock("@celestia-island/hikari", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@celestia-island/hikari")>();
  return {
    ...actual,
    HkIconButton: defineComponent({
      name: "HkIconButtonStub",
      inheritAttrs: false,
      setup(_, { attrs }) {
        iconButtonAttrs.push(attrs as Record<string, unknown>);
        return () => null;
      },
    }),
  };
});

vi.mock("vue-i18n", () => ({
  useI18n: () => ({ t: (_k: string, f?: string) => f ?? _k }),
}));

import { provideActionBar } from "@/composables/useActionBar";
import { usePageRefresh } from "@/composables/usePageRefresh";

enableAutoUnmount(afterEach);

function mountChrome(options: {
  refresh?: () => void;
  loading?: Ref<boolean>;
  visible?: () => boolean;
}) {
  // Two components: provide/inject does NOT resolve within the providing
  // component itself, so the registering view must be a CHILD of the
  // layout (exactly the AdminLayout→view relationship the harness
  // mirrors for the view tests).
  const Registrar = defineComponent({
    name: "Registrar",
    setup() {
      usePageRefresh(options.refresh ?? (() => {}), options);
      return () => null;
    },
  });
  const Layout = defineComponent({
    name: "Layout",
    setup() {
      const bar = provideActionBar();
      return () => [
        h(Registrar),
        h("div", { class: "probe" }, bar.actions.value.flatMap((r) => r())),
      ];
    },
  });
  return mount(Layout);
}

describe("usePageRefresh", () => {
  afterEach(() => {
    iconButtonAttrs.length = 0;
  });

  it("registers the standard laddered ghost refresh with a fresh label per render", async () => {
    mountChrome({ refresh: () => {} });
    await flushPromises();
    expect(iconButtonAttrs).toHaveLength(1);
    // The rung is viewport-driven (useChromeActionSize): assert the ladder.
    expect([28, 32]).toContain(iconButtonAttrs[0]!.size);
    expect(iconButtonAttrs[0]!.variant).toBe("ghost");
    expect(iconButtonAttrs[0]!["aria-label"]).toBe("Refresh");
  });

  it("hides the button while visible() is false and shows it when true", async () => {
    const visible = ref(false);
    const w = mountChrome({ visible: () => visible.value });
    await flushPromises();
    // The renderer emits NOTHING while the gate is closed — the probe is
    // empty (the sessions drill-down stages' contract).
    expect(iconButtonAttrs).toHaveLength(0);
    expect(w.find(".probe").text()).toBe("");

    visible.value = true;
    await flushPromises();
    expect(iconButtonAttrs).toHaveLength(1);
    // The rung is viewport-driven (useChromeActionSize): assert the ladder.
    expect([28, 32]).toContain(iconButtonAttrs[0]!.size);
  });

  it("disables the button while loading is true", async () => {
    const loading = ref(false);
    mountChrome({ loading });
    await flushPromises();
    expect(iconButtonAttrs[0]!.disabled).toBe(false);
    loading.value = true;
    await flushPromises();
    expect(iconButtonAttrs[0]!.disabled).toBe(true);
  });
});
