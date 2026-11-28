// Contract for the deployment default wiring: the brand `res` scheme is
// declared through hikari's page seam (`window.__celestiaDefaultTheme`)
// so a selection-less visitor lands on the brand floor (pure white /
// near-black sea) instead of the stock factory scheme. Resolves through
// the SAME seam hikari's own `resolveDefaultTheme` reads — this test
// exercises the real function, not a reimplementation.
import { beforeEach, describe, expect, it } from "vitest";

import { declareBrandDefault, registerBrandPresets, RES_THEME_ID, brandPresets } from "@/theme";
import { themePresets } from "@celestia-island/hikari";

describe("deployment default wiring", () => {
  beforeEach(() => {
    delete (window as unknown as { __celestiaDefaultTheme?: string }).__celestiaDefaultTheme;
    // Production wiring order: the brand row is installed into hikari's
    // exported table before any consumer reads it (main.ts does the same).
    registerBrandPresets();
  });

  it("declares the brand id on the page seam hikari reads", () => {
    // A floor-ruling change must always trip this declaration before any
    // default resolution — the seam is the single wiring point.
    // MUT:brand-default-seam
    declareBrandDefault();
    expect(
      (window as unknown as { __celestiaDefaultTheme?: string }).__celestiaDefaultTheme,
    ).toBe(RES_THEME_ID);
  });

  it("declares an id that resolves in the live preset table", () => {
    declareBrandDefault();
    const declared = (window as unknown as { __celestiaDefaultTheme?: string })
      .__celestiaDefaultTheme;
    // The seam is only honored when the id resolves — a dangling default
    // silently falls back to the factory scheme again. MUT:brand-default-seam
    expect(themePresets[declared as string]).toBeDefined();
    expect(brandPresets[declared as string]).toBeDefined();
  });

  it("keeps the factory scheme selectable beside the brand", () => {
    declareBrandDefault();
    // The design keeps hikari's stock `default` in the table (personal
    // installs pick it in the picker); only the DEFAULT moves to the brand.
    // MUT:brand-factory-row-kept
    expect(themePresets["default"]).toBeDefined();
    expect(Object.keys(themePresets)).toContain(RES_THEME_ID);
  });
});