/**
 * The build-identity ladder (family rule, celestia-erp's twin).
 *
 * The panel footer prints `<semver> <branch>:<hash7>` so two builds of the
 * same release are distinguishable. Resolution order:
 *   1. `PANEL_BUILD_IDENTITY` — the container hand-off. A Docker build has no
 *      `.git` (the COPY list is package-scoped on purpose), so the CI job that
 *      OWNS the checkout resolves the identity and passes it in.
 *   2. git, read from the tree: the branch, or — when HEAD is detached — the
 *      default branch's name if HEAD is exactly its tip, else `name-rev` over
 *      local heads / origin remotes with `~N` depth stripped.
 *   3. `null` — no git metadata and no hand-off. The caller then prints the
 *      bare version rather than inventing a branch name.
 *
 * Lives outside `vite.config.ts` so the parse rules are unit-testable: the
 * shape check is defence-in-depth for the container path (a malformed
 * build-arg must fall THROUGH the ladder, never bake a bogus identity).
 */
import { execFileSync } from "child_process";
import { resolve } from "path";

/** `<branch>:<hash7..40>`, single colon, lowercase hex hash. Branch may
 *  itself contain colons (the split is on the LAST one). */
export function parseIdentityToken(raw: string): string | undefined {
  const token = raw.trim();
  const splitAt = token.lastIndexOf(":");
  if (splitAt <= 0) return undefined;
  const branch = token.slice(0, splitAt).replace(/:+$/, "");
  const hash = token.slice(splitAt + 1);
  if (branch === "") return undefined;
  if (!/^[0-9a-f]{7,40}$/.test(hash)) return undefined;
  if (hash !== hash.toLowerCase()) return undefined;
  return `${branch}:${hash.toLowerCase()}`;
}

/** The container hand-off, checked before the git ladder. */
export function identityFromEnv(
  env: Record<string, string | undefined> = process.env,
): string | undefined {
  const raw = env.PANEL_BUILD_IDENTITY?.trim();
  return raw ? parseIdentityToken(raw) : undefined;
}

/** The git ladder. `undefined` when the tree has no git metadata. */
export function gitIdentity(root: string): string | undefined {
  try {
    const run = (args: string[]) =>
      execFileSync("git", args, { cwd: root }).toString().trim();
    const short = run(["rev-parse", "--short=7", "HEAD"]);
    if (!/^[0-9a-f]{7,40}$/.test(short)) return undefined;
    const commit = short.toLowerCase();
    let branch = run(["rev-parse", "--abbrev-ref", "HEAD"]);
    if (!branch || branch === "HEAD") {
      // Detached: the default branch's name wins when HEAD is exactly its
      // tip (a parallel same-tip branch must not steal the identity).
      try {
        const sym = run(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]);
        const name = sym.split("/").slice(1).join("/");
        if (name && name !== "HEAD") {
          const head = run(["rev-parse", "HEAD"]);
          for (const ref of [`refs/heads/${name}`, `refs/remotes/origin/${name}`]) {
            if (run(["rev-parse", "--verify", ref]) === head) return `${name}:${commit}`;
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

/** The full ladder: env hand-off, then git, then `undefined`. */
export function resolveBuildIdentity(repoRoot: string): string | undefined {
  return identityFromEnv() ?? gitIdentity(repoRoot);
}

export { resolve as resolvePath };
