import {
  HkEmptyState,
  HkLoginFlow,
  HkStatusBar,
  useToast,
  type HkLoginChannel,
} from "@celestia-island/hikari";
import { KeyRound, Unplug } from "lucide-vue-next";
import { computed, h, defineComponent, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";

import * as api from "@/api";
import { ApiError } from "@/api/client";
import { useEngineHealth } from "@/composables/useEngineHealth";
import { useAuthStore } from "@/stores/auth";
import type { AccountMethods, LoginMethods } from "@/types";
import { makeErrorText } from "@/composables/errorText";
import { verifyLadder } from "@/composables/verifyLadder";

import "./LoginView.scss";

/** The open-sign-in channel every deployment has, whatever the password
 *  switch says: the account-first funnel starts by collecting a name, and
 *  that name is the raw material for BOTH proofs (the password and the
 *  linked providers), so the funnel must exist before either does. */
const ACCOUNT_CHANNEL = "account";

/** The provider type that rides its own segment, not a row: the family's
 *  central sign-in (auth.cw's "组织登录" counterpart). It only qualifies
 *  when the descriptor holds EXACTLY one of it — two of them have no
 *  principled winner, so both fall back to the ordinary lanes. */
const PEER_PROVIDER_TYPE = "builtin:erp";

/**
 * The sign-in face: the family's ACCOUNT-FIRST ladder, in two steps.
 *
 * Step 1 (`account`) collects a user name only. Step 2 (`verify`) shows
 * how THAT account can sign in, and any one of those paths finishes the
 * login — "pick one or two, pass either":
 *
 *  - `/auth/account-methods` answers with the account's factors;
 *  - a password factor becomes hikari's verify card (the single factor a
 *    page can PROVE without navigating away);
 *  - a linked-provider factor becomes a row that starts that provider's
 *    OAuth flow (a full-page navigation — the flow plants its state cookie
 *    and redirects, which a fetch cannot carry through).
 *
 * Everything visual is hikari's `HkLoginFlow`; this file owns only the
 * ladder state and the wiring.
 *
 * ## Why the name is collected before the factor list
 *
 * The classic card asks for the password first, which is wrong the moment
 * an account has no password: an OAuth-only user had to guess that a
 * provider row was the way in. Asking the name first makes the account's
 * real options visible before any credential is typed — and the server
 * answers unknown names exactly like password accounts (see
 * `auth::account_factors`), so this costs no enumeration privacy.
 *
 * ## The two lanes are PEERS, not a form with alternatives under it
 *
 *  - the account funnel (always present — it is the ladder itself);
 *  - one SSO lane per enabled provider when the password switch is OFF,
 *    because then there is no account funnel to hang the rows under;
 *  - the Celestia ID peer lane when exactly one `builtin:erp` provider is
 *    configured — the family's stand-alone central sign-in for someone who
 *    has not typed an account name yet.
 *
 * The segment group renders only for two or more channels (hikari's own
 * rule: a lone dead segment reads as a broken switch).
 */
export default defineComponent({
  name: "LoginView",
  setup() {
    const { t } = useI18n();
    const toast = useToast();

    const errorText = makeErrorText(t);
    const router = useRouter();
    const route = useRoute();
    const auth = useAuthStore();

    const methods = ref<LoginMethods | null>(null);
    /** The account's factors, fetched on the first submit. */
    const account = ref<AccountMethods | null>(null);
    /** Whether the name went into `account` (false = the probe is still
     *  in flight). The name is ALSO the authorisation for the password
     *  proof: see `onVerify`. */
    const probed = ref(false);
    const { health } = useEngineHealth();
    const appVersion: string = __APP_VERSION__;
    const channel = ref("");
    const loading = ref(false);
    /** The in-flight state of the step-1 probe, separate from the password
     *  submit: the two ride different buttons. */
    const probing = ref(false);

    /** Provider marks are loose-typed by the family (a brand SVG or <img>);
     * 0721 ships no brand assets, so every SSO row rides the same neutral
     * key mark and the LABEL names the destination. */
    function providerIcon() {
      return h(KeyRound, { size: 16 });
    }

    const peerProvider = computed(() => {
      const candidates = providers.value.filter((provider) => provider.type === PEER_PROVIDER_TYPE);
      // Singular by construction: with TWO erp providers neither owns the
      // "Celestia ID" tab (the rows name them, the ordinal disambiguates).
      return candidates.length === 1 ? candidates[0] : undefined;
    });

    /** The descriptor's providers. Read through a plain getter (not a
     *  cached computed) so `channels` always sees the current array. */
    const providers = computed(() => methods.value?.providers ?? []);

    const channels = computed<HkLoginChannel[]>(() => {
      const offered = providers.value;
      const password = methods.value?.password ?? false;
      const peer = peerProvider.value;
      // By KEY, never by object identity: `offered` holds fresh copies,
      // so `provider !== peer` would be true for every row and the peer
      // would appear twice (a stand-alone lane AND a row under the card).
      const others = offered.filter((provider) => provider.id !== peer?.id);
      const list: HkLoginChannel[] = [];

      list.push({
        key: ACCOUNT_CHANNEL,
        mode: "credentials" as const,
        label: t("common.auth.accountChannel"),
        title: t("common.auth.title"),
        subtitle: t("common.auth.subtitle"),
        usernameType: "text",
        usernameAutocomplete: "username",
        // Account-first: the opening card must NOT ask for a password
        // (hikari's own default since the MFA redesign). The password is
        // proven on the verify step as one factor among the announced ones.
        passwordField: false,
        submitLabel: t("common.auth.submit"),
        // A password-off deployment's account funnel has no password to
        // prove, so the providers ride the card as rows (the family home
        // for provider names) instead of becoming stand-alone lanes.
        methods: password || others.length === 0
          ? []
          : others.map((provider) => ({
              key: provider.id,
              label: provider.display_name,
              icon: providerIcon(),
            })),
        methodsDivider: password || others.length === 0 ? undefined : t("common.auth.orContinue"),
      });

      if (!password) {
        // No password lane at all: every provider must stay reachable
        // without a password proof, so each is a whole-flow channel.
        // Their tabs stay panel copy — never the server's display_name —
        // but a tab that repeats is a dead switch, so when the same type
        // appears more than once the panel disambiguates with its own
        // ordinal ("Single sign-on · 1"), not with remote text.
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

    /** Step 2 is the ladder's answer, not a host-side guess: the card
     *  moves to the verify step only once an account's factors are known. */
    const step = computed<"account" | "verify">(() =>
      probed.value && channel.value === ACCOUNT_CHANNEL ? "verify" : "account",
    );

    const ladder = computed(() =>
      account.value
        ? verifyLadder(account.value)
        : { provePassword: false, methods: [] },
    );

    /** The verify card's factor list. ONE `password` at most: it is the
     *  only factor the card can prove in place, so announcing it twice
     *  would be a duplicate picker entry. */
    const verifyFactors = computed<string[]>(() =>
      ladder.value.provePassword ? ["password"] : [],
    );

    /** The verify step's alternative rows, labelling each linked provider
     *  the way the server named it. A provider the deployment no longer
     *  offers still gets a row (the account really has the binding), but
     *  it is disabled: its OAuth entry would refuse, and a live-looking
     *  row that 404s is worse than an obviously unavailable one. */
    const verifyMethods = computed(() => {
      const labels = new Map<string, string>();
      for (const provider of methods.value?.providers ?? []) {
        labels.set(provider.id, provider.display_name);
      }
      for (const factor of account.value?.factors ?? []) {
        if (factor.kind === "oauth") labels.set(factor.provider, factor.label);
      }
      const offered = new Set((methods.value?.providers ?? []).map((provider) => provider.id));
      return ladder.value.methods.map((provider) => ({
        key: provider,
        label: labels.get(provider) || provider,
        icon: providerIcon(),
        disabled: !offered.has(provider),
      }));
    });

    /** What the page is verifying — the verify card's subtitle input. */
    const accountName = ref("");

    function reset() {
      account.value = null;
      probed.value = false;
      accountName.value = "";
    }

    function signInWith(providerId: string): void {
      window.location.href = `/api/v1/auth/oauth/${encodeURIComponent(providerId)}`;
    }

    function reportSignInFailure(err: unknown): void {
      // The family feedback contract: transient attempt feedback is a
      // toast, never DOM inserted beside the card.
      toast.error(
        err instanceof ApiError && err.status === 403
          ? t("common.auth.passwordDisabled")
          : err instanceof ApiError && err.status === 401
            ? t("common.auth.failed")
            : errorText(err),
      );
    }

    /** Step 1 → step 2: ask the server which factors this account has. */
    async function probeAccount(username: string): Promise<void> {
      probing.value = true;
      try {
        account.value = await api.accountMethods(username);
        accountName.value = username;
        probed.value = true;
      } catch (err) {
        // The ladder is unavailable; say so once and stay on step 1 (the
        // name is still in the field, so a retry costs nothing).
        toast.error(errorText(err));
      } finally {
        probing.value = false;
      }
    }

    async function onAccountSubmit(
      submitted: string,
      username: string,
      password: string,
    ): Promise<void> {
      // A stand-alone provider lane submitted nothing to prove; its own
      // events (sso-click / method-select) carry the navigation.
      if (submitted !== ACCOUNT_CHANNEL) return;
      // Step 2 of the SAME card (a host-side re-entry of the account form
      // while the ladder is open): treat it as a fresh probe.
      reset();
      if (password) {
        // A two-field card would land here with a password already typed;
        // account-first never asks for it on step 1, so this is the "the
        // channel declaration changed underneath us" path.
        await auth
          .login(username, password)
          .then(() => completeSignIn())
          .catch(reportSignInFailure);
        return;
      }
      await probeAccount(username);
    }

    /** Step 2, the in-page proof: the password factor. */
    async function onVerify(_kind: string, proof: string): Promise<void> {
      // The proof is only valid for the account the ladder was opened
      // for — the name is the authorisation for the password submit, so
      // empty it refuses rather than guessing.
      if (!accountName.value) return;
      loading.value = true;
      try {
        await auth.login(accountName.value, proof);
        await completeSignIn();
      } catch (err) {
        reportSignInFailure(err);
      } finally {
        loading.value = false;
      }
    }

    async function completeSignIn(): Promise<void> {
      const redirect = typeof route.query.redirect === "string" ? route.query.redirect : "/";
      await router.push(redirect);
    }

    /** Step 2 → step 1 (hikari's "back" affordance). The typed name is
     *  kept: `accountName` seeds the credentials card so the round trip
     *  costs no retyping. */
    function onVerifyBack(): void {
      const keep = accountName.value;
      reset();
      accountName.value = keep;
    }

    function onSsoClick(channelKey: string): void {
      signInWith(channelKey);
    }

    /** An alternative-method row under the account card: the same
     *  full-page redirect as a peer channel. */
    function onMethodSelect(_channel: string, providerId: string): void {
      signInWith(providerId);
    }

    onMounted(() => {
      api
        .loginMethods()
        .then((payload) => {
          methods.value = payload;
          // The account funnel always opens: with the switch off it still
          // lists the providers' rows, so it stays the sane default.
          channel.value = ACCOUNT_CHANNEL;
        })
        .catch(() => {
          // The descriptor is unreachable: render the ladder with the
          // local default — password ON, no providers — rather than a dead
          // page (the server still enforces the switch, and the ladder's
          // own probe reports what the account really has).
          methods.value = {
            password: true,
            providers: [],
            policy: {
              version: 1,
              mode: "local",
              password: true,
              methods: [],
            },
          };
          channel.value = ACCOUNT_CHANNEL;
        });
    });


    /** The verify step's alternative rows, as hikari's `verifyMethods`
     *  prop takes them. A row is NOT a factor (it never enters the factor
     *  picker and never emits `verify`): picking one hands the whole
     *  sign-in to the provider, which is the "or continue with…" half of
     *  the any-one-path rule. */
    function onVerifyMethodSelect(providerId: string): void {
      signInWith(providerId);
    }

    return () => (
      <div class="login-face">
        <div class="login-stage">
          <div class="login-panel">
            {hasChannels.value ? (
              <HkLoginFlow
                modelValue={channel.value}
                onUpdate:modelValue={(v: string) => (channel.value = v)}
                channels={channels.value}
                step={step.value}
                loading={loading.value || probing.value}
                logoSrc="/logo.webp"
                title={t("common.auth.title")}
                subtitle={t("common.auth.subtitle")}
                initialUsername={accountName.value}
                verifyFactors={verifyFactors.value}
                verifyMethods={verifyMethods.value}
                verifyMethodsDivider={
                  verifyMethods.value.length > 0 ? t("common.auth.orProvider") : undefined
                }
                verifyAccountName={accountName.value}
                verifyLoading={loading.value}
                onAccount-submit={onAccountSubmit}
                onSso-click={onSsoClick}
                onMethod-select={onMethodSelect}
                onVerify={onVerify}
                onVerify-back={onVerifyBack}
                onVerify-method-select={onVerifyMethodSelect}
              />
            ) : methods.value ? (
              // No channels at all: an honest dead state. The account
              // funnel is unconditional, so this needs the catch lane to
              // have failed too.
              <HkEmptyState
                boxed
                icon={Unplug}
                title={t("common.status.empty")}
                description={t("common.auth.noMethods")}
              />
            ) : null}
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
