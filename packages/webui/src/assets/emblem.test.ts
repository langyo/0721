import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

/**
 * The brand assets are shipped bytes, so their guard is a golden pin plus
 * a structural parse — a view test cannot see them.
 *
 * Why it matters: the emblem was re-vendored from upstream once and came
 * back with its interior whites keyed out along with the background (the
 * instax patch and the hat's high notes became transparent holes). Over
 * the dark login surface the badge read as moth-eaten. The corrected
 * render removes only the background CONNECTED to the border, so the
 * enclosed whites survive; this file pins that outcome.
 */

const WEBUI_ROOT = resolve(__dirname, "..", "..");
const PUBLIC = resolve(WEBUI_ROOT, "public");

function sha256(path: string): string {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

describe("brand assets", () => {
  it("ships the corrected emblem — the one whose interior whites survived", () => {
    // MUT:emblem-vendor — re-vendoring upstream's white-keyed copy changes
    // these bytes and reddens this pin (measured on the two candidates:
    // the corrected render keeps 252 opaque near-white pixels, the keyed
    // one keeps 0).
    expect(sha256(resolve(PUBLIC, "logo.webp"))).toBe(
      "5ad83f187536c75e756d590832a41a0a2ca76c0ff7ea6ea92e6ecb0f67fc661c",
    );
  });

  it("packs the tab icon as six PNG entries that tile the file exactly", () => {
    const ico = readFileSync(resolve(PUBLIC, "favicon.ico"));
    // ICONDIR: reserved=0, type=1 (icon), count
    expect(ico.readUInt16LE(0)).toBe(0);
    expect(ico.readUInt16LE(2)).toBe(1);
    const count = ico.readUInt16LE(4);
    expect(count).toBe(6);

    let expectedOffset = 6 + 16 * count; // the payload area starts here
    const dims: number[] = [];
    for (let i = 0; i < count; i += 1) {
      const e = 6 + 16 * i;
      const width = ico[e]!;
      const height = ico[e + 1]!;
      expect(width).toBe(height);
      dims.push(width === 0 ? 256 : width); // 256 is encoded as 0
      expect(ico.readUInt16LE(e + 4)).toBe(1); // planes
      expect(ico.readUInt16LE(e + 6)).toBe(32); // bpp
      const size = ico.readUInt32LE(e + 8);
      const offset = ico.readUInt32LE(e + 12);
      // Contiguous, no gaps: each payload begins where the last ended.
      expect(offset).toBe(expectedOffset);
      expect(size).toBeGreaterThan(0);
      // MUT:favicon-container — dropping the PNG signature or shifting an
      // offset reddens here rather than in a browser.
      expect(ico.subarray(offset, offset + 8)).toEqual(
        Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
      );
      expectedOffset += size;
    }
    expect(expectedOffset).toBe(ico.length);
    expect(dims).toEqual([16, 32, 48, 64, 128, 256]);
  });
});