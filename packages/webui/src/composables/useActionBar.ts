import { computed, inject, onScopeDispose, provide, ref, type Ref, type VNode } from "vue";

/**
 * Unified page action bar.
 *
 * A layout (`AdminLayout`, `ChatLayout`) calls `provideActionBar()` once and
 * renders `actions.value` (top-right buttons) and `center.value` (the header
 * centre group) — and, in ChatLayout, `hud.value` (the left-wing readout
 * cluster; see the hud slot note below) — in its header. Any descendant view
 * then registers its own top-bar buttons
 * (filter trigger, refresh, add, search, …) via
 * `useActionBar().setActions(() => [...])` in its setup — the header updates
 * reactively. Registrations are ADDITIVE and render in registration order:
 * a page that registers its filter trigger and then its refresh gets
 * [filter, refresh] left-to-right in the header (the user direction:
 * 筛选在刷新左边；没有刷新就在主题按钮左边).
 *
 * A registration may ask for the FRONT of the cluster instead of the back
 * (`setActions(fn, "start")`). The page's primary create action uses it, so
 * the plus always lands left of the refresh — the user direction
 * (2026-10-05): 新增一律是右上角图标组里的加号按钮、在刷新左边，不是页面内
 * 单独的新增按钮。顺序因此固定为
 * [create][filter?][refresh][window][global search][theme]，与各 composable 的调用
 * 先后无关。
 *
 * Registration is **per-component-scope**: when the registering view unmounts
 * (e.g. switching the chat surface between report review and node overview),
 * its actions are automatically cleared so a stale button can't linger from
 * the previous page. This is what lets different pages "own" the top-right
 * area cleanly, mirroring the admin panel.
 *
 * **Slot ordering (2026-10-11 standardization)**: registrations declare
 * their PURPOSE slot; the cluster renders canonical slots by rank
 * ([create][filter][search][refresh][window]) and customs after them,
 * stuffed right-to-left in join order — see PageActionSlot below. The
 * registration CALL ORDER no longer influences the cluster.
 *
 * **Why a render function, not VNodes?** VNodes are single-use: once Vue
 * patches them into the DOM, re-using the same VNode object on the next
 * render cycle causes event handlers to silently detach (the click works
 * the first time but not after a re-render). Storing a function that returns
 * fresh VNodes ensures each render cycle gets a clean set with intact
 * event handlers.
 *
 * **The centre slot** (2026-09-28): the second header zone pages can own —
 * a filter / mode button group rendered in the header's true centre, the
 * console face of the chat frontend's top-centre tab strip (报告审阅 /
 * 节点概览 / 历史记录). Same registration contract, same per-scope
 * lifetime: a view whose filter is a small closed enum renders it there
 * (HkTabs segmented) instead of a filter card inside the page body.
 *
 * **The hud slot** (2026-10-02): the third header zone — read-only
 * indicators rendered at the END of the CHAT header's left wing (after
 * the workspace chip, HeaderLeft). ChatLayout is the ONLY layout
 * rendering `hud.value` today: AdminLayout does not (its header is
 * hikari's HkAdminHeader, which has no readout cluster), so an
 * AdminLayout page calling setHud would register INVISIBLY — extend
 * AdminLayout first if a console page ever needs a readout. The first
 * registrant is the 3D twin's live FPS tag (PhysicalPreview): instead
 * of the layout hardcoding an `activeDesktop === "holographic"` check
 * plus a bare styled span, the scene itself registers a framed tag for
 * exactly as long as it is mounted — the same contract as the right
 * wing's buttons, so a plugin surface can configure its own header
 * readouts without touching the layouts. HUD entries are INDICATORS: a
 * renderer must not render interactive controls (they have
 * `setActions`).
 */

/** A function that produces fresh VNodes for the action bar on each render. */
export type ActionBarRenderer = () => VNode[];

/**
 * The canonical top-right cluster slots (2026-10-11 standardization, user
 * direction): every button declares its PURPOSE, and the cluster renders
 * by the slot's canonical rank — [create][filter][search][refresh][window]
 * — regardless of registration order. A button whose purpose is not in
 * the enum is a CUSTOM slot: customs ride AFTER the canonical ranks,
 * stuffed 由右往左 — the zone's right edge (adjacent to the global
 * search) takes the FIRST-registered custom, the next one lands to its
 * LEFT — so the enum zone never gets displaced and customs never
 * interleave with it.
 *
 * Adding a canonical purpose = add the key here AND its rank in
 * SLOT_RANK below (one place, test-pinned); everything else registers as
 * its own custom slot string and inherits the stuffing rule.
 */
export const PageActionSlot = {
  /** The page's primary create action (the plus). */
  Create: "create",
  /** The page's filter trigger. */
  Filter: "filter",
  /** The page's search trigger (the list scaffold's keyword panel;
   * canonical since the scaffold landed — the old per-page
   * "Search = refetch" buttons rode custom slots). */
  Search: "search",
  /** The page's refresh. */
  Refresh: "refresh",
  /** A page's secondary-window opener (2026-11-04 user direction): an
   * icon button that opens a window/panel holding dynamic content the
   * page must NOT expand inline (凭据、部署助手等一切动态内容都收下一级
   * 的窗口内). The window itself stays the page's own (HkAdaptiveDialog);
   * only the OPENING affordance is header chrome. */
  Window: "window",
} as const;
export type PageActionSlotValue = (typeof PageActionSlot)[keyof typeof PageActionSlot];

/** Canonical ranks — lower renders further LEFT. */
const SLOT_RANK: Record<string, number> = {
  [PageActionSlot.Create]: 0,
  [PageActionSlot.Filter]: 1,
  [PageActionSlot.Search]: 2,
  [PageActionSlot.Refresh]: 3,
  [PageActionSlot.Window]: 4,
};

/** A setActions registration: the renderer plus its declared slot. */
export interface ActionBarRegistration {
  renderer: ActionBarRenderer;
  /** The button's purpose — a canonical PageActionSlot or a custom
   *  string; REQUIRED for every registration (the ordering key). */
  slot: string;
  /** Registration sequence (disambiguates same-slot entries). */
  seq: number;
}

/** Sort registrations into render order: canonical slots by rank; customs
 * after them, stuffed right-to-left (first-registered = rightmost, so
 * the customs zone grows LEFTWARD — reverse registration order).
 * Exported pure for the unit pin. */
export function orderActionRegistrations(regs: ActionBarRegistration[]): ActionBarRegistration[] {
  const canonical = regs
    .filter((r) => r.slot in SLOT_RANK)
    .sort((a, b) => SLOT_RANK[a.slot] - SLOT_RANK[b.slot] || a.seq - b.seq);
  const customs = regs.filter((r) => !(r.slot in SLOT_RANK)).sort((a, b) => b.seq - a.seq);
  return [...canonical, ...customs];
}

interface ActionBarContext {
  /** Every registered top-right entry (renderer + declared slot); the
   *  public `actions` computed serves them in SLOT order — a page may
   *  own SEVERAL (create, filter trigger, refresh, customs), and the
   *  order no longer depends on the composable call sequence. */
  actionRegistrations: Ref<ActionBarRegistration[]>;
  /** Per-PROVIDER registration sequence — one counter for every scope
   *  sharing this bar, so cross-component customs tie on nothing (the
   *  stuffing rule's seq comparison must be globally monotonic). */
  nextSeq: () => number;
  /** Ordered renderers (what the header renders) — derived. */
  actions: Ref<ActionBarRenderer[]>;
  center: Ref<ActionBarRenderer | null>;
  /** Registered header-HUD renderers (left-wing trailing readouts), in
   *  registration order — same additive, per-scope contract as actions. */
  hud: Ref<ActionBarRenderer[]>;
}

const KEY = Symbol("action-bar");

export function provideActionBar() {
  const actionRegistrations = ref<ActionBarRegistration[]>([]);
  let seqCounter = 0;
  const nextSeq = () => seqCounter++;
  const ctx: ActionBarContext = {
    actionRegistrations,
    nextSeq,
    actions: computed(() =>
      orderActionRegistrations(actionRegistrations.value).map((r) => r.renderer),
    ),
    center: ref<ActionBarRenderer | null>(null),
    hud: ref<ActionBarRenderer[]>([]),
  };
  provide(KEY, ctx);
  return ctx;
}

export function useActionBar() {
  const ctx = inject<ActionBarContext | null>(KEY, null);
  if (!ctx) {
    // No provider (e.g. outside any layout) — inert inert-refs keep the
    // return type uniform (consumers read `actions.value` without
    // null-narrowing), while the setters stay silent no-ops.
    return {
      setActions: (_renderer: ActionBarRenderer, _slot: string) => {},
      clearActions: () => {},
      actions: ref<ActionBarRenderer[]>([]),
      setCenter: (_renderer: ActionBarRenderer) => {},
      clearCenter: () => {},
      center: ref<ActionBarRenderer | null>(null),
      setHud: (_renderer: ActionBarRenderer) => {},
      clearHud: () => {},
      hud: ref<ActionBarRenderer[]>([]),
    };
  }
  return {
  setActions: (renderer: ActionBarRenderer, slot: string) => {
      // ADDITIVE registration: a page composes several buttons (create,
      // filter trigger, refresh, customs) by calling this once per
      // affordance, each declaring its PURPOSE slot — the cluster's
      // render order comes from the slot ranks, never the call
      // sequence. The identity guard keeps the dispose of one
      // registration (a crossfade overlap, an unmount) from clearing
      // its siblings.
      if (!ctx.actionRegistrations.value.some((r) => r.renderer === renderer)) {
        ctx.actionRegistrations.value = [
          ...ctx.actionRegistrations.value,
          { renderer, slot, seq: ctx.nextSeq() },
        ];
      }
      // Auto-clear when this view's effect scope dies so buttons never
      // outlive the page that registered them.
      onScopeDispose(() => {
        ctx.actionRegistrations.value = ctx.actionRegistrations.value.filter(
          (r) => r.renderer !== renderer,
        );
      });
    },
    clearActions: () => {
      ctx.actionRegistrations.value = [];
    },
    actions: ctx.actions,
    setCenter: (renderer: ActionBarRenderer) => {
      ctx.center.value = renderer;
      onScopeDispose(() => {
        if (ctx.center.value === renderer) {
          ctx.center.value = null;
        }
      });
    },
    clearCenter: () => {
      ctx.center.value = null;
    },
    center: ctx.center,
    setHud: (renderer: ActionBarRenderer) => {
      // Same additive + identity-guard contract as setActions (a view may
      // register several readouts; a crossfade overlap must not let the
      // leaving view's dispose wipe the entering one's tag).
      if (!ctx.hud.value.includes(renderer)) {
        ctx.hud.value = [...ctx.hud.value, renderer];
      }
      onScopeDispose(() => {
        ctx.hud.value = ctx.hud.value.filter((r) => r !== renderer);
      });
    },
    clearHud: () => {
      ctx.hud.value = [];
    },
    hud: ctx.hud,
  };
}
