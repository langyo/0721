/**
 * The HTTP client for the REST whitelist (login / refresh / logout /
 * multipart upload / health).
 *
 * The session credential lives in the httpOnly cookie the login plants;
 * it rides every same-origin request automatically — this client never
 * touches a token (no Authorization header, no query parameter, no
 * storage). A 401 anywhere clears the in-memory session and bounces to
 * /login.
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

/** Registers the global 401 reaction (the auth store wires logout here). */
export function onUnauthorized(handler: () => void): void {
  unauthorizedHandler = handler;
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

export async function apiFetch<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const headers: Record<string, string> = {};
  if (options.body !== undefined) headers["Content-Type"] = "application/json";
  const response = await fetch(path, {
    method: options.method ?? (options.body !== undefined ? "POST" : "GET"),
    headers,
    // same-origin: the session cookie rides along.
    credentials: "same-origin",
    body: options.body !== undefined ? JSON.stringify(options.body) : undefined,
  });
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

/**
 * Multipart upload with per-request progress. `fetch` exposes no upload
 * progress events, so the upload lane rides XMLHttpRequest; the session
 * cookie rides the same-origin request automatically. The REST whitelist
 * keeps the binary body off the JSON control lane.
 */
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
