/**
 * The footer's version label (family rule, same shape as celestia-erp and
 * plana): `<semver> <branch>:<hash7>` — two builds of the same release must
 * be distinguishable, so the build's git identity rides along.
 *
 * `__APP_BUILD_IDENTITY__` is `null` for a build with no git metadata
 * (tarball / Docker), and the label then degrades to the bare semver rather
 * than inventing a branch name.
 */
export function appVersionLabel(): string {
  const version: string = __APP_VERSION__;
  return __APP_BUILD_IDENTITY__ ? `${version} ${__APP_BUILD_IDENTITY__}` : version;
}
