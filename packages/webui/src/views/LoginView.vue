<script setup lang="ts">
import {
  HkEmptyState,
  HkLoginFlow,
  type HkAuthChannelListEntry,
  type HkLoginChannel,
} from "@celestia-island/hikari";
import { KeyRound, Unplug } from "lucide-vue-next";
import { computed, h, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";

import * as api from "@/api";
import { ApiError } from "@/api/client";
import ConnectionStatusTag from "@/components/ConnectionStatusTag.vue";
import { useAuthStore } from "@/stores/auth";
import type { LoginMethods } from "@/types";

/**
 * The sign-in face, driven by `GET /api/v1/auth/login-methods`.
 *
 * It is the FAMILY panel (`HkLoginFlow`), not a hand-built card: the
 * same component chest, the ERP faces and the mail console mount. The
 * descriptor becomes the channel list —
 *
 *  - the password kill-switch on  → one credentials channel (the
 *    account + password card);
 *  - every enabled provider       → one SSO channel, whose whole flow
 *    is a single full-width row that navigates the WHOLE page to the
 *    redirect entry (the flow plants an httpOnly state cookie and 302s,
 *    which a fetch cannot carry through).
 *
 * The segment group renders only for two or more channels, so a
 * deployment configured for Celestia ID alone shows exactly one panel
 * with no dead segment — the shape the kill-switch is meant to produce.
 */

const { t } = useI18n();
const router = useRouter();
const route = useRoute();
const auth = useAuthStore();

const methods = ref<LoginMethods | null>(null);
const channel = ref("");
const loading = ref(false);
const error = ref<string | null>(null);

/** Provider marks are loose-typed by the family (a brand SVG or <img>);
 * 0721 ships no brand assets, so every SSO row rides the same neutral
 * key mark and the LABEL names the destination. */
function providerIcon() {
  return h(KeyRound, { size: 16 });
}

/** Provider rows for the credentials card's alternative-methods list —
 * the family shape: a provider is a ROW under the form, not a channel
 * tab (a provider name in the channel segment read as "log in to ERP"
 * instead of "log in with your organization"). */
const providerRows = computed<HkAuthChannelListEntry[]>(() =>
  (methods.value?.providers ?? []).map((provider) => ({
    key: provider.id,
    label: provider.display_name,
    icon: providerIcon(),
  })),
);

const channels = computed<HkLoginChannel[]>(() => {
  if (methods.value?.password) {
    // The classic lane: the credential card, with every enabled provider
    // as an alternative row beneath the form (`method-select`).
    return [
      {
        key: "password",
        mode: "credentials",
        label: t("common.auth.passwordChannel"),
        title: t("common.auth.title"),
        subtitle: t("common.auth.subtitle"),
        usernameType: "text",
        usernameAutocomplete: "username",
        passwordField: true,
        submitLabel: t("common.auth.submit"),
        methods: providerRows.value,
        methodsDivider: providerRows.value.length > 0 ? t("common.auth.orContinue") : undefined,
      },
    ];
  }
  // Password off: each provider's whole sign-in IS one redirect, so it
  // rides an sso channel. A single provider therefore renders a lone
  // full-width row with no segment group at all — the Celestia-ID-only
  // shape. Only a multi-provider deployment grows the segment group, and
  // there the provider names are the honest labels for the choice.
  return (methods.value?.providers ?? []).map((provider) => ({
    key: provider.id,
    mode: "sso" as const,
    label: provider.display_name,
    title: t("common.auth.title"),
    subtitle: t("common.auth.subtitle"),
    icon: providerIcon(),
    buttonLabel: provider.display_name,
  }));
});

const hasChannels = computed(() => channels.value.length > 0);

onMounted(() => {
  api
    .loginMethods()
    .then((payload) => {
      methods.value = payload;
      // Open on the first offered channel: the password card when the
      // kill-switch allows it, otherwise the provider row.
      channel.value = payload.password ? "password" : (payload.providers[0]?.id ?? "");
    })
    .catch(() => {
      // The descriptor is unreachable: render the classic password lane
      // rather than a dead page (the server still enforces the switch).
      methods.value = { password: true, providers: [] };
      channel.value = "password";
    });
});

function signInWith(providerId: string): void {
  window.location.href = `/api/v1/auth/oauth/${encodeURIComponent(providerId)}`;
}

async function onAccountSubmit(_channel: string, username: string, password: string): Promise<void> {
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

function onSsoClick(channelKey: string): void {
  signInWith(channelKey);
}

/** An alternative-method row under the credential card: same full-page
 * redirect as an sso channel. */
function onMethodSelect(_channel: string, providerId: string): void {
  signInWith(providerId);
}
</script>

<template>
  <div class="login-face">
    <div class="login-panel">
      <HkLoginFlow
        v-if="hasChannels"
        v-model="channel"
        :channels="channels"
        step="account"
        :loading="loading"
        logo-src="/logo.webp"
        :title="t('common.auth.title')"
        :subtitle="t('common.auth.subtitle')"
        @account-submit="onAccountSubmit"
        @sso-click="onSsoClick"
        @method-select="onMethodSelect"
      />
      <!-- No password and no provider: the honest dead state (the server
           guards the kill-switch, so this is unreachable in practice). -->
      <HkEmptyState
        v-else-if="methods"
        boxed
        :icon="Unplug"
        :title="t('common.status.empty')"
        :description="t('common.auth.noMethods')"
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

.login-panel {
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