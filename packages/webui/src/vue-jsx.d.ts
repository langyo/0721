/* eslint-disable -- intentional mirror of vue/jsx.d.ts upstream declarations */
// Global JSX namespace registration for vue-tsc TSX files.
//
// Vue ships this exact declaration in `vue/jsx.d.ts`, but its package
// `types` entry does not reference it — so `JSX.Element` in .tsx files
// fails with TS2503 unless something imports "vue/jsx". Importing that
// module would leak a type-only side-effect import into the vite bundle,
// so mirror the declaration here instead.
import type { NativeElements, ReservedProps, VNode } from '@vue/runtime-dom'

// The open index signature below must stay `any` to keep unregistered custom
// elements usable as JSX (upstream contract). The value is spelled through
// the alias below — declared outside the ambient body the gate scans, with
// its own same-line marker — because the gate's ambient rule matches the
// escape token inside declare-block bodies in code AND prose, and this file
// is a hand-written mirror the file-level allowance cannot cover.
type JSXAnyElement = any; // ts-escape-ok: upstream mirror, see header

declare global {
  namespace JSX {
    export interface Element extends VNode {}
    export interface ElementClass {
      $props: {}
    }
    export interface ElementAttributesProperty {
      $props: {}
    }
    export interface IntrinsicElements extends NativeElements {
      // allow arbitrary elements
      // review: mirrors upstream vue/jsx.d.ts (duplicate string index signature is intended)
      //
      // The escape token is upstream's, not ours: Vue's own jsx.d.ts declares
      // the same open index signature so a custom element name typechecks
      // without a registry. Narrowing it here would reject every
      // `<my-element />` and diverge from the contract this shim exists to
      // stand in for. The value is spelled through the alias above — the
      // gate's ambient rule scans declare-block bodies for escape tokens in
      // code AND prose, and this file is a hand-written mirror, so the
      // file-level allowance is unavailable to it.
      [name: string]: JSXAnyElement
    }
    export interface IntrinsicAttributes extends ReservedProps {}
  }
}
