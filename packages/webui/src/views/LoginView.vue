<script setup lang="ts">

import { HkSignInCard } from "@celestia-island/hikari";
import ConnectionStatusTag from "@/components/ConnectionStatusTag.vue";
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";

import { ApiError } from "@/api/client";
import { useAuthStore } from "@/stores/auth";

/**
 * The sign-in face: hikari's shared credential card (identical visual
 * language across the family), bound to /api/v1/auth/login. The card
 * owns field validation (empty-field guard, caps-lock hints); this view
 * owns the request, the loading state and the error surface.
 */

const { t } = useI18n();
const router = useRouter();
const route = useRoute();
const auth = useAuthStore();

const loading = ref(false);
const error = ref<string | null>(null);

async function onSubmit(username: string, password: string) {
  loading.value = true;
  error.value = null;
  try {
    await auth.login(username, password);
    const redirect = typeof route.query.redirect === "string" ? route.query.redirect : "/";
    await router.push(redirect);
  } catch (err) {
    error.value =
      err instanceof ApiError && err.status === 401
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
        :title="t('common.auth.title')"
        :subtitle="t('common.auth.subtitle')"
        logo-src="/logo.webp"
        :loading="loading"
        @submit="onSubmit"
      />
      <p v-if="error" class="login-error" role="alert">{{ error }}</p>
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

.login-status {
  position: fixed;
  inset-inline-start: 0.75rem;
  inset-block-end: 0.75rem;
  z-index: 20;
}
</style>
