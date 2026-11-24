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

/** Strip comments so prose about the ban is not mistaken for a violation. */
function withoutComments(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/(^|[^:])\/\/.*$/gm, "$1");
}

const FILES = walk(SRC)
  .filter((f) => !/\.test\.tsx?$/.test(f))
  .map((f) => ({ path: relative(SRC, f).split("\\").join("/"), text: withoutComments(readFileSync(f, "utf8")) }));

describe("the page-inline notice ban (tree-wide)", () => {
  it("scans the tree at all — a green over zero files would prove nothing", () => {
    // MUT:inline-notice-scan — narrowing this walk (or breaking the
    // extension filter) empties the corpus and reddens here rather than
    // silently passing everything below.
    expect(FILES.length).toBeGreaterThan(15);
    expect(FILES.some((f) => f.path === "views/FilesView.tsx")).toBe(true);
    expect(FILES.some((f) => f.path === "layouts/AppShell.tsx")).toBe(true);
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
        // A class TOKEN: `class="… dialog-error …"` or a CSS `.dialog-error` rule.
        const asClass = new RegExp(`class(?:Name)?="[^"]*\\b${cls}\\b`);
        const asRule = new RegExp(`\\.${cls}\\b\\s*(?=\\{|,|\\s)`);
        if (asClass.test(f.text)) offenders.push(`${f.path}: class ${cls}`);
        if (f.path.endsWith(".css") && asRule.test(f.text)) offenders.push(`${f.path}: rule .${cls}`);
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
