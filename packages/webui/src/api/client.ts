/**
 * The HTTP client for the REST whitelist (login / refresh / logout /
 * multipart upload / health).
 *
 * The session credential lives in the httpOnly cookie the login plants;
 * it rides every same-origin request automatically — this client never
 * touches a token (no Authorization header, no query parameter, no
 * storage).
 *
 * The 401 lane (R-M①) is a two-state ladder: on the FIRST 401 of a wave
 * the client tries `POST /api/v1/auth/refresh` ONCE (concurrent-merged —
 * every 401 that lands while a refresh is already in flight joins the
 * same attempt instead of minting its own) and re-issues the original
 * request once. A successful refresh replays the request; a failed
 * refresh means the session is truly gone (never-signed-in and expired
 * read identically) and the registered auth-lost reaction owns the
 * bounce to /login. No `auth.verify` probing anywhere: refresh both
 * proves and rotates the cookie, so a verify round trip is dead weight.
 */

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

let unauthorizedHandler: (() => void) | null = null;

/** Registers the global auth-lost reaction (the auth store wires its
 *  two-state logout here). */
export function onUnauthorized(handler: () => void): void {
  unauthorizedHandler = handler;
}

/** The in-flight refresh attempt (the concurrency merge): concurrent 401s
 *  share ONE refresh round trip; `null` = no refresh is running. */
let refreshInFlight: Promise<boolean> | null = null;

/** Test seam: drop a wedged in-flight refresh between cases. */
export function resetUnauthorizedState(): void {
  refreshInFlight = null;
}

/** POST the refresh endpoint once per wave. Resolves `true` when the
 *  session was rotated (the retry may proceed), `false` when the refresh
 *  itself was refused — the session is gone and the auth-lost reaction
 *  fires exactly once per wave. */
async function refreshOnce(): Promise<boolean> {
  try {
    const response = await fetch("/api/v1/auth/refresh", {
      method: "POST",
      credentials: "same-origin",
    });
    if (response.ok) return true;
  } catch {
    // Network death during refresh: fall through to the auth-lost lane.
  }
  unauthorizedHandler?.();
  return false;
}

/** The 401 ladder: join-or-start the single refresh, and answer whether
 *  the caller may re-issue its request once. */
async function refreshAndRetry(): Promise<boolean> {
  refreshInFlight ??= refreshOnce().finally(() => {
    refreshInFlight = null;
  });
  return refreshInFlight;
}

async function readError(response: Response): Promise<string> {
  try {
    const body = (await response.json()) as { error?: string; message?: string };
    return body.error ?? body.message ?? response.statusText;
  } catch {
    return response.statusText || `HTTP ${response.status}`;
  }
}

export interface RequestOptions {
  method?: string;
  body?: unknown;
}

async function rawFetch(path: string, options: RequestOptions): Promise<Response> {
  const headers: Record<string, string> = {};
  if (options.body !== undefined) headers["Content-Type"] = "application/json";
  return fetch(path, {
    method: options.method ?? (options.body !== undefined ? "POST" : "GET"),
    headers,
    // same-origin: the session cookie rides along.
    credentials: "same-origin",
    body: options.body !== undefined ? JSON.stringify(options.body) : undefined,
  });
}

export async function apiFetch<T>(path: string, options: RequestOptions = {}): Promise<T> {
  let response = await rawFetch(path, options);
  if (response.status === 401) {
    // R-M①: one refresh attempt (merged across concurrent 401s), then
    // the ORIGINAL request is re-issued exactly once. MUT:p2a-refresh-retry —
    // dropping the replay (or the refresh itself) turns the hook tests red.
    if (await refreshAndRetry()) {
      response = await rawFetch(path, options);
    }
  }
  if (response.status === 401) {
    unauthorizedHandler?.();
    throw new ApiError(401, "unauthorized");
  }
  if (!response.ok) {
    throw new ApiError(response.status, await readError(response));
  }
  if (response.status === 204) return undefined as T;
  const text = await response.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

/** The multipart upload lane for the REST whitelist (progress events via
 *  XHR; the same-origin cookie authenticates). `path` carries the query
 *  string (`?folder_id=…&bucket=…`) the caller resolved. */
export function apiUpload(
  path: string,
  file: File,
  onProgress?: (fraction: number) => void,
): Promise<void> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("POST", path);
    // No Authorization header, no token anywhere: same-origin cookie.
    xhr.withCredentials = false;
    xhr.upload.addEventListener("progress", (event) => {
      if (event.lengthComputable && onProgress) {
        onProgress(event.loaded / event.total);
      }
    });
    xhr.addEventListener("load", () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        resolve();
      } else {
        if (xhr.status === 401) unauthorizedHandler?.();
        let message = `HTTP ${xhr.status}`;
        try {
          const body = JSON.parse(xhr.responseText) as { error?: string };
          if (body.error) message = body.error;
        } catch {
          // Non-JSON error body: keep the status line.
        }
        reject(new ApiError(xhr.status, message));
      }
    });
    xhr.addEventListener("error", () => reject(new ApiError(0, "network error")));
    const form = new FormData();
    form.append("file", file, file.name);
    xhr.send(form);
  });
}
