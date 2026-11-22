<script setup lang="ts">
import {
  HkAdaptiveTabs,
  HkAdminHeader,
  HkIconButton,
  useChromeActionSize,
} from "@celestia-island/hikari";
import { Settings, Users as UsersIcon, Images, SlidersHorizontal, Unplug, FolderOpen, Clock, Link2, KeyRound } from "lucide-vue-next";
import { computed, ref, type Component } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";

import SettingsDialog from "@/components/SettingsDialog.vue";
import ConnectionStatusTag from "@/components/ConnectionStatusTag.vue";
import { provideActionBar } from "@/composables/useActionBar";
import { LOCALE_OPTIONS, setLocale } from "@/i18n";
import { useAuthStore } from "@/stores/auth";

/**
 * The authed shell — the chest page-chrome anatomy on a 3-page app:
 *
 *  - HkAdminHeader owns the action row. Pages register their buttons
 *    through the action-bar composables (`useActionBar` /
 *    `usePageCreateAction` / `usePageRefresh`); the plus (create), the
 *    refresh and any custom affordance render in the canonical slot
 *    order [create][filter][search][refresh][window] — the slot law's
 *    single source is hikari's page-chrome design note plus
 *    useActionBar's SLOT_RANK table.
 *  - The header's END position (its theme-toggle slot, where chest mounts
 *    the unified settings entry) carries the settings gear — the same
 *    ghost chrome as every other header action.
 *  - Primary navigation rides HkAdaptiveTabs under the header: ≥768 the
 *    pill strip, <768 the same value contract collapses into a
 *    sheet-anchored picker (the family's mobile threshold).
 */

const { t, locale } = useI18n();
const route = useRoute();
const router = useRouter();
const auth = useAuthStore();

// The unified page action bar: one provider per shell; every view
// registers its own header buttons for exactly as long as it is mounted.
const pageBar = provideActionBar();

const headerTitle = computed(() =>
  typeof route.meta.titleKey === "string" ? t(route.meta.titleKey) : "",
);

interface NavTab {
  key: string;
  label: string;
  icon: Component;
  adminOnly: boolean;
}

const navTabs = computed<NavTab[]>(() => [
  { key: "/", label: t("common.nav.portal"), icon: Images, adminOnly: false },
  { key: "/files", label: t("common.files.title"), icon: FolderOpen, adminOnly: false },
  { key: "/timeline", label: t("common.files.timeline"), icon: Clock, adminOnly: false },
  { key: "/shares", label: t("common.files.myShares"), icon: Link2, adminOnly: false },
  { key: "/keys", label: t("common.keys.title"), icon: KeyRound, adminOnly: false },
  { key: "/users", label: t("common.nav.users"), icon: UsersIcon, adminOnly: true },
  { key: "/settings", label: t("common.nav.settings"), icon: SlidersHorizontal, adminOnly: true },
  { key: "/providers", label: t("common.providers.title"), icon: Unplug, adminOnly: true },
]);

const visibleTabs = computed(() =>
  navTabs.value
    .filter((tab) => !tab.adminOnly || auth.isAdmin)
    .map((tab) => ({ key: tab.key, label: tab.label, icon: tab.icon })),
);

const activeTab = computed(() => (route.path === "/" ? "/" : route.path));

function onTabSelect(key: string) {
  void router.push(key);
}

const settingsOpen = ref(false);
const chromeSize = useChromeActionSize();
const settingsLabel = computed(() => t("common.chrome.settings"));

async function handleLogout() {
  settingsOpen.value = false;
  auth.logout();
  await router.push({ name: "login" });
}

const localeOptions = LOCALE_OPTIONS.value.map((o) => ({ code: o.code, label: o.label }));
</script>

<template>
  <div class="app-shell">
    <header class="app-shell-header">
      <HkAdminHeader
        :title="headerTitle"
        :username="auth.user?.display_name ?? auth.user?.username ?? ''"
        :actions="pageBar.actions.value.flatMap((renderer) => renderer())"
        :locale-options="localeOptions"
        :current-locale="locale"
        :logout-label="t('common.actions.logout')"
        :avatar-trigger-label="t('common.chrome.settings')"
        :locale-menu-label="t('common.chrome.language')"
        @locale-select="(code: string) => setLocale(code)"
        @logout="handleLogout"
      >
        <template #theme-toggle>
          <!-- The unified settings entry (chest's end-cluster position):
               theme + language + about + sign-out live behind this gear. -->
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
      </HkAdminHeader>
      <nav class="app-shell-nav" :aria-label="t('common.nav.portal')">
        <HkAdaptiveTabs :model-value="activeTab" :tabs="visibleTabs" @update:model-value="onTabSelect" />
      </nav>
    </header>
    <main class="app-shell-content">
      <RouterView v-slot="{ Component: view }">
        <component :is="view" />
      </RouterView>
    </main>
    <footer class="app-shell-footer">
      <!-- The bottom-left connection light: the transport probe (live WS
           state + heartbeat latency); tap-to-retry rides the probe. -->
      <ConnectionStatusTag source="transport" />
    </footer>
    <SettingsDialog v-model="settingsOpen" @logout="handleLogout" />
  </div>
</template>

<style scoped lang="scss">
.app-shell {
  min-height: 100vh;
  display: flex;
  flex-direction: column;
}

.app-shell-header {
  position: sticky;
  top: 0;
  z-index: 10;
}

.app-shell-nav {
  display: flex;
  justify-content: center;
  padding: 0.25rem 1rem 0.5rem;
}

.app-shell-content {
  flex: 1;
  width: 100%;
  max-width: 72rem;
  margin: 0 auto;
  padding: 1rem 1rem 3rem;
}

.app-shell-footer {
  position: fixed;
  inset-inline-start: 0.75rem;
  inset-block-end: 0.75rem;
  z-index: 20;
}
</style>
