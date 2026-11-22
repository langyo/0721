import { defineStore } from "pinia";
import { computed, ref } from "vue";

import * as api from "@/api";
import { onUnauthorized } from "@/api/client";
import { setProbeClient } from "@/composables/useConnectionProbe";
import { rpcClient } from "@/transport";
import { connectTransport as connectRpcTransport } from "@/transport";
import { onRpcAuthLost } from "@/transport";
import type { UserDto } from "@/types";

/**
 * The session store — cookie-backed, memory-only.
 *
 * The credential lives in the httpOnly cookie the REST login plants;
 * this store keeps only the USER projection in memory (nothing session-
 * shaped is persisted: no localStorage key, no token the page could
 * leak). A reload restores the session by proving the cookie over the
 * RPC lane (`auth.verify`); a 401 anywhere drops the in-memory state
 * and the router guard lands on /login.
 */
export const useAuthStore = defineStore("auth", () => {
  const user = ref<UserDto | null>(null);
  /** True while restore() is proving the session cookie. */
  const restoring = ref(false);

  const isAuthenticated = computed(() => user.value !== null);
  const isAdmin = computed(() => user.value?.role === "admin");

  function connectTransport(): void {
    setProbeClient(rpcClient);
    void connectRpcTransport();
  }

  async function login(username: string, password: string): Promise<UserDto> {
    // The login plants the httpOnly cookie (same-origin from here on).
    const session = await api.login({ username, password });
    user.value = session.user;
    connectTransport();
    return session.user;
  }

  async function restore(): Promise<UserDto | null> {
    if (user.value !== null) return user.value;
    restoring.value = true;
    try {
      // Proves the cookie: no token anywhere in this flow.
      user.value = await api.verify();
      connectTransport();
      return user.value;
    } catch {
      // No cookie / expired cookie: nothing to clear client-side (the
      // cookie is httpOnly); the router guard sends the visitor to
      // /login, where the stale cookie costs nothing (verify failed).
      user.value = null;
      return null;
    } finally {
      restoring.value = false;
    }
  }

  async function logout(): Promise<void> {
    // The cookie is httpOnly — only the server can clear it. The REST
    // logout expires it AND supersedes every outstanding token at the
    // watermark; failures still drop the local state (worst case the
    // cookie dies of old age).
    try {
      await api.logout();
    } catch {
      // Server unreachable: proceed with the local drop.
    }
    user.value = null;
  }

  return {
    user,
    restoring,
    isAuthenticated,
    isAdmin,
    login,
    restore,
    logout,
  };
});

/** Wire the global 401/auth-lost reactions (call once at app boot,
 *  after pinia): both the REST lane's 401 and the WS lane's auth close
 *  land in the same logout. */
export function wireUnauthorizedLogout(store: ReturnType<typeof useAuthStore>): void {
  onUnauthorized(() => {
    void store.logout();
  });
  onRpcAuthLost(() => {
    void store.logout();
  });
}
