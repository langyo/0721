/**
 * The unified page action bar — re-exported from hikari upstream.
 *
 * This used to be a local port of the chest-era composable; the family
 * upstreamed the whole ranked-additive system (provideActionBar /
 * useActionBar / PageActionSlot / orderActionRegistrations +
 * useActionBarCenter) and HkAdminShell now provides the bar itself —
 * the shell hands the header slot its ranked `actions` and `center`
 * channels. The hud zone never upstreamed (chat-shell-specific), and
 * 0721 never rendered it; the import surface is kept so the wrapper
 * composables (usePageCreateAction / usePageRefresh) and the view
 * tests keep their addresses while every line of logic lives upstream.
 */
export {
  PageActionSlot,
  orderActionRegistrations,
  provideActionBar,
  useActionBar,
  useActionBarCenter,
} from "@celestia-island/hikari";

export type {
  ActionBarRegistration,
  ActionBarRenderer,
  PageActionSlotValue,
} from "@celestia-island/hikari";
