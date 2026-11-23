import type { Composer } from "vue-i18n";

/**
 * The never-empty failure text factory (the family rosters' shared lane
 * helper, extracted from the four views that each carried a copy).
 *
 * Why "never empty" is the contract: every consumer of these values is a
 * FALSY-GATED surface — a scaffold `error`/`staleError` prop, a
 * `v-if="someError"` paragraph, a dialog note. An empty string therefore
 * does not render an EMPTY message; it renders NO message at all, and a
 * real failure reads as an untouched page ("no rows yet" over a dead
 * load, a dialog closing over a failed delete). The transport layer's
 * own errors always carry a message, but test doubles, partial
 * failures and future lanes must not be able to reintroduce the hole.
 *
 * Shape coverage (each pinned somewhere in the suite):
 *  - `Error` with a non-empty message → the message;
 *  - `Error("")` → the localized fallback (NOT an empty string);
 *  - a non-empty bare string (a JSON-RPC lane's common rejection) → the
 *    string itself;
 *  - anything else (undefined, null, objects) → the localized fallback.
 */
export function makeErrorText(
  t: Composer["t"],
  fallbackKey = "common.status.error",
): (e: unknown) => string {
  return (e: unknown): string => {
    if (e instanceof Error && e.message) return e.message;
    if (typeof e === "string" && e.length > 0) return e;
    return t(fallbackKey);
  };
}