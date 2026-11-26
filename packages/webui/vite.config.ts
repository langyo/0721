import vueSfc from "@vitejs/plugin-vue";
import vueJsx from "@vitejs/plugin-vue-jsx";
import { execFileSync } from "child_process";
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

/**
 * Family build identity (`<branch>:<hash7>`, e.g. `master:a440d4d`).
 *
 * The family rule (celestia-erp's `vite.config.ts` is the twin this mirrors,
 * itself the plana twin): the branch and the exact commit ARE the identity,
 * so a footer that prints only a semver cannot tell two builds of the same
 * version apart. Ladder: direct branch first, then `name-rev` over local
 * heads / origin remotes (stripping any `~N` depth and the remote prefix),
 * with the default-tip guard so a parallel same-tip branch cannot steal the
 * name; `undefined` when the tree has no git metadata (a tarball or Docker
 * build), which keeps such builds deterministic instead of inventing a name.
 */
function buildIdentity(root: string): string | undefined {
  try {
    const run = (args: string[]) =>
      execFileSync("git", args, { cwd: root }).toString().trim();
    const short = run(["rev-parse", "--short=7", "HEAD"]);
    if (!/^[0-9a-f]{7,40}$/.test(short)) return undefined;
    const commit = short.toLowerCase();
    let branch = run(["rev-parse", "--abbrev-ref", "HEAD"]);
    if (!branch || branch === "HEAD") {
      // Detached: prefer the default branch's name when HEAD is exactly its
      // tip, then name-rev over local/remote refs.
      try {
        const sym = run(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]);
        const name = sym.split("/").slice(1).join("/");
        if (name && name !== "HEAD") {
          const head = run(["rev-parse", "HEAD"]);
          for (const ref of [`refs/heads/${name}`, `refs/remotes/origin/${name}`]) {
            if (run(["rev-parse", "--verify", ref]) === head) {
              return `${name}:${commit}`;
            }
          }
        }
      } catch {
        /* no symbolic remote HEAD recorded */
      }
      try {
        branch = run(["name-rev", "--name-only", "--exclude=refs/tags/*", "HEAD"]);
      } catch {
        branch = "";
      }
      branch = branch
        .replace(/~\d+$/, "")
        .replace(/\^\d+$/, "")
        .replace(/^remotes\/origin\//, "");
    } else {
      branch = branch.replace(/^remotes\/origin\//, "");
    }
    if (!branch || branch === "undefined") branch = "detached";
    return `${branch}:${commit}`;
  } catch {
    return undefined;
  }
}

export default defineConfig({
  define: {
    __APP_VERSION__: JSON.stringify("0.4.0"),
    // Absent git metadata leaves this as `null` on purpose: the view then
    // prints the bare version rather than a fabricated identity.
    __APP_BUILD_IDENTITY__: JSON.stringify(
      buildIdentity(resolve(pkgDir, "../..")) ?? null,
    ),
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
