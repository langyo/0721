// The shared-teleport-space style guard.
//
// Dialogs render inside a teleported overlay root, OUTSIDE the page
// component's own DOM subtree. That has two consequences this file
// pins:
//
//   1. A rule that styles something inside a dialog CANNOT be nested
//      under the page's root class (or under a form inside one page) —
//      the element is not a descendant of that ancestor once teleported.
//      Such rules must sit flat. (Root cause of the 2026-11 migration
//      regression: two `.dialog-error` rules were nested under
//      `.edit-form`/`.create-form` and lost the delete-failure face.)
//   2. Flat rules become GLOBAL class names in the single-page bundle,
//      so a name declared in several sheets must carry IDENTICAL
//      declarations — otherwise whichever chunk loads last wins and the
//      same error reads differently from page to page.
//
// Rule 2 is the fragile half: today the three `.dialog-error`
// declarations happen to match byte for byte, which makes the overlap
// harmless by coincidence. Nothing enforced it before this guard, so a
// future one-sheet edit would drift silently. The assertions below turn
// that coincidence into a pinned invariant.

import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const here = dirname(fileURLToPath(import.meta.url));

/** Every stylesheet the console ships. */
function allSheets(root: string, acc: string[] = []): string[] {
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const full = join(root, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === "node_modules" || entry.name === "__tests__") continue;
      allSheets(full, acc);
    } else if (entry.name.endsWith(".scss")) {
      acc.push(full);
    }
  }
  return acc;
}

/** The declaration body of `.<className>` at ANY nesting depth, with
 *  whitespace collapsed so formatting alone never reads as a mismatch.
 *  `null` when the sheet does not declare it. */
function declarationOf(sheet: string, className: string): string | null {
  const src = readFileSync(sheet, "utf8");
  const re = new RegExp(`\\.${className}\\s*\\{([^}]*)\\}`, "g");
  const hits = [...src.matchAll(re)];
  if (hits.length === 0) return null;
  // Collapse whitespace and drop the trailing semicolon before `}`.
  const normalise = (body: string) =>
    body
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line !== "")
      .join(" ")
      .replace(/;\s*$/, "");
  const bodies = hits.map((m) => normalise(m[1] ?? ""));
  // A sheet declaring the same class twice must agree with itself too.
  const first = bodies[0]!;
  for (const body of bodies) expect(body, `${sheet}: .${className} declared inconsistently`).toBe(first);
  return first;
}

/** Classes that live in the teleported overlay space and are therefore
 *  declared flat (see rule 1 above). Each must resolve to ONE declaration
 *  set across the whole bundle (rule 2). */
const SHARED_TELEPORT_CLASSES = ["dialog-error"] as const;

describe("teleported dialog styles", () => {
  const sheets = allSheets(resolve(here, ".."));

  it("scans a non-empty set of sheets (zero-scan is never green)", () => {
    // The console ships a dozen-odd sheets; a number this low can only
    // mean the walk broke, which would make every assertion below vacuous.
    expect(sheets.length).toBeGreaterThan(5);
  });

  it("keeps every shared overlay class declared identically wherever it appears", () => {
    for (const className of SHARED_TELEPORT_CLASSES) {
      const declarations = sheets
        .map((sheet) => ({ sheet, body: declarationOf(sheet, className) }))
        .filter((entry): entry is { sheet: string; body: string } => entry.body !== null);
      // At least one sheet must declare it, otherwise this guard is
      // silently checking nothing.
      expect(
        declarations.length,
        `.${className} is declared nowhere — the guard has nothing to pin`,
      ).toBeGreaterThan(0);
      const bodies = new Set(declarations.map((entry) => entry.body));
      expect(
        bodies.size,
        `.${className} is declared with DIFFERENT bodies in ${declarations
          .map((e) => e.sheet.replace(/^.*\/src\//, ""))
          .join(", ")} — the later-loading chunk wins and the same element ` +
          `renders differently per page. Collapse them into one shared rule.`,
      ).toBe(1);
    }
  });

  it("never nests a shared overlay class under a page-scope ancestor", () => {
    // A nested `.dialog-error {` (any leading whitespace) means some
    // ancestor must be present in the DOM for the rule to apply — false
    // for anything teleported into the overlay root.
    for (const className of SHARED_TELEPORT_CLASSES) {
      for (const sheet of sheets) {
        const src = readFileSync(sheet, "utf8");
        const nested = new RegExp(`^[ \\t]+\\.${className}\\s*\\{`, "m").test(src);
        expect(
          nested,
          `${sheet}: .${className} is nested under an ancestor selector, but it ` +
            `styles a TELEPORTED element — the ancestor is absent in the overlay root`,
        ).toBe(false);
      }
    }
  });
});
