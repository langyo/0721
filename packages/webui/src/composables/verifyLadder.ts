import type { AccountMethods, LoginFactor } from "@/types";

/**
 * The verify-step ladder an account's factors become.
 *
 * `password` — the only factor this card can PROVE without leaving the
 * page (hikari's verify body is a single-proof card).
 * `methods` — the providers the account can still sign in through, as
 * rows that navigate the whole page to the OAuth entry.
 */
export interface VerifyLadder {
  provePassword: boolean;
  methods: string[];
}

/**
 * Fold the server's factor list into the two shapes hikari's verify step
 * can render.
 *
 * Two normalisations, both defensive against a contract drift the webui
 * must not turn into a dead end:
 *
 * 1. **A response with no factor at all still offers the password.** The
 *    server promises at least one factor (that promise is what keeps the
 *    endpoint from being an enumeration oracle — an empty list would be
 *    the one shape meaning "no such account"). An older server, a proxy
 *    rewriting the body, or a future contract change must not leave the
 *    user staring at a card with nothing to do: the password row is the
 *    same baseline the server would have sent, and submitting it simply
 *    fails at `/auth/login` if that guess was wrong.
 * 2. **Unknown factor kinds are ignored**, not rendered. `kind` is a
 *    closed set today (`password` / `oauth`); a future third kind would
 *    otherwise put an unlabelled row on the card.
 *
 * Semantic cut worth stating: a `kind: "password"` factor is the fact
 * "this account carries a usable password hash", not the deployment-wide
 * `password` flag — the flag rides the response separately so the ladder
 * can tell the two apart, and the flag is TRUE for unknown names too.
 */
export function verifyLadder(payload: AccountMethods): VerifyLadder {
  const factors: LoginFactor[] = Array.isArray(payload.factors) ? payload.factors : [];
  const provePassword = factors.some((factor) => factor.kind === "password");
  const methods: string[] = [];
  for (const factor of factors) {
    if (factor.kind !== "oauth") continue;
    const provider = factor.provider ?? "";
    // A row with no provider id has nowhere to navigate; a repeat is the
    // same destination twice.
    if (!provider || methods.includes(provider)) continue;
    methods.push(provider);
  }
  return {
    provePassword: provePassword || methods.length === 0,
    methods,
  };
}
