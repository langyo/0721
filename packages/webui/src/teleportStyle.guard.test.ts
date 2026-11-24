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
// Rule 2 is the fragile half: the `.dialog-error` declarations match
// byte for byte today, which makes the overlap harmless by coincidence.
// The assertions below turn that coincidence into a pinned invariant.
//
// The scanner reads BRACE DEPTH, not indentation: SCSS is free-form, so
// ` .x {` at any indentation inside another block is nested, and a
// top-level rule may be written across many lines. Both a real nesting
// and a real top-level rule are pinned by the self-test below, so the
// detector cannot silently no-op.

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

/** Comments removed (both `/* … *\/` and `//`), so a documented selector
 *  never counts as a real one and depth counting cannot be fooled by a
 *  stray brace inside prose. */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, " ").replace(/\/\/[^\n]*/g, " ");
}

/** One occurrence of `.<className> { … }`. */
export interface ClassRule {
  /** Declarations written DIRECTLY in the block, nested rule blocks
   *  removed and whitespace collapsed — the comparable identity of the
   *  rule. */
  topLevelBody: string;
  /** The brace depth the selector itself sits at (0 = top level). */
  depth: number;
}

/** Every occurrence of `.<className> { … }` in a sheet, with balanced
 *  braces — so a block that itself contains nested rules is captured
 *  whole, and its TOP-LEVEL declarations are what `topLevelBody`
 *  carries. */
export function classRules(src: string, className: string): ClassRule[] {
  const code = stripComments(src);
  const out: ClassRule[] = [];
  let depth = 0;
  let i = 0;
  while (i < code.length) {
    const ch = code[i]!;
    if (ch === "{") {
      depth += 1;
      i += 1;
      continue;
    }
    if (ch === "}") {
      depth = Math.max(0, depth - 1);
      i += 1;
      continue;
    }
    if (ch !== ".") {
      i += 1;
      continue;
    }
    // A class selector only counts where a selector may start: the
    // character before must be a delimiter (or the string start).
    const prev = i === 0 ? "" : code[i - 1]!;
    if (prev !== "" && !/[\s,{}>+~(]/.test(prev)) {
      i += 1;
      continue;
    }
    const m = /^\.([A-Za-z][\w-]*)\s*\{/.exec(code.slice(i));
    if (!m || m[1] !== className) {
      i += 1;
      continue;
    }
    // Capture the balanced block (the `{` sits at the end of the match).
    const open = i + m[0].length - 1;
    let blockDepth = 0;
    let close = open;
    while (close < code.length) {
      if (code[close] === "{") blockDepth += 1;
      else if (code[close] === "}") {
        blockDepth -= 1;
        if (blockDepth === 0) break;
      }
      close += 1;
    }
    const inner = code.slice(open + 1, close);
    // Collect what is written at the block's own level: nested rule
    // blocks are skipped wholesale, they are other selectors' business.
    // A nested block's SELECTOR text (`.field-label` in
    // `.dialog-body { .field-label { … } }`) still lands in the buffer,
    // so the final filter keeps only property-shaped fragments — a
    // selector never starts with a property name.
    let buf = "";
    let local = 0;
    for (const c of inner) {
      if (c === "{") local += 1;
      else if (c === "}") local -= 1;
      else if (local === 0) buf += c;
    }
    const topLevelBody = buf
      .split(";")
      .map((line) => line.trim())
      .filter((line) => /^[-\w]+\s*:/.test(line))
      .join("; ");
    out.push({ topLevelBody, depth });
    // The block is balanced, so the outer depth is unchanged past it.
    i = close + 1;
  }
  return out;
}

/** Classes that live in the teleported overlay space and are therefore
 *  declared flat (rule 1) and identical wherever repeated (rule 2).
 *  The sheets' own headers document each of these as a teleport-space
 *  anchor — extend this list together with those headers. */
const SHARED_TELEPORT_CLASSES = ["dialog-error", "dialog-body", "warn-text"] as const;

describe("teleported dialog styles", () => {
  const sheets = allSheets(resolve(here, ".."));

  it("scans a non-empty set of sheets (zero-scan is never green)", () => {
    // The console ships a dozen-odd sheets; a number this low can only
    // mean the walk broke, which would make every assertion below vacuous.
    expect(sheets.length).toBeGreaterThan(5);
  });

  it("the scanner tells a nested rule from a top-level one (self-test)", () => {
    // Without this the depth walk could silently no-op and every
    // assertion below would pass on anything. Both directions, on
    // synthetic fixtures through the SAME function the real checks use.
    const flat = classRules(".keep, .x { color: red; }", "x");
    expect(flat).toHaveLength(1);
    expect(flat[0]!.depth).toBe(0);
    expect(flat[0]!.topLevelBody).toBe("color: red");

    const nested = classRules(".page {\n  .x { color: red; .y { margin: 0; } }\n}", "x");
    expect(nested).toHaveLength(1);
    expect(nested[0]!.depth).toBeGreaterThan(0);
    // The nested rule block inside `.x` is skipped: the comparable body
    // is only what `.x` declares at its own level.
    expect(nested[0]!.topLevelBody).toBe("color: red");

    const deep = classRules(".a { .b { .x { color: red; } } }", "x");
    expect(deep[0]!.depth).toBeGreaterThan(1);

    // A same-prefix class (`@media`, another name) is not the target.
    expect(classRules(".x-other { color: red; }", "x")).toHaveLength(0);
    // A `//`-commented selector is prose, not a rule.
    expect(classRules("// .x { color: red; }", "x")).toHaveLength(0);
  });

  it("keeps every shared overlay class declared identically wherever it appears", () => {
    for (const className of SHARED_TELEPORT_CLASSES) {
      const declarations = sheets
        .map((sheet) => ({ sheet, body: declarationTopLevel(sheet, className) }))
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
    // A nested `.<class> {` means some ancestor must be present in the
    // DOM for the rule to apply — false for anything teleported into the
    // overlay root.
    for (const className of SHARED_TELEPORT_CLASSES) {
      for (const sheet of sheets) {
        for (const rule of classRules(stripComments(readFileSync(sheet, "utf8")), className)) {
          expect(
            rule.depth,
            `${sheet}: .${className} is nested under an ancestor selector (depth ` +
              `${rule.depth}), but it styles a TELEPORTED element — the ancestor is ` +
              `absent in the overlay root`,
          ).toBe(0);
        }
      }
    }
  });
});

/** The comparable top-level body of `.<className>` in one sheet, `null`
 *  when the sheet does not declare it. A sheet declaring the class twice
 *  must agree with itself (checked here too). */
function declarationTopLevel(sheet: string, className: string): string | null {
  const rules = classRules(stripComments(readFileSync(sheet, "utf8")), className);
  if (rules.length === 0) return null;
  const first = rules[0]!.topLevelBody;
  for (const rule of rules) {
    expect(
      rule.topLevelBody,
      `${sheet}: .${className} is declared inconsistently within the sheet`,
    ).toBe(first);
  }
  return first;
}
