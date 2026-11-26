import { createRouter, createWebHistory } from "vue-router";

import { useAuthStore } from "@/stores/auth";

/**
 * Route table. `/login` is public; everything else requires a session,
 * and the admin pages (`/users`, `/settings`) additionally require the
 * admin role — the guard reads the verified user (restore() ran before
 * the first navigation), so role checks never trust a forged local
 * claim: the server enforces them again on every request anyway.
 */
export const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      path: "/login",
      name: "login",
      component: () => import("@/views/LoginView"),
      meta: { titleKey: "common.auth.title", public: true },
    },
    {
      path: "/",
      component: () => import("@/layouts/AppShell"),
      children: [
        {
          path: "",
          name: "portal",
          component: () => import("@/views/PortalView"),
          meta: { titleKey: "common.portal.title" },
        },
        {
          path: "files",
          name: "files",
          component: () => import("@/views/FilesView"),
          meta: { titleKey: "common.files.title" },
        },
        {
          path: "files/:folderId",
          name: "files-folder",
          component: () => import("@/views/FilesView"),
          meta: { titleKey: "common.files.title" },
        },
        {
          path: "timeline",
          name: "timeline",
          component: () => import("@/views/TimelineView"),
          meta: { titleKey: "common.files.timeline" },
        },
        {
          path: "shares",
          name: "shares",
          component: () => import("@/views/SharesView"),
          meta: { titleKey: "common.files.myShares" },
        },
        {
          path: "users",
          name: "users",
          component: () => import("@/views/UsersView"),
          meta: { titleKey: "common.users.title", admin: true },
        },
        {
          path: "buckets",
          name: "buckets",
          component: () => import("@/views/BucketsView"),
          meta: { titleKey: "common.buckets.title", admin: true },
        },
        {
          path: "unregistered",
          name: "unregistered",
          component: () => import("@/views/UnregisteredView"),
          meta: { titleKey: "common.unregistered.title", admin: true },
        },
        {
          path: "settings",
          name: "site-settings",
          component: () => import("@/views/SettingsView"),
          meta: { titleKey: "common.settings.title", admin: true },
        },
        {
          path: "keys",
          name: "api-keys",
          component: () => import("@/views/KeysView"),
          meta: { titleKey: "common.keys.title" },
        },
        {
          path: "providers",
          name: "login-providers",
          component: () => import("@/views/ProvidersView"),
          meta: { titleKey: "common.providers.title", admin: true },
        },
      ],
    },
    {
      path: "/s/:token",
      name: "shared-file",
      component: () => import("@/views/SharedFileView"),
      meta: { public: true, titleKey: "common.files.sharedFile" },
    },
    { path: "/:pathMatch(.*)*", redirect: "/" },
  ],
});

router.beforeEach((to) => {
  const auth = useAuthStore();
  if (to.meta.public) return true;
  if (!auth.isAuthenticated) {
    return { name: "login", query: { redirect: to.fullPath } };
  }
  if (to.meta.admin && !auth.isAdmin) {
    return { name: "portal" };
  }
  return true;
});
