/// <reference types="vite/client" />

// hikari's build-identity store reads the app version the bundler
// injects (vite `define`); declared here so the source-distributed
// package typechecks inside this tree.
declare const __APP_VERSION__: string;

// hikari's stylesheet entry (a .scss side-effect export).
declare module "@celestia-island/hikari/styles";
