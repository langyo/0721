import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve, relative } from "node:path";

import { describe, expect, it } from "vitest";

/**
 * The page-inline notice ban, enforced across the whole source tree.
 *
 * The user's rule (2026-11-24) is absolute: 严格禁止出现非必要的在页面内
 * 动态插入提示/部分模块的操作 —— 模块插入也应该套一层 tooltip/二级窗口，
 * 绝对禁止页面内点一下哪个按钮就直接在旁边多一堆 DOM 元素.
 *
 * Per-view tests cannot hold that line: each one only sees the view it
 * mounted, so a NEW view (or a re-added block under a fresh class name)
 * sails past every suite. R3 proved exactly that — a click-grown
 * `<div class="inline-hint" role="note">` in UsersView left all twelve of
 * its pins green.
 *
 * This file is the tree-wide invariant instead. It reads the source and
 * refuses three shapes:
 *
 *  1. `role="alert"` in any view/layout/component — the transient-notice
 *     role. Every one of the eleven that existed before the sweep is now a
 *     toast; a new one is a regression, whatever styles it.
 *  2. The retired notice class names, in TSX *or* CSS — a block that
 *     keeps the old vocabulary is the old block.
 *  3. `role="status"` outside the two allowlisted progress lanes. Those
 *     two are live progress surfaces (`<progress>` per in-flight upload),
 *     not notice-after-an-action; everything else claiming that role is a
 *     page notice wearing a quieter attribute.
 *
 * Test files are exempt (they assert the absence), and so are comments —
 * the rationale for this rule is written in prose all over the tree.
 */

const WEBUI_ROOT = resolve(__dirname, "..", "..");
const SRC = resolve(WEBUI_ROOT, "src");

/** Live progress lanes — the ONLY legitimate `role="status"` surfaces. */
const STATUS_ROLE_ALLOWLIST = new Set([
  "views/PortalView.tsx",
  "views/FilesView.tsx",
]);

/** Class names the sweep retired: their reappearance means the old block is back. */
const RETIRED_NOTICE_CLASSES = [
  "dialog-error",
  "toggle-error",
  "upload-error",
  "save-flash",
  "save-error",
  "copy-failed",
  "login-error",
  "inline-hint",
];

function walk(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      if (entry === "node_modules" || entry === "dist") continue;
      out.push(...walk(full));
    } else if (/\.(tsx?|css)$/.test(entry)) {
      out.push(full);
    }
  }
  return out;
}

/**
 * Remove comments from a JS/TS/CSS source, string- and regex-aware.
 *
 * A regex-based stripper is UNSOUND here: a `//` inside a regex literal or
 * a string deletes the rest of the line INCLUDING live code, so a notice
 * planted after such a literal would evade every check (demonstrated in
 * review with `const sep = /^.*\/\//;` followed by a banned span — both
 * suites stayed green). This is a small single-pass scanner instead:
 *
 *  - line and block comments are removed (block comments keep their
 *    newlines so line-based reasoning survives);
 *  - strings and template literals are copied verbatim (escapes honoured);
 *  - `/` is treated as a regex start only when the previous significant
 *    character says so (operators, openers, statement starts); after an
 *    identifier, a digit, `)` or `]` it is division. Regexes honour `\`
 *    escapes and `[...]` classes, and cannot span lines.
 *
 * The failure mode of an imperfect heuristic is asymmetric, and it fails
 * safe for this rule's purpose: comments are detected FIRST (the `//` and
 * `/*` checks run before the regex branch), so a real comment always
 * opens a comment state; the residual risk of the division heuristic is
 * copying extra text INTO the corpus (e.g. `a++ / 2; // note` reads the
 * `/` as a regex start and leaks the note), which can only cause a false
 * positive — it can never hide a violation.
 */
function stripComments(source: string): string {
  const regexAfter = /[=(,:\[!&|?{};+\-*%<>~^~]/;
  let out = "";
  let prev = "=";
  let i = 0;
  while (i < source.length) {
    const c = source[i];
    const next = source[i + 1] ?? "";
    if (c === "/" && next === "/") {
      while (i < source.length && source[i] !== "\n") i++;
      continue;
    }
    if (c === "/" && next === "*") {
      i += 2;
      let closed = false;
      while (i < source.length) {
        if (source[i] === "*" && source[i + 1] === "/") {
          i += 2;
          closed = true;
          break;
        }
        if (source[i] === "\n") out += "\n";
        i++;
      }
      out += " ";
      if (!closed) break; // unterminated block comment: the rest is comment
      continue;
    }
    if (c === '"' || c === "'" || c === "`") {
      const quote = c;
      out += c;
      i++;
      while (i < source.length && source[i] !== quote) {
        if (source[i] === "\\") {
          out += source[i] + (source[i + 1] ?? "");
          i += 2;
          continue;
        }
        // A newline physically ends a ' or " string; keep scanning (the
        // source would be invalid TS anyway) rather than looping forever.
        out += source[i];
        i++;
      }
      out += quote;
      i++;
      prev = quote;
      continue;
    }
    if (c === "/" && next !== "/" && next !== "*" && regexAfter.test(prev)) {
      // Regex literal: copy verbatim (escapes and character classes).
      out += c;
      i++;
      while (i < source.length && source[i] !== "/") {
        if (source[i] === "\\") {
          out += source[i] + (source[i + 1] ?? "");
          i += 2;
          continue;
        }
        if (source[i] === "\n") break; // not a regex after all; bail out
        out += source[i];
        i++;
      }
      out += "/";
      i++;
      prev = "/";
      continue;
    }
    out += c;
    i++;
    if (!/\s/.test(c)) prev = c;
  }
  return out;
}

const FILES = walk(SRC)
  .filter((f) => !/\.test\.tsx?$/.test(f))
  .map((f) => ({ path: relative(SRC, f).split("\\").join("/"), text: stripComments(readFileSync(f, "utf8")) }));

describe("the page-inline notice ban (tree-wide)", () => {
  it("scans the tree at all — a green over a thinned corpus proves nothing", () => {
    // MUT:inline-notice-scan — narrowing this walk (or breaking the
    // extension filter) thins the corpus below the full view list and
    // reddens here rather than silently passing everything below. Every
    // migrated view/layout/component must be IN the corpus: an exempted
    // file is an unguarded file.
    const REQUIRED = [
      "App.tsx",
      "layouts/AppShell.tsx",
      "components/FileDetailView.tsx",
      "components/SettingsDialog.tsx",
      "views/LoginView.tsx",
      "views/SharedFileView.tsx",
      "views/FilesView.tsx",
      "views/PortalView.tsx",
      "views/TimelineView.tsx",
      "views/SharesView.tsx",
      "views/KeysView.tsx",
      "views/UsersView.tsx",
      "views/ProvidersView.tsx",
      "views/SettingsView.tsx",
    ];
    const inCorpus = new Set(FILES.map((f) => f.path));
    const missing = REQUIRED.filter((r) => !inCorpus.has(r));
    expect(missing).toEqual([]);
    expect(FILES.length).toBeGreaterThan(60);
  });

  it("renders no transient notice inside a page", () => {
    const offenders = FILES.filter(
      (f) => f.path.endsWith(".tsx") && f.text.includes('role="alert"'),
    ).map((f) => f.path);
    // MUT:inline-alert-role — re-adding `<p role="alert">` to any view
    // reddens this, whatever it is styled with.
    expect(offenders).toEqual([]);
  });

  it("keeps the retired notice class names retired", () => {
    const offenders: string[] = [];
    for (const f of FILES) {
      for (const cls of RETIRED_NOTICE_CLASSES) {
        // A class TOKEN: `class="… dialog-error …"` or a CSS selector that
        // names the class. The token boundaries reject both longer names
        // (`.dialog-error-foo`) and deeper specificity drift (`.dialog-error:hover`
        // and `.dialog-error:not(.x)` still match; `.dialog-errorish` does not).
        const asClass = new RegExp(`class(?:Name)?="[^"]*\\b${cls}\\b`);
        const asSelector = new RegExp(`\\.${cls}(?![\\w-])`);
        if (asClass.test(f.text)) offenders.push(`${f.path}: class ${cls}`);
        if (f.path.endsWith(".css") && asSelector.test(f.text)) offenders.push(`${f.path}: rule .${cls}`);
      }
    }
    // MUT:retired-notice-class — restoring any removed notice (or its CSS
    // rule) reddens this.
    expect(offenders).toEqual([]);
  });

  it("confines role=status to the live progress lanes, once each", () => {
    // A per-FILE allowlist is too coarse — it would let a second notice
    // ride inside an allowlisted view's body. The lane is exactly one
    // element pairing the live region with a <progress>, so the checks
    // are: no role="status" outside the allowlist, and inside it exactly
    // one, in a file that really renders progress.
    const offenders: string[] = [];
    for (const f of FILES) {
      if (!f.path.endsWith(".tsx")) continue;
      const hits = f.text.match(/role="status"/g)?.length ?? 0;
      if (hits === 0) continue;
      if (!STATUS_ROLE_ALLOWLIST.has(f.path)) {
        offenders.push(`${f.path}: role="status" outside the progress lanes`);
        continue;
      }
      if (hits > 1) offenders.push(`${f.path}: ${hits} role="status" (the lane is one)`);
      // The live region must actually show progress: the native element
      // (FilesView's overlay) or the family ring (PortalView's lane).
      if (!/<progress|HkProgressRing/.test(f.text)) {
        offenders.push(`${f.path}: role="status" without a progress surface`);
      }
    }
    // MUT:status-role-allowlist — a notice re-added as `role="status"`
    // (the quieter attribute), or a second one smuggled into an
    // allowlisted view, reddens this.
    expect(offenders).toEqual([]);
  });
});
