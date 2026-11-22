import { defineStore } from "pinia";
import { computed, ref } from "vue";

import * as api from "@/api";
import { loadSession, onUnauthorized, saveSession } from "@/api/client";
import type { UserDto } from "@/types";

/**
 * The session store. The token persists in localStorage (15-day server
 * tokens — the legacy semantics); `restore()` verifies it at boot so a
 * stale token lands on /login instead of a broken console.
 */
export const useAuthStore = defineStore("auth", () => {
  const token = ref<string | null>(loadSession()?.token ?? null);
  const user = ref<UserDto | null>(null);
  /** True while restore() is proving the persisted token. */
  const restoring = ref(false);

  const isAuthenticated = computed(() => token.value !== null);
  const isAdmin = computed(() => user.value?.role === "admin");

  async function login(username: string, password: string): Promise<UserDto> {
    const session = await api.login({ username, password });
    token.value = session.token;
    user.value = session.user;
    saveSession({ token: session.token });
    return session.user;
  }

  async function restore(): Promise<UserDto | null> {
    if (!token.value) return null;
    restoring.value = true;
    try {
      user.value = await api.verify();
      return user.value;
    } catch {
      // Stale/expired token: drop the session; the router guard sends
      // the visitor to /login.
      token.value = null;
      user.value = null;
      saveSession(null);
      return null;
    } finally {
      restoring.value = false;
    }
  }

  function logout(): void {
    token.value = null;
    user.value = null;
    saveSession(null);
  }

  return {
    token,
    user,
    restoring,
    isAuthenticated,
    isAdmin,
    login,
    restore,
    logout,
  };
});

/** Wire the global 401 reaction (call once at app boot, after pinia). */
export function wireUnauthorizedLogout(store: ReturnType<typeof useAuthStore>): void {
  onUnauthorized(() => store.logout());
}
