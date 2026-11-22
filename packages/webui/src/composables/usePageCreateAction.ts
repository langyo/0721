import { h, type Component, type Ref } from "vue";
import { Plus } from "lucide-vue-next";

import {
  HkIconButton,
  useChromeActionSize,
} from "@celestia-island/hikari";

import { PageActionSlot, useActionBar } from "./useActionBar";

/**
 * The standard CREATE affordance for a page whose content is an
 * admin-managed list.
 *
 * A page's primary create action is a plus icon button in the app header's
 * top-right cluster — never a standalone button inside the page body (user
 * direction 2026-10-05: 新增一律是右上角图标组里的加号按钮、在刷新左边，
 * 包括 API 客户端、插件安装等全部页面). The visible label the in-page
 * button used to carry becomes the button's accessible name and tooltip;
 * the page keeps owning its own dialog.
 *
 * Geometry is deliberately identical to `usePageRefresh`'s (HkIconButton
 * the chrome ladder, 16px glyph) so the cluster reads as one row of chrome, and the
 * registration declares the Create slot, whose canonical rank (SLOT_RANK in
 * useActionBar.ts, the one ranking table) places the plus leftmost no matter
 * which composable the view calls first:
 * [create][filter?][search][refresh][window].
 *
 * Call once from a view's (or card's) setup; the registration dies with
 * that scope, so leaving the page — or switching the tab that mounted the
 * card — clears the button.
 */
export function usePageCreateAction(options: {
  /** Accessible name + tooltip — the label the in-page button printed.
   *  A FUNCTION is resolved on every header render, which is how a page
   *  whose create action changes with its sub-tab (or its locale) keeps the
   *  button labelled correctly without re-registering. */
  label: string | (() => string);
  /** Opens the page's own create dialog / starts the create flow. */
  onCreate: () => unknown;
  /** True while the action cannot run (e.g. a save is in flight). */
  disabled?: Ref<boolean> | (() => boolean);
  /** False hides the button entirely (evaluated on every header render). */
  visible?: () => boolean;
  /** Glyph override; defaults to the plus. */
  icon?: Component;
  /** Glyph size in px; defaults to 16 (the refresh button's). */
  iconSize?: number;
}): void {
  const bar = useActionBar();
  // The chrome ladder (2026-10-24 ruling, hikari single source):
  // the whole top-right cluster steps together — a flat 28 next
  // to ladder-32 neighbors read as a different button family
  // (2026-10-27 report).
  const chromeSize = useChromeActionSize();
  bar.setActions(() => {
    if (options.visible?.() === false) return [];
    const Icon = options.icon ?? Plus;
    const label = typeof options.label === "function" ? options.label() : options.label;
    const disabled =
      typeof options.disabled === "function"
        ? options.disabled()
        : options.disabled?.value === true;
    return [
      h(
        HkIconButton,
        {
          size: chromeSize.value,
          variant: "ghost",
          "aria-label": label,
          title: label,
          disabled,
          onClick: () => {
            void options.onCreate();
          },
        },
        { icon: () => h(Icon, { size: options.iconSize ?? 16 }) },
      ),
    ];
  }, PageActionSlot.Create);
}
