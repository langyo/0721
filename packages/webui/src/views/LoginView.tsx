import {
  HkEmptyState,
  HkLoginFlow,
  HkStatusBar,
  useToast,
  type HkLoginChannel,
} from "@celestia-island/hikari";
import { KeyRound, Unplug } from "lucide-vue-next";
import { computed, defineComponent, h, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import * as api from "@/api";
import { ApiError } from "@/api/client";
import { useEngineHealth } from "@/composables/useEngineHealth";
import { useAuthStore } from "@/stores/auth";
import type { LoginMethods } from "@/types";
import { makeErrorText } from "@/composables/errorText";

import "./LoginView.css";

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
 *
 * Sign-in failures surface as TOASTS (the family feedback contract):
 * nothing about the attempt inserts DOM beside the card.
 */
export default defineComponent({
  name: "LoginView",
  setup() {
    const { t } = useI18n();
    const errorText = makeErrorText(t);
    const toast = useToast();
    const router = useRouter();
    const route = useRoute();
    const auth = useAuthStore();
    const methods = ref<LoginMethods | null>(null);
    const { health } = useEngineHealth();
    const appVersion: string = __APP_VERSION__;
    const channel = ref("");
    const loading = ref(false);
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
      try {
        await auth.login(username, password);
        const redirect = typeof route.query.redirect === "string" ? route.query.redirect : "/";
        await router.push(redirect);
      } catch (err) {
        // The family feedback contract: transient attempt feedback is a
        // toast, never DOM inserted beside the card.
        toast.error(
          err instanceof ApiError && err.status === 403
            ? t("common.auth.passwordDisabled")
            : err instanceof ApiError && err.status === 401
              ? t("common.auth.failed")
              : errorText(err),
        );
      } finally {
        loading.value = false;
      }
    }
    function onSsoClick(channelKey: string): void {
      signInWith(channelKey);
    }
    /** An alternative-method row under the account card: the same full-page
     * redirect as a peer channel. */
    function onMethodSelect(_channel: string, providerId: string): void {
      signInWith(providerId);
    }
    return () => (
      <div class="login-face">
        <div class="login-stage">
          <div class="login-panel">
            {hasChannels.value && (
              <HkLoginFlow
                modelValue={channel.value}
                onUpdate:modelValue={(v: string) => { channel.value = v; }}
                channels={channels.value}
                step="account"
                loading={loading.value}
                logoSrc="/logo.webp"
                title={t("common.auth.title")}
                subtitle={t("common.auth.subtitle")}
                onAccount-submit={onAccountSubmit}
                onSso-click={onSsoClick}
                onMethod-select={onMethodSelect}
              />
            )}
            {/* No password and no provider: the honest dead state. It needs a
                live descriptor replying with neither method — the catch lane
                above renders the classic password card instead. */}
            {!hasChannels.value && methods.value && (
              <HkEmptyState
                boxed
                icon={Unplug}
                title={t("common.status.empty")}
                description={t("common.auth.noMethods")}
              />
            )}
          </div>
        </div>
        {/* The family footer strip: panel + engine identity and the health
            light. Sessionless, so the connection state stays idle (a WS
            probe here would hang red forever — no token yet). */}
        <footer class="login-footer">
          {/* standalone=false: the strip is this face's own footer element;
               the default renders its own fixed <footer> and would nest. */}
          <HkStatusBar
            standalone={false}
            version={appVersion}
            engineVersion={health.value?.engineVersion ?? null}
            connectionStatus="idle"
          />
        </footer>
      </div>
    );
  },
});

