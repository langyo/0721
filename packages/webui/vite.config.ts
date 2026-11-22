import vueSfc from "@vitejs/plugin-vue";
import vueJsx from "@vitejs/plugin-vue-jsx";
import { readdirSync, rmSync, readFileSync } from "fs";
import { resolve } from "path";
import { defineConfig, type Plugin } from "vite";

const pkgDir = import.meta.dirname;

// outDir lives outside the package (../../dist/webui) and is Docker
// bind-mounted, so we cannot delete+recreate the directory itself (the
// container would keep seeing the old, deleted inode). Instead this
// plugin wipes the CONTENTS of outDir at buildStart — same effect as
// `emptyOutDir: true` but the directory inode is preserved, and stale
// hashed assets from previous builds no longer accumulate (they would
// pile up and, worse, get embedded into the release binary through the
// server's include_dir!).
function cleanOutDirContents(outDir: string): Plugin {
  return {
    name: "clean-outdir-contents",
    apply: "build",
    buildStart() {
      let entries: string[];
      try {
        entries = readdirSync(outDir);
      } catch {
        return;
      }
      for (const entry of entries) {
        rmSync(resolve(outDir, entry), { recursive: true, force: true });
      }
    },
  };
}

// Post-build gate: the deployed binary embeds whatever dist/ contains, and
// rolldown-vite SILENTLY EXTERNALIZES imports it fails to resolve — a
// stale node_modules could ship chunks containing raw `from"lucide-vue-next"`
// and every browser would throw "Failed to resolve module specifier" at
// boot. Rollup would fail the build; rolldown just warns. This plugin scans
// the written output for any surviving bare module specifier and turns it
// into a hard build error, so the failure mode can never ship silently.
// (Ported from the shittim-chest webui — the 2026-08-30 incident guard.)
function bareSpecifierGatePlugin(outDir: string): Plugin {
  // Bare specifier charset: starts with a letter or @, no spaces/colons —
  // excludes relative ("./x", "../x") and absolute ("/x") forms as well as
  // data:/http(s):/node: URLs and CSS-content strings like `from": "…`.
  // The `from` alternative allows NO whitespace: the gate scans the
  // MINIFIED bundle where a real re-export/statement import is always
  // `}from"x"` / `from"x"`, while prose inside string constants keeps the
  // human spacing and must not trip the gate (a false positive here makes
  // every build fail).
  const BARE_SPECIFIER = /(?:\bfrom["']|\bimport\s*\(\s*["'])([a-z@][a-z0-9@/._-]*)["']/g;
  return {
    name: "bare-specifier-gate",
    apply: "build",
    closeBundle() {
      const offenders: string[] = [];
      const scan = (dir: string) => {
        let files: string[];
        try {
          files = readdirSync(dir);
        } catch {
          return;
        }
        for (const f of files) {
          const p = resolve(dir, f);
          if (!f.endsWith(".js")) continue;
          const src = readFileSync(p, "utf-8");
          for (const m of src.matchAll(BARE_SPECIFIER)) {
            offenders.push(`${f}: ${m[1]}`);
          }
        }
      };
      scan(outDir);
      scan(resolve(outDir, "assets"));
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

export default defineConfig({
  plugins: [
    cleanOutDirContents(resolve(pkgDir, "../../dist/webui")),
    bareSpecifierGatePlugin(resolve(pkgDir, "../../dist/webui")),
    vueSfc(),
    // hikari ships its component library as raw .tsx source through the
    // package's `.` export, so the JSX transform must run on those modules
    // too — without this plugin every hikari component import fails to
    // parse at build time.
    vueJsx(),
  ],
  resolve: {
    alias: {
      "@": resolve(pkgDir, "src"),
    },
  },
  css: {
    preprocessorOptions: {
      // hikari's stylesheets are consumed via bare specifiers
      // (`@use "@celestia-island/hikari/styles/…" or plain-name partials),
      // resolved through the package's `./styles/*` export. The load paths
      // below point at the SAME directory inside the installed package so
      // the vendored theme partials' plain-name `@use 'tokens'` imports
      // keep resolving for the sass compiler.
      scss: {
        loadPaths: [
          resolve(pkgDir, "node_modules/@celestia-island/hikari/src/styles"),
          resolve(pkgDir, "node_modules/@celestia-island/hikari/src/styles/theme"),
        ],
      },
    },
  },
  publicDir: resolve(pkgDir, "public"),
  optimizeDeps: {
    // hikari rides the dev-server through a source alias, so its .tsx
    // components land in the dep graph as raw source. The dep
    // pre-bundler applies neither the project's vue-jsx plugin nor any
    // esbuild jsx loader to them, so the optimized artifact keeps raw JSX
    // and every hikari .tsx import 500s in import-analysis. Excluding the
    // package serves those modules through the normal transform pipeline
    // instead, where @vitejs/plugin-vue-jsx applies. Build is unaffected
    // (the bare-specifier gate above hard-fails undigested output).
    exclude: ["@celestia-island/hikari"],
  },
  base: "/",
  build: {
    outDir: resolve(pkgDir, "../../dist/webui"),
    // Contents are wiped by the cleanOutDirContents plugin (preserves the
    // directory inode for the Docker bind-mount). Keep this false so Vite
    // itself never deletes the outDir directory.
    emptyOutDir: false,
    target: "es2020",
    rollupOptions: {
      input: {
        main: resolve(pkgDir, "index.html"),
      },
    },
  },
  test: {
    environment: "happy-dom",
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
    globals: true,
  },
});
