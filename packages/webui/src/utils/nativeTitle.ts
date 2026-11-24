/**
 * Native `title` tooltips for hikari icon buttons.
 *
 * `HkIconButton` declares `inheritAttrs: false` and spreads `attrs` onto its
 * root `<button>`, so a native `title` reaches the DOM as the browser's own
 * hover tooltip — which is exactly what the Vue SFC templates did when they
 * wrote `:title="…"` on these controls (the attribute fell through to the
 * button). Its TypeScript props do not declare `title`, so writing it as a
 * JSX attribute is a type error: the component's prop type is closed and TS
 * rejects any key it does not know.
 *
 * A spread carries it through untouched — spreads are not subject to
 * excess-property checking — so the tooltip survives the SFC → TSX move with
 * no behavioural change. Use this helper instead of either extreme: do NOT
 * drop the tooltip (that silently removes a user-visible affordance), and do
 * NOT fold it into `aria-label` (an accessible name is not a hover tooltip;
 * the two serve different users, and several call sites already pass both).
 *
 * Keep `aria-label` alongside this wherever the icon carries no visible text.
 */
export function nativeTitle(title: string): { title: string } {
  return { title };
}