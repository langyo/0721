import vueSfc from "@vitejs/plugin-vue";
import vueJsx from "@vitejs/plugin-vue-jsx";
import { readdirSync, rmSync } from "fs";
import { resolve } from "path";
import { defineConfig, type Plugin } from "vitest/config";

import { bareSpecifierGatePlugin } from "./src/build/bareSpecifierGate";

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

export default defineConfig({
  define: {
    __APP_VERSION__: JSON.stringify("0.4.0"),
  },
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
