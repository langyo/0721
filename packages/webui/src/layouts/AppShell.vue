<script setup lang="ts">
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
  FolderOpen,
  Images,
  KeyRound,
  Link2,
  LogOut,
  Settings,
  SlidersHorizontal,
  Unplug,
  Users as UsersIcon,
} from "lucide-vue-next";
import { computed, ref, type Component } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";

import SettingsDialog from "@/components/SettingsDialog.vue";
import { LOCALE_OPTIONS, setLocale } from "@/i18n";
import { useAuthStore } from "@/stores/auth";
import { useConnectionProbe } from "@/composables/useConnectionProbe";
import { useEngineHealth } from "@/composables/useEngineHealth";

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

const appVersion: string = __APP_VERSION__;

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
</script>

<template>
  <HkAdminShell :sidebar-collapsed="false" :content-key="route.path">
    <template #header="{ actions, center }">
      <HkAdminHeader
        :title="headerTitle"
        :username="displayName"
        :user-email="accountName"
        avatar-action="menu"
        :avatar-menu-label="t('common.menu.account')"
        :logout-label="t('common.actions.logout')"
        :locale-options="localeOptions"
        :current-locale="locale"
        :actions="actions"
        :avatar-trigger-label="t('common.menu.account')"
        :locale-menu-label="t('common.chrome.language')"
        @locale-select="(code: string) => setLocale(code)"
        @logout="handleLogout"
      >
        <template #theme-toggle>
          <!-- The unified settings entry (the end-cluster position):
               theme + language + about live behind this gear. -->
          <HkIconButton
            :size="chromeSize"
            variant="ghost"
            :aria-label="settingsLabel"
            :title="settingsLabel"
            @click="settingsOpen = true"
          >
            <template #icon><Settings :size="16" /></template>
          </HkIconButton>
        </template>
        <!-- The shell hands the header the page-owned centre renderer;
             a render function IS a functional component — mount it
             directly instead of wrapping it in a fresh identity. -->
        <template v-if="center" #center>
          <component :is="center" />
        </template>
        <template #avatar-modal="{ open, onUpdateOpen }">
          <HkModal
            :model-value="open"
            :title="t('common.menu.account')"
            width="26rem"
            @update:model-value="(v: boolean) => onUpdateOpen(v)"
          >
            <div class="account-window">
              <HkAvatar :name="displayName" size="lg" />
              <div class="account-rows">
                <div class="account-row">
                  <span class="account-label">{{ t("common.account.displayName") }}</span>
                  <span>{{ displayName || "—" }}</span>
                </div>
                <div class="account-row">
                  <span class="account-label">{{ t("common.account.username") }}</span>
                  <span class="account-mono">{{ accountName || "—" }}</span>
                </div>
                <div class="account-row">
                  <span class="account-label">{{ t("common.account.role") }}</span>
                  <span class="account-badge">{{ roleLabel }}</span>
                </div>
              </div>
              <div class="account-actions">
                <HkButton variant="danger" :aria-label="t('common.actions.logout')" @click="handleLogout">
                  <template #icon><LogOut :size="16" /></template>
                  {{ t("common.actions.logout") }}
                </HkButton>
              </div>
            </div>
          </HkModal>
        </template>
      </HkAdminHeader>
    </template>

    <template #sidebar="{ onNavigate, inDrawer, collapsed }">
      <HSidebar :width="inDrawer ? '100%' : '224px'" :collapsed="collapsed ?? false">
        <HkMenu
          variant="sidebar"
          open
          default-expand-groups
          title="0721"
          :items="menuItems"
          :active-key="activeKey"
          @select="(key: string) => navigate(key, onNavigate)"
        />
      </HSidebar>
    </template>

    <template #content>
      <RouterView />
    </template>

    <template #footer>
      <HkStatusBar
        :version="appVersion"
        :engine-version="health?.engineVersion ?? null"
        :engine-build-hash="health?.engineBuildHash ?? undefined"
        :connection-status="statusBarState"
        :latency-ms="probe.latencyMs ?? undefined"
        :transport-tier="probe.transportTier"
        :attempt-number="probe.attemptNumber"
        :countdown="probe.countdown"
        @retry="retryNow"
      />
    </template>
  </HkAdminShell>

  <SettingsDialog v-model="settingsOpen" />
</template>

<style scoped lang="scss">
.account-window {
  display: flex;
  flex-direction: column;
  gap: 1rem;
  align-items: center;
  padding-block: 0.5rem;
}

.account-rows {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
  width: 100%;
}

.account-row {
  display: flex;
  justify-content: space-between;
  gap: 1rem;
  padding: 0.375rem 0.5rem;
  border-radius: 0.5rem;
  background: color-mix(in srgb, currentColor 5%, transparent);
  font-size: 0.9rem;
}

.account-label {
  opacity: 0.65;
}

.account-mono {
  font-family: var(--hk-font-mono, ui-monospace, monospace);
}

.account-badge {
  font-weight: 600;
}

.account-actions {
  margin-top: 0.25rem;
}
</style>
