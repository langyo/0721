/**
 * The one size formatter (binary units), shared by the file roster, the
 * portal tile and the detail pane.
 *
 * Why it is a module and not three local copies: they drifted — the pane
 * printed decimal `KB/MB` while the tiles printed binary `KiB/MiB`, so a
 * 2048-byte file read "2.0 KB" in the pane and "2.0 KiB" in its own tile
 * on the same page. The wave that put all three surfaces on one detail
 * component is exactly the place to make the number agree.
 *
 * A non-finite or negative input degrades to an em dash rather than a
 * nonsense number (the DTO says `number`; a broken producer must not
 * render "NaN B").
 */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KiB", "MiB", "GiB", "TiB", "PiB"];
  let value = bytes;
  let unit = -1;
  do {
    value /= 1024;
    unit += 1;
  } while (value >= 1024 && unit < units.length - 1);
  return `${value >= 10 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}