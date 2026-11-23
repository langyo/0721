import { describe, expect, it } from "vitest";

import { formatSize } from "./formatSize";

/**
 * The shared formatter's contract — every surface renders the same number
 * from the same function (the pane and a tile used to disagree: decimal
 * versus binary units).
 *
 * MUT:format-size-units — switching a unit token or the 1024 divisor
 * reddens the corresponding row.
 */

describe("formatSize", () => {
  it("renders sub-kibibyte values as plain bytes", () => {
    expect(formatSize(0)).toBe("0 B");
    expect(formatSize(1)).toBe("1 B");
    expect(formatSize(1023)).toBe("1023 B");
  });

  it("uses BINARY units — the disagreement the three copies had", () => {
    // 2048 B is 2.0 KiB; the pane used to print "2.0 KB" while the tile
    // printed "2.0 KiB" for the same file on the same page.
    expect(formatSize(2048)).toBe("2.0 KiB");
    expect(formatSize(1024 ** 2)).toBe("1.0 MiB");
    expect(formatSize(1024 ** 3)).toBe("1.0 GiB");
    expect(formatSize(1024 ** 4)).toBe("1.0 TiB");
  });

  it("rounds to a whole number once the mantissa reaches ten", () => {
    expect(formatSize(1024 * 10)).toBe("10 KiB");
    expect(formatSize(1024 * 10.6)).toBe("11 KiB");
  });

  it("degrades to an em dash for a broken value", () => {
    expect(formatSize(Number.NaN)).toBe("—");
    expect(formatSize(Number.POSITIVE_INFINITY)).toBe("—");
    expect(formatSize(-1)).toBe("—");
  });
});
