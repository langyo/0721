import { h, type Ref } from "vue";
import { useI18n } from "vue-i18n";
import { RefreshCw } from "lucide-vue-next";

import {
  HkIconButton,
  useChromeActionSize,
} from "@celestia-island/hikari";

import { PageActionSlot, useActionBar } from "./useActionBar";

/**
 * The standard page refresh affordance for backend (AdminLayout) pages.
 *
 * Registers THE refresh button into the unified action bar so it renders in
 * the app header's top-right action slot — the same slot the theme toggle
 * lives in, and the same button geometry: HkIconButton on the chrome action ladder, the size
 * step the header's search/settings triggers use:
 * 后台每个子页面的刷新按钮统一放右上角、与主题切换同款). The in-body
 * refresh buttons this replaces (filter strips, page-header action rows)
 * are gone — a page either has a refresh, and it is HERE, or it has none.
 *
 * Call once from a view's setup; the registration dies with the view's
 * scope (useActionBar's per-scope contract), so switching pages clears the
 * button automatically.
 */
export function usePageRefresh(
  refresh: () => unknown,
  options?: {
    /** True while the page's fetch is in flight — the button disables. */
    loading?: Ref<boolean>;
    /** Accessible label override; defaults to common.actions.refresh. */
    label?: string;
    /** False hides the button entirely (the renderer emits nothing) — for
     *  views whose refresh belongs to ONE stage/mode of a longer-lived
     *  component. Evaluated on every header
     *  render, so flipping the stage shows/hides the button reactively;
     *  the per-scope registration contract is unchanged. */
    visible?: () => boolean;
  },
) {
  const bar = useActionBar();
  // The chrome ladder (2026-10-24 ruling, hikari single source):
  // the whole top-right cluster steps together — a flat 28 next
  // to ladder-32 neighbors read as a different button family
  // (2026-10-27 report).
  const chromeSize = useChromeActionSize();
  const { t } = useI18n();
  bar.setActions(() => {
    if (options?.visible?.() === false) return [];
    // Resolved per render (not captured at setup) so a runtime locale
    // switch re-labels the button like every other header chrome.
    const label = options?.label ?? t("common.actions.refresh", "Refresh");
    return [
      h(
        HkIconButton,
        {
          size: chromeSize.value,
          variant: "ghost",
          "aria-label": label,
          title: label,
          disabled: options?.loading?.value === true,
          onClick: () => {
            void refresh();
          },
        },
        // h() instead of JSX: this module is .ts (composables stay
        // framework-light) and a union-typed slot object renders fine.
        { icon: () => h(RefreshCw, { size: 16 }) },
      ),
    ];
  }, PageActionSlot.Refresh);
}
