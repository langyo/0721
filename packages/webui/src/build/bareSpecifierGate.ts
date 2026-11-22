import { readdirSync, readFileSync, type Dirent } from "fs";
import { resolve } from "path";
import type { Plugin } from "vite";

/**
 * Post-build gate: the deployed binary embeds whatever dist/ contains, and
 * rolldown-vite SILENTLY EXTERNALIZES imports it fails to resolve — a
 * stale node_modules could ship chunks containing raw `from"lucide-vue-next"`
 * and every browser would throw "Failed to resolve module specifier" at
 * boot. Rollup would fail the build; rolldown just warns. This plugin scans
 * the written output for any surviving bare module specifier and turns it
 * into a hard build error, so the failure mode can never ship silently.
 * (Ported from the shittim-chest webui — the 2026-08-30 incident guard.)
 *
 * The scan is RECURSIVE over the whole outDir: every .js file under it,
 * at any depth. Vite's current output is flat (root + assets/), but the
 * gate must not ride on that layout coincidence — a manualChunks change,
 * a worker bundle, or a copied vendored tree can nest deeper, and a
 * one-level scan would wave the offending chunk through.
 */

// Bare specifier charset: starts with a letter or @, no spaces/colons —
// excludes relative ("./x", "../x") and absolute ("/x") forms as well as
// data:/http(s):/node: URLs and CSS-content strings like `from": "…`.
// The `from` alternative allows NO whitespace: the gate scans the
// MINIFIED bundle where a real re-export/statement import is always
// `}from"x"` / `from"x"`, while prose inside string constants keeps the
// human spacing and must not trip the gate (a false positive here makes
// every build fail).
const BARE_SPECIFIER = /(?:\bfrom["']|\bimport\s*\(\s*["'])([a-z@][a-z0-9@/._-]*)["']/g;

/** Collect every offender (`file: specifier`) under `dir`, recursively. */
export function scanDirForBareSpecifiers(dir: string): string[] {
  const offenders: string[] = [];
  const scan = (current: string) => {
    let entries: Dirent[];
    try {
      entries = readdirSync(current, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      const p = resolve(current, entry.name);
      if (entry.isDirectory()) {
        scan(p);
        continue;
      }
      if (!entry.name.endsWith(".js")) continue;
      const src = readFileSync(p, "utf-8");
      for (const m of src.matchAll(BARE_SPECIFIER)) {
        offenders.push(`${entry.name}: ${m[1]}`);
      }
    }
  };
  scan(dir);
  return offenders;
}

/** The vite plugin form: hard-fails `closeBundle` on any offender. */
export function bareSpecifierGatePlugin(outDir: string): Plugin {
  return {
    name: "bare-specifier-gate",
    apply: "build",
    closeBundle() {
      const offenders = scanDirForBareSpecifiers(outDir);
      if (offenders.length > 0) {
        throw new Error(
          `[bare-specifier-gate] output contains unresolved bare module ` +
            `specifiers (rolldown externalized them silently — the importing ` +
            `package's node_modules is stale):\n  ` +
            offenders.slice(0, 20).join("\n  ") +
            (offenders.length > 20 ? `\n  …and ${offenders.length - 20} more` : ""),
        );
      }
    },
  };
}
