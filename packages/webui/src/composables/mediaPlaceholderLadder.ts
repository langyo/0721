import { ref } from "vue";

/** One placeholder face the ladder answers with. */
export interface MediaFace {
  kind: "sign-in" | "forbidden" | "unavailable" | "missing";
}

/**
 * The R-M② image placeholder ladder (private buckets), as a pure
 * decision table the views consume.
 *
 * The ladder: ONE refresh attempt first (P2a's merged semantics — one
 * refresh per wave, concurrent attempts share it); refresh refused →
 * the sign-in face (never-signed-in and expired share the copy);
 * refresh OK → retry `/media` once and translate the STATUS:
 * 403 → forbidden, 503 → unavailable, 404 (or any other failure) →
 * missing.
 */

export type LadderFace = NonNullable<ReturnType<typeof classify>>;

/** The retry status → face. `null` = the retry succeeded (2xx). */
function classify(status: number): MediaFace["kind"] | null {
  if (status === 403) return "forbidden";
  if (status === 503) return "unavailable";
  return "missing";
}

export function useMediaPlaceholderLadder() {
  const faces = ref<Map<string, MediaFace>>(new Map());

  /**
   * Run the ladder for one failed image URL. The injected loaders keep
   * the decision pure: `refresh` answers whether the session refresh
   * succeeded, `probeMedia` returns the retry's HTTP status.
   */
  async function resolve(
    url: string,
    refresh: () => Promise<boolean>,
    probeMedia: (url: string) => Promise<number>,
  ): Promise<MediaFace | null> {
    const refreshed = await refresh();
    if (!refreshed) {
      const face: MediaFace = { kind: "sign-in" };
      faces.value = new Map(faces.value).set(url, face);
      return face;
    }
    const status = await probeMedia(url);
    const kind = status >= 200 && status < 300 ? null : classify(status);
    // EVERY mutation replaces the Map: the views' computeds read
    // `faceOf` through the ref, and an in-place delete would never
    // re-trigger them.
    const next = new Map(faces.value);
    if (kind === null) {
      next.delete(url);
      faces.value = next;
      return null;
    }
    const face: MediaFace = { kind };
    faces.value = next.set(url, face);
    return face;
  }

  function faceOf(url: string): MediaFace | null {
    return faces.value.get(url) ?? null;
  }

  function clear(url: string): void {
    const next = new Map(faces.value);
    next.delete(url);
    faces.value = next;
  }

  return { faces, resolve, faceOf, clear };
}
