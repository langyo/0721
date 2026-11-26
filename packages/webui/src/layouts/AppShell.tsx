import {
  HkAdminHeader,
  HkAdminShell,
  HkAvatar,
  HkButton,
  HkIconButton,
  HkMenu,
  HkModal,
  HkStatusBar,
  HSidebar,
  useChromeActionSize,
  type HkMenuItem,
} from "@celestia-island/hikari";
import {
  Clock,
  Database,
  FolderOpen,
  Images,
  KeyRound,
  Link2,
  LogOut,
  Settings,
  PackageOpen,
  SlidersHorizontal,
  Unplug,
  Users as UsersIcon,
} from "lucide-vue-next";
import { appVersionLabel } from "@/lib/versionLabel";
import { computed, defineComponent, h, ref, type Component, type VNode } from "vue";
import { useI18n } from "vue-i18n";
import { RouterView, useRoute, useRouter } from "vue-router";

import SettingsDialog from "@/components/SettingsDialog";
import { LOCALE_OPTIONS, setLocale } from "@/i18n";
import { useAuthStore } from "@/stores/auth";
import { useConnectionProbe } from "@/composables/useConnectionProbe";
import { useEngineHealth } from "@/composables/useEngineHealth";

import "./AppShell.scss";

/**
 * The authed shell — the family admin console anatomy (the ayane/chest
 * blueprint):
 *
 *  - `HkAdminShell` owns the frame AND the page action bar: it hands
 *    the header slot the ranked `actions` (slot order
 *    [create][filter][search][refresh][window] — the law lives in
 *    hikari's PageActionSlot) plus the page-owned `center` renderer.
 *    Views register through `useActionBar()` (upstream); nothing in
 *    this layout touches the bar anymore.
 *  - The sidebar is the family grouped nav (`HSidebar` + `HkMenu`):
 *    themed collapsible sections — content / access / system — with
 *    the system group mounted only for admins.
 *  - The header carries the identity face: avatar menu → the account
 *    window (HkModal), the language cascade, logout, and the gear that
 *    opens the family settings dialog (theme / language / about).
 *  - The footer is the family `HkStatusBar`: the panel version + the
 *    LIVE engine identity (from /api/health) + the transport probe
 *    (WS tier, heartbeat latency, retry round) with tap-to-retry.
 */

/** The header slot's payload the shell hands down: the ranked action
 * bar's rendered affordances and the page-owned centre renderer. */
interface ShellHeaderSlot {
  actions?: VNode[];
  center?: Component;
}

/** The sidebar slot's payload (HkFaceShell's nav channels). */
interface ShellSidebarSlot {
  onNavigate?: () => void;
  inDrawer?: boolean;
  collapsed?: boolean;
}

export default defineComponent({
  name: "AppShell",
  setup() {
    const { t, locale } = useI18n();
    const route = useRoute();
    const router = useRouter();
    const auth = useAuthStore();

    const settingsOpen = ref(false);
    const accountOpen = ref(false);
    const chromeSize = useChromeActionSize();

    const { result: probe, retryNow } = useConnectionProbe();
    const { health } = useEngineHealth();

    const headerTitle = computed(() =>
      typeof route.meta.titleKey === "string" ? t(route.meta.titleKey) : "",
    );

    // ── Sidebar: grouped nav (the console's table of contents) ───────────

    interface NavItem {
      key: string;
      path: string;
      labelKey: string;
      icon: Component;
    }

    const CONTENT_ITEMS: NavItem[] = [
      { key: "portal", path: "/", labelKey: "common.nav.portal", icon: Images },
      { key: "files", path: "/files", labelKey: "common.files.title", icon: FolderOpen },
      { key: "timeline", path: "/timeline", labelKey: "common.files.timeline", icon: Clock },
      { key: "shares", path: "/shares", labelKey: "common.files.myShares", icon: Link2 },
    ];

    const ACCESS_ITEMS: NavItem[] = [
      { key: "keys", path: "/keys", labelKey: "common.keys.title", icon: KeyRound },
    ];

    const SYSTEM_ITEMS: NavItem[] = [
      { key: "users", path: "/users", labelKey: "common.users.title", icon: UsersIcon },
      { key: "buckets", path: "/buckets", labelKey: "common.buckets.title", icon: Database },
      { key: "unregistered", path: "/unregistered", labelKey: "common.unregistered.title", icon: PackageOpen },
      { key: "providers", path: "/providers", labelKey: "common.providers.title", icon: Unplug },
      { key: "site-settings", path: "/settings", labelKey: "common.nav.settings", icon: SlidersHorizontal },
    ];

    function toMenuItem(item: NavItem): HkMenuItem {
      return { key: item.key, label: t(item.labelKey), icon: item.icon };
    }

    const menuItems = computed<HkMenuItem[]>(() => {
      const groups: { id: string; labelKey: string; items: NavItem[] }[] = [
        { id: "content", labelKey: "common.nav.groupContent", items: CONTENT_ITEMS },
        { id: "access", labelKey: "common.nav.groupAccess", items: ACCESS_ITEMS },
      ];
      if (auth.isAdmin) {
        groups.push({ id: "system", labelKey: "common.nav.groupSystem", items: SYSTEM_ITEMS });
      }
      return groups.map((group) => ({
        key: `group:${group.id}`,
        label: t(group.labelKey),
        children: group.items.map(toMenuItem),
      }));
    });

    const activeKey = computed(() => {
      const path = route.path;
      if (path === "/") return "portal";
      const hit = [...CONTENT_ITEMS, ...ACCESS_ITEMS, ...SYSTEM_ITEMS].find(
        (item) => item.path !== "/" && path.startsWith(item.path),
      );
      return hit?.key ?? "portal";
    });

    function navigate(key: string, close?: () => void): void {
      const all = [...CONTENT_ITEMS, ...ACCESS_ITEMS, ...(auth.isAdmin ? SYSTEM_ITEMS : [])];
      const target = all.find((item) => item.key === key);
      if (target) void router.push(target.path);
      close?.();
    }

    // ── Header: identity face + settings gear ────────────────────────────

    const displayName = computed(() => auth.user?.display_name || auth.user?.username || "");
    const accountName = computed(() => auth.user?.username ?? "");
    const roleLabel = computed(() =>
      auth.isAdmin ? t("common.status.admin") : t("common.status.member"),
    );
    const settingsLabel = computed(() => t("common.chrome.settings"));

    const localeOptions = LOCALE_OPTIONS.value.map((o) => ({ code: o.code, label: o.label }));

    async function handleLogout(): Promise<void> {
      settingsOpen.value = false;
      accountOpen.value = false;
      await auth.logout();
      await router.push({ name: "login" });
    }

    // ── Footer: the family status bar over the transport probe ───────────

    // Family rule: the status bar shows `<version> <branch>:<hash7>` so two
    // builds of the same semver are distinguishable (see lib/versionLabel).
    const versionLabel: string = appVersionLabel();

    const statusBarState = computed<"connected" | "reconnecting" | "disconnected" | "connecting">(
      () => {
        switch (probe.value.state) {
          case "connected":
            return "connected";
          case "connecting":
            return "connecting";
          case "reconnecting":
            return "reconnecting";
          default:
            // disconnected | failed collapse: the bar has no failed lamp.
            return "disconnected";
        }
      },
    );

    return () => (
      <>
        <HkAdminShell sidebarCollapsed={false} contentKey={route.path}>
          {{
            header: (payload: ShellHeaderSlot) => (
              <HkAdminHeader
                title={headerTitle.value}
                username={displayName.value}
                userEmail={accountName.value}
                avatarAction="menu"
                avatarMenuLabel={t("common.menu.account")}
                logoutLabel={t("common.actions.logout")}
                localeOptions={localeOptions}
                currentLocale={locale.value}
                actions={payload.actions}
                avatarTriggerLabel={t("common.menu.account")}
                localeMenuLabel={t("common.chrome.language")}
                onLocaleSelect={(code: string) => setLocale(code)}
                onLogout={handleLogout}
              >
                {{
                  "theme-toggle": () => (
                    // The unified settings entry (the end-cluster position):
                    // theme + language + about live behind this gear.
                    <HkIconButton
                      size={chromeSize.value}
                      variant="ghost"
                      aria-label={settingsLabel.value}
                      {...{ title: settingsLabel.value }}
                      onClick={() => (settingsOpen.value = true)}
                    >
                      {{ icon: () => <Settings size={16} /> }}
                    </HkIconButton>
                  ),
                  // The shell hands the header the page-owned centre renderer;
                  // a render function IS a functional component — mount it
                  // directly instead of wrapping it in a fresh identity.
                  ...(payload.center ? { center: () => h(payload.center!) } : {}),
                  "avatar-modal": (modal: { open: boolean; onUpdateOpen: (v: boolean) => void }) => (
                    <HkModal
                      modelValue={modal.open}
                      title={t("common.menu.account")}
                      width="26rem"
                      onUpdate:modelValue={(v: boolean) => modal.onUpdateOpen(v)}
                    >
                      <div class="account-window">
                        <HkAvatar name={displayName.value} size="lg" />
                        <div class="account-rows">
                          <div class="account-row">
                            <span class="account-label">{t("common.account.displayName")}</span>
                            <span>{displayName.value || "—"}</span>
                          </div>
                          <div class="account-row">
                            <span class="account-label">{t("common.account.username")}</span>
                            <span class="account-mono">{accountName.value || "—"}</span>
                          </div>
                          <div class="account-row">
                            <span class="account-label">{t("common.account.role")}</span>
                            <span class="account-badge">{roleLabel.value}</span>
                          </div>
                        </div>
                        <div class="account-actions">
                          <HkButton variant="danger" aria-label={t("common.actions.logout")} onClick={handleLogout}>
                            {{
                              icon: () => <LogOut size={16} />,
                              default: () =>                                   t("common.actions.logout"),
                            }}
                          </HkButton>
                        </div>
                      </div>
                    </HkModal>
                  ),
                }}
              </HkAdminHeader>
            ),

            sidebar: (payload: ShellSidebarSlot) => (
              <HSidebar width={payload.inDrawer ? "100%" : "224px"} collapsed={payload.collapsed ?? false}>
                <HkMenu
                  variant="sidebar"
                  open
                  defaultExpandGroups
                  title="0721"
                  items={menuItems.value}
                  activeKey={activeKey.value}
                  onSelect={(key: string) => navigate(key, payload.onNavigate)}
                />
              </HSidebar>
            ),

            content: () => (
              <RouterView />
            ),

            footer: () => (
              <HkStatusBar
                version={versionLabel}
                engineVersion={health.value?.engineVersion ?? null}
                engineBuildHash={health.value?.engineBuildHash ?? undefined}
                connectionStatus={statusBarState.value}
                latencyMs={probe.value.latencyMs ?? undefined}
                transportTier={probe.value.transportTier}
                attemptNumber={probe.value.attemptNumber}
                countdown={probe.value.countdown}
                onRetry={retryNow}
              />
            ),
          }}
        </HkAdminShell>

        <SettingsDialog
          modelValue={settingsOpen.value}
          onUpdate:modelValue={(v: boolean) => (settingsOpen.value = v)}
        />
      </>
    );
  },
});
