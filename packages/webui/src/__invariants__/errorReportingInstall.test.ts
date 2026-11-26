import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { createErrorReporting, SERVER_ERROR_LANDING_ID } from "@celestia-island/hikari";
import { afterEach, describe, expect, it } from "vitest";
import { createApp } from "vue";

/**
 * The bootstrap installs the family error landing — and installs it before
 * `mount`.
 *
 * Order is the whole point. `createErrorReporting().install` is what reads
 * the server's `hk-server-error` data island; a boot that installs it
 * AFTER `app.mount("#app")` paints the host tree (or lets the router guard
 * redirect) before the landing can take over, so the visitor sees a flash
 * of the wrong page — or, for an island whose card is meant to be the
 * whole document, never sees it at all. Nothing else in the suite can
 * catch that: every view test mounts its own app and never runs
 * `main.ts`.
 *
 * Read as source rather than executed: `main.ts` is the mount entry
 * (`app.mount("#app")` needs a real `#app`, a router, an auth restore and
 * the i18n singletons), so the pin is the ORDER of two statements in that
 * file — plus a live install of the same plugin, so the assertion cannot
 * go stale against a hikari that no longer consumes the island.
 *
 * MUT:error-reporting-not-installed — deleting the `app.use(...)` line
 * reddens the first test. MUT:error-reporting-after-mount — moving it
 * below `app.mount(...)` reddens the second.
 */

const MAIN_TS = resolve(__dirname, "..", "..", "src", "main.ts");

/** `main.ts` with comment lines dropped, so prose about the hook cannot
 *  satisfy — or break — the order check. */
function bootstrapSource(): string {
  return readFileSync(MAIN_TS, "utf8")
    .split("\n")
    .filter((line) => {
      const trimmed = line.trim();
      return !trimmed.startsWith("//") && !trimmed.startsWith("*") && !trimmed.startsWith("/*");
    })
    .join("\n");
}

describe("bootstrap installs the error reporting hook", () => {
  // The island is consumed on the first read and hikari keeps install
  // state at module scope, so a leftover document from an earlier test
  // (or from this one, on a re-run) must never carry into the next.
  afterEach(() => {
    document.body.innerHTML = "";
  });


  it("calls app.use(createErrorReporting())", () => {
    expect(bootstrapSource()).toMatch(/app\.use\(\s*createErrorReporting\(/);
  });

  it("installs it BEFORE app.mount (the island is read at install time)", () => {
    const source = bootstrapSource();
    const install = source.search(/app\.use\(\s*createErrorReporting\(/);
    const mount = source.search(/app\.mount\(/);
    expect(install, "the hook must be installed at all").toBeGreaterThanOrEqual(0);
    expect(mount, "the bootstrap must still mount the app").toBeGreaterThanOrEqual(0);
    expect(install).toBeLessThan(mount);
  });

  it("the installed hook really consumes the server island", () => {
    // A live install of the very plugin `main.ts` names: with a handoff
    // island on the document, install must TAKE it (read once, then
    // removed — hikari's contract, so a reinstall cannot re-raise a
    // landing the visitor already saw). If hikari ever drops the island
    // read, this reddens and the source pin above becomes a false sense
    // of safety.
    //
    // What is asserted is the CONSUMPTION, not the overlay: whether a
    // losing second error still paints depends on hikari's module-scope
    // "first error wins" state, which other suites in the same run may
    // already have set. Overlay rendering is hikari's own test's job.
    document.body.innerHTML = `<script type="application/json" id="${SERVER_ERROR_LANDING_ID}">{"v":1,"status":502,"code":"upstream_down","message":"upstream down","raw":{"error":"upstream down"}}</script>`;
    expect(document.getElementById(SERVER_ERROR_LANDING_ID)).not.toBeNull();
    const app = createApp({ render: () => null });
    app.use(createErrorReporting());
    expect(document.getElementById(SERVER_ERROR_LANDING_ID)).toBeNull();
  });
});
