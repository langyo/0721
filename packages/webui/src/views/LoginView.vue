<script setup lang="ts">
import { HkSignInCard } from "@celestia-island/hikari";
import { KeyRound } from "lucide-vue-next";
import { onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";

import * as api from "@/api";
import { ApiError } from "@/api/client";
import ConnectionStatusTag from "@/components/ConnectionStatusTag.vue";
import { useAuthStore } from "@/stores/auth";
import type { LoginMethods } from "@/types";

/**
 * The sign-in face, driven by `GET /api/v1/auth/login-methods`: the
 * password form renders only when the kill-switch allows it; every
 * enabled OAuth provider renders a button that navigates the WHOLE
 * page to the redirect entry (the flow plants an httpOnly state
 * cookie and 302s to the provider — a fetch cannot carry a navigation
 * through). A provider round-trip lands back signed in.
 */

const { t } = useI18n();
const router = useRouter();
const route = useRoute();
const auth = useAuthStore();

const methods = ref<LoginMethods | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);

onMounted(() => {
  api
    .loginMethods()
    .then((payload) => {
      methods.value = payload;
    })
    .catch(() => {
      // The descriptor is unreachable: render the password form (the
      // classic lane) rather than a dead page.
      methods.value = { password: true, providers: [] };
    });
});

function signInWith(providerId: string) {
  window.location.href = `/api/v1/auth/oauth/${encodeURIComponent(providerId)}`;
}

async function onSubmit(username: string, password: string) {
  loading.value = true;
  error.value = null;
  try {
    await auth.login(username, password);
    const redirect = typeof route.query.redirect === "string" ? route.query.redirect : "/";
    await router.push(redirect);
  } catch (err) {
    error.value =
      err instanceof ApiError && err.status === 403
        ? t("common.auth.passwordDisabled")
        : err instanceof ApiError && err.status === 401
          ? t("common.auth.failed")
          : err instanceof Error
            ? err.message
            : t("common.status.error");
  } finally {
    loading.value = false;
  }
}
</script>

<template>
  <div class="login-face">
    <div class="login-card">
      <HkSignInCard
        v-if="methods?.password"
        :title="t('common.auth.title')"
        :subtitle="t('common.auth.subtitle')"
        logo-src="/logo.webp"
        :loading="loading"
        @submit="onSubmit"
      >
        <template v-if="methods?.providers?.length" #methods>
          <p class="or-continue">{{ t("common.auth.orContinue") }}</p>
          <div class="provider-buttons">
            <button
              v-for="provider in methods.providers"
              :key="provider.id"
              type="button"
              class="provider-button"
              @click="signInWith(provider.id)"
            >
              <KeyRound :size="14" />
              <span>{{ provider.display_name }}</span>
            </button>
          </div>
        </template>
      </HkSignInCard>

      <!-- Password disabled: the providers ARE the page. -->
      <div v-else-if="methods" class="providers-only">
        <h1 class="providers-title">{{ t("common.auth.title") }}</h1>
        <p v-if="error" class="login-error" role="alert">{{ error }}</p>
        <div class="provider-buttons">
          <button
            v-for="provider in methods.providers"
            :key="provider.id"
            type="button"
            class="provider-button"
            @click="signInWith(provider.id)"
          >
            <KeyRound :size="14" />
            <span>{{ provider.display_name }}</span>
          </button>
        </div>
      </div>

      <p v-if="error && methods?.password" class="login-error" role="alert">{{ error }}</p>
    </div>
    <!-- Sessionless surface: the health probe + the idle (gray) light —
         a WS probe here would hang red forever (no token yet). -->
    <div class="login-status">
      <ConnectionStatusTag source="health" idle />
    </div>
  </div>
</template>

<style scoped lang="scss">
.login-face {
  min-height: 100vh;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 1.5rem;
  background:
    linear-gradient(
      color-mix(in srgb, rgb(0 0 0 / 35%), transparent),
      color-mix(in srgb, rgb(0 0 0 / 35%), transparent)
    ),
    url("/bg.webp") center / cover no-repeat fixed;
}

.login-card {
  width: min(24rem, 100%);
}

.login-error {
  margin: 0.75rem 0 0;
  padding: 0.5rem 0.75rem;
  border-radius: 0.5rem;
  background: color-mix(in srgb, rgb(220 38 38) 14%, transparent);
  color: rgb(220 38 38);
  font-size: 0.875rem;
  text-align: center;
}

.or-continue {
  margin: 0.25rem 0 0.5rem;
  font-size: 0.78rem;
  opacity: 0.65;
  text-align: center;
}

.providers-only {
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
  padding: 1.5rem;
  border-radius: 1rem;
  background: color-mix(in srgb, rgb(255 255 255 / 94%) 100%, transparent);
  color: #1f2937;
}

:global([data-mode="dark"]) .providers-only {
  background: color-mix(in srgb, rgb(17 24 39 / 94%) 100%, transparent);
  color: #e5e7eb;
}

.providers-title {
  margin: 0;
  font-size: 1.25rem;
  text-align: center;
}

.provider-buttons {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.provider-button {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 0.5rem;
  width: 100%;
  padding: 0.6rem 1rem;
  border-radius: 0.6rem;
  border: 1px solid color-mix(in srgb, currentColor 25%, transparent);
  background: transparent;
  color: inherit;
  font-size: 0.9rem;
  cursor: pointer;

  &:hover {
    background: color-mix(in srgb, currentColor 8%, transparent);
  }
}

.login-status {
  position: fixed;
  inset-inline-start: 0.75rem;
  inset-block-end: 0.75rem;
  z-index: 20;
}
</style>
