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
      component: () => import("@/views/LoginView.vue"),
      meta: { titleKey: "common.auth.title", public: true },
    },
    {
      path: "/",
      component: () => import("@/layouts/AppShell.vue"),
      children: [
        {
          path: "",
          name: "portal",
          component: () => import("@/views/PortalView.vue"),
          meta: { titleKey: "common.portal.title" },
        },
        {
          path: "users",
          name: "users",
          component: () => import("@/views/UsersView.vue"),
          meta: { titleKey: "common.users.title", admin: true },
        },
        {
          path: "settings",
          name: "site-settings",
          component: () => import("@/views/SettingsView.vue"),
          meta: { titleKey: "common.settings.title", admin: true },
        },
      ],
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
