/// <reference types="vite/client" />

// hikari's build-identity store reads the app version the bundler
// injects (vite `define`); declared here so the source-distributed
// package typechecks inside this tree.
declare const __APP_VERSION__: string;
/** `<branch>:<hash7>` of the tree this bundle was built from, or `null`
 *  when the build had no git metadata (tarball / Docker). The footer appends
 *  it so two builds of the same version are distinguishable (family rule). */
declare const __APP_BUILD_IDENTITY__: string | null;

// hikari's stylesheet entry (a .scss side-effect export).
declare module "@celestia-island/hikari/styles";
