import { defineStore } from "pinia";
import { computed, ref } from "vue";

import * as api from "@/api";
import { loadSession, onUnauthorized, saveSession } from "@/api/client";
import { setProbeClient } from "@/composables/useConnectionProbe";
import { onRpcAuthLost } from "@/transport";
import { rpcClient } from "@/transport";
import type { UserDto } from "@/types";

/**
 * The session store. The token persists in localStorage (15-day server
 * tokens — the legacy semantics); `restore()` verifies it at boot so a
 * stale token lands on /login instead of a broken console. A verified
 * session also (re)connects the JSON-RPC transport — the WS handshake
 * presents the token through the client's getToken hook.
 */
export const useAuthStore = defineStore("auth", () => {
  const token = ref<string | null>(loadSession()?.token ?? null);
  const user = ref<UserDto | null>(null);
  /** True while restore() is proving the persisted token. */
  const restoring = ref(false);

  const isAuthenticated = computed(() => token.value !== null);
  const isAdmin = computed(() => user.value?.role === "admin");

  function connectTransport(): void {
    setProbeClient(rpcClient);
    void rpcClient.connect();
  }

  async function login(username: string, password: string): Promise<UserDto> {
    const session = await api.login({ username, password });
    token.value = session.token;
    user.value = session.user;
    saveSession({ token: session.token });
    connectTransport();
    return session.user;
  }

  async function restore(): Promise<UserDto | null> {
    if (!token.value) return null;
    restoring.value = true;
    try {
      user.value = await api.verify();
      connectTransport();
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
    // The installed RpcClient exposes no teardown; with getToken now
    // answering null the socket dies on its next handshake/auth beat
    // and the reconnect loop parks (the login face shows no light for
    // the WS tier anyway).
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

/** Wire the global 401/auth-lost reactions (call once at app boot,
 *  after pinia): both the REST lane's 401 and the WS lane's auth close
 *  land in the same logout. */
export function wireUnauthorizedLogout(store: ReturnType<typeof useAuthStore>): void {
  onUnauthorized(() => store.logout());
  onRpcAuthLost(() => store.logout());
}
