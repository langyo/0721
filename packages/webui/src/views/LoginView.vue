<script setup lang="ts">
import {
  HkEmptyState,
  HkLoginFlow,
  HkStatusBar,
  type HkAuthChannelListEntry,
  type HkLoginChannel,
} from "@celestia-island/hikari";
import { KeyRound, Unplug } from "lucide-vue-next";
import { computed, h, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";

import * as api from "@/api";
import { ApiError } from "@/api/client";
import { useEngineHealth } from "@/composables/useEngineHealth";
import { useAuthStore } from "@/stores/auth";
import type { LoginMethods } from "@/types";
import { makeErrorText } from "@/composables/errorText";

/**
 * The sign-in face, driven by `GET /api/v1/auth/login-methods`.
 *
 * It is hikari's `HkLoginFlow` panel, not a hand-built card, so the
 * channel/step semantics (and the copy rules the component spells out in
 * its own docstring — provider names belong on rows, never in the channel
 * segment) come from the upstream component rather than from this file.
 * The descriptor becomes the channel list —
 *
 *  - the password kill-switch on  → one credentials channel (the
 *    account + password card);
 *  - every enabled provider       → one SSO channel, whose whole flow
 *    is a single full-width row that navigates the WHOLE page to the
 *    redirect entry (the flow plants an httpOnly state cookie and 302s,
 *    which a fetch cannot carry through).
 *
 * The segment group renders only for two or more channels (the
 * component's own rule), so a deployment configured for Celestia ID
 * alone shows exactly one panel with no segment at all.
 */

const { t } = useI18n();

const errorText = makeErrorText(t);
const router = useRouter();
const route = useRoute();
const auth = useAuthStore();

const methods = ref<LoginMethods | null>(null);
const { health } = useEngineHealth();
const appVersion: string = __APP_VERSION__;
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
          : errorText(err);
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
    <div class="login-stage">
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
      <!-- No password and no provider: the honest dead state. It needs a
           live descriptor replying with neither method — the catch lane
           above renders the classic password card instead. -->
      <HkEmptyState
        v-else-if="methods"
        boxed
        :icon="Unplug"
        :title="t('common.status.empty')"
        :description="t('common.auth.noMethods')"
      />

      <p v-if="error" class="login-error" role="alert">{{ error }}</p>
    </div>
    </div>

    <!-- The family footer strip: panel + engine identity and the health
         light. Sessionless, so the connection state stays idle (a WS
         probe here would hang red forever — no token yet). -->
    <footer class="login-footer">
      <!-- standalone=false: the strip is this face's own footer element;
           the default renders its own fixed <footer> and would nest. -->
      <HkStatusBar
        :standalone="false"
        :version="appVersion"
        :engine-version="health?.engineVersion ?? null"
        connection-status="idle"
      />
    </footer>
  </div>
</template>

<style scoped lang="scss">
.login-face {
  min-height: 100vh;
  display: flex;
  flex-direction: column;
  // The family face surface (auth.cw's plain backdrop) — hikari's theme
  // variable, so light/dark follows the shell.
  background: rgb(var(--color-surface));
}

.login-stage {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 1.5rem;
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

.login-footer {
  display: flex;
  align-items: center;
  padding: 0.375rem 0.75rem;
}
</style>
