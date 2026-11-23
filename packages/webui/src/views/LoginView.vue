<script setup lang="ts">
import {
  HkEmptyState,
  HkLoginFlow,
  HkStatusBar,
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
 * channel/step semantics come from the upstream component rather than
 * from this file. The descriptor becomes the channel list, and the lanes
 * are PEERS — not a form with alternatives tucked beneath it:
 *
 *  - the password kill-switch on  → one credentials channel (the
 *    account + password card);
 *  - every enabled provider       → one SSO channel, whose whole flow
 *    is a single full-width row that navigates the WHOLE page to the
 *    redirect entry (the flow plants an httpOnly state cookie and 302s,
 *    which a fetch cannot carry through).
 *
 * The segment group renders only for two or more channels (the
 * component's own rule — a lone dead segment reads as a broken switch),
 * so a deployment configured for Celestia ID alone shows exactly one
 * panel with no segment at all. A deployment with both shows the switch,
 * and the card body crossfades between the lanes.
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

/** Segment labels are the PANEL's own copy — hikari's locked copy rule
 * ("segment labels are ALWAYS the panel's own locale keys, never a
 * server-provided provider name": a leaked provider label read as "log
 * in to ERP" instead of "log in with your organization"). The key below
 * is the closed provider-TYPE enum, so this table is our code, not
 * remote data; the server's `display_name` stays where it belongs — on
 * the channel's row (`buttonLabel`). */
/** The provider type that rides its own segment, not a row: the family's
 * central sign-in (auth.cw's "组织登录" counterpart). It only qualifies
 * when the descriptor holds EXACTLY one of it — two of them have no
 * principled winner, so both fall back to the ordinary lanes. Everything
 * else is an alternative under the account card, which is where hikari
 * puts provider names — rows, never segments. */
const PEER_PROVIDER_TYPE = "builtin:erp";

/** Provider marks are loose-typed by the family (a brand SVG or <img>);
 * 0721 ships no brand assets, so every SSO row rides the same neutral
 * key mark and the LABEL names the destination. */
function providerIcon() {
  return h(KeyRound, { size: 16 });
}

const channels = computed<HkLoginChannel[]>(() => {
  const providers = methods.value?.providers ?? [];
  const password = methods.value?.password ?? false;
  // Celestia ID is the peer lane; every other provider is an alternative
  // row under the account card (hikari: a provider name in the segment
  // reads as "log in to ERP" rather than "log in with your organization").
  // Singular by construction: with TWO erp providers neither owns the
  // "Celestia ID" tab — a deployment that duplicated the peer type gets
  // the ordinary lanes instead of one arbitrary winner and one generic
  // tab (the ordinal disambiguates, the rows name them).
  const peerCandidates = providers.filter(
    (provider) => provider.type === PEER_PROVIDER_TYPE,
  );
  const peer = peerCandidates.length === 1 ? peerCandidates[0] : undefined;
  const others = providers.filter((provider) => provider !== peer);
  const list: HkLoginChannel[] = [];

  if (password) {
    list.push({
      key: "password",
      mode: "credentials",
      label: t("common.auth.passwordChannel"),
      title: t("common.auth.title"),
      subtitle: t("common.auth.subtitle"),
      usernameType: "text",
      usernameAutocomplete: "username",
      passwordField: true,
      submitLabel: t("common.auth.submit"),
      // Rows under the card: the family home for provider names.
      methods: others.map((provider) => ({
        key: provider.id,
        label: provider.display_name,
        icon: providerIcon(),
      })),
      methodsDivider:
        others.length > 0 ? t("common.auth.orContinue") : undefined,
    });
  } else {
    // No password lane: every provider must be reachable on its own, so
    // each is a whole-flow channel. Their tabs stay panel copy — never the
    // server's display_name — but a tab that repeats is a dead switch, so
    // when the same type appears more than once the panel disambiguates
    // with its own ordinal ("Single sign-on · 1"), not with remote text.
    const repeated = others.length > 1;
    others.forEach((provider, index) => {
      list.push({
        key: provider.id,
        mode: "sso" as const,
        label: repeated
          ? `${t("common.auth.ssoChannel")} · ${index + 1}`
          : t("common.auth.ssoChannel"),
        title: t("common.auth.title"),
        subtitle: t("common.auth.subtitle"),
        icon: providerIcon(),
        buttonLabel: provider.display_name,
      });
    });
  }

  if (peer) {
    list.push({
      key: peer.id,
      mode: "sso" as const,
      // Panel copy, never the server's display_name (locked copy rule).
      label: t("common.auth.celestiaChannel"),
      title: t("common.auth.title"),
      subtitle: t("common.auth.subtitle"),
      icon: providerIcon(),
      buttonLabel: peer.display_name,
    });
  }

  return list;
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

/** An alternative-method row under the account card: the same full-page
 * redirect as a peer channel (the rows exist again now that non-peer
 * providers ride the card, so this handler must exist with them). */
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
