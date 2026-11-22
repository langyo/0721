import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";
import { afterEach, describe, expect, it } from "vitest";

import { bareSpecifierGatePlugin, scanDirForBareSpecifiers } from "./bareSpecifierGate";

/**
 * The bare-specifier gate's contract, pinned against a throwaway dist
 * tree — including the NEGATIVE CONTROL for the recursion fix (R2 S2-3):
 * an offender in a NESTED subdirectory (deeper than the root+assets
 * layout vite happens to emit today) must trip the gate exactly like a
 * top-level one. The old one-level scan waved exactly this through.
 */

const dirs: string[] = [];

function makeDist(): string {
  const dir = mkdtempSync(join(tmpdir(), "bare-gate-"));
  dirs.push(dir);
  return dir;
}

afterEach(() => {
  for (const dir of dirs.splice(0)) {
    rmSync(dir, { recursive: true, force: true });
  }
});

describe("scanDirForBareSpecifiers", () => {
  it("accepts a clean tree (relative/absolute imports and prose pass)", () => {
    const dir = makeDist();
    writeFileSync(
      join(dir, "main.js"),
      'import"./chunk.js";import x from"../x.js";const s="convert colors from srgb-linear to x";',
    );
    expect(scanDirForBareSpecifiers(dir)).toEqual([]);
  });

  it("flags a bare specifier at the dist root", () => {
    const dir = makeDist();
    writeFileSync(join(dir, "main.js"), 'import{a}from"lucide-vue-next";');
    expect(scanDirForBareSpecifiers(dir)).toEqual(["main.js: lucide-vue-next"]);
  });

  it("flags bare specifiers at ANY depth — the recursion contract", () => {
    const dir = makeDist();
    // The layout vite emits today…
    mkdirSync(join(dir, "assets"));
    // …and a deeper nest (worker bundle / vendored tree / future chunk
    // layout). The old root+assets one-level scan missed this file.
    mkdirSync(join(dir, "assets", "workers", "deep"), { recursive: true });
    writeFileSync(join(dir, "assets", "index-Ab12.js"), 'const x=1;x.from"vue"');
    writeFileSync(
      join(dir, "assets", "workers", "deep", "worker-Cd34.js"),
      '}from"@celestia-island/hikari";import("lodash")',
    );
    const offenders = scanDirForBareSpecifiers(dir);
    expect(offenders).toContain("worker-Cd34.js: @celestia-island/hikari");
    expect(offenders).toContain("worker-Cd34.js: lodash");
    expect(offenders).toContain("index-Ab12.js: vue");
  });
});

describe("bareSpecifierGatePlugin", () => {
  it("closeBundle throws on a nested offender (the hard-fail path)", () => {
    const dir = makeDist();
    mkdirSync(join(dir, "chunks", "more"), { recursive: true });
    writeFileSync(
      join(dir, "chunks", "more", "nested.js"),
      'import{x}from"pinia";',
    );
    const plugin = bareSpecifierGatePlugin(dir) as { closeBundle: () => void };
    expect(() => plugin.closeBundle()).toThrow(/bare module specifiers[\s\S]*pinia/);
  });

  it("closeBundle passes on a clean tree", () => {
    const dir = makeDist();
    writeFileSync(join(dir, "main.js"), 'import"./a.js";');
    const plugin = bareSpecifierGatePlugin(dir) as { closeBundle: () => void };
    expect(() => plugin.closeBundle()).not.toThrow();
  });
});
