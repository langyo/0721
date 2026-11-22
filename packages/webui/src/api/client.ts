/**
 * The HTTP client for the `/api` surface.
 *
 * Every request carries the bearer token from the auth store's persistent
 * session; a 401 anywhere clears the session and bounces to /login (the
 * server invalidates tokens at the watermark — the client never tries to
 * outlive one).
 */

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

/** The persisted session (token + user), read back at boot. */
export interface StoredSession {
  token: string;
}

const SESSION_KEY = "i0721-session";

export function loadSession(): StoredSession | null {
  try {
    const raw = localStorage.getItem(SESSION_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as StoredSession;
    return typeof parsed.token === "string" && parsed.token ? parsed : null;
  } catch {
    return null;
  }
}

export function saveSession(session: StoredSession | null): void {
  try {
    if (session) localStorage.setItem(SESSION_KEY, JSON.stringify(session));
    else localStorage.removeItem(SESSION_KEY);
  } catch {
    // Private mode: the session survives for this tab only.
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
  /** Skip the bearer header (the login call itself). */
  anonymous?: boolean;
}

export async function apiFetch<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const headers: Record<string, string> = {};
  if (!options.anonymous) {
    const session = loadSession();
    if (session) headers.Authorization = `Bearer ${session.token}`;
  }
  if (options.body !== undefined) headers["Content-Type"] = "application/json";
  const response = await fetch(path, {
    method: options.method ?? (options.body !== undefined ? "POST" : "GET"),
    headers,
    body: options.body !== undefined ? JSON.stringify(options.body) : undefined,
  });
  if (response.status === 401 && !options.anonymous) {
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
 * progress events, so the upload lane rides XMLHttpRequest; the API shape
 * (reject with ApiError) matches apiFetch.
 */
export function apiUpload(
  path: string,
  file: File,
  onProgress?: (fraction: number) => void,
): Promise<void> {
  return new Promise((resolve, reject) => {
    const session = loadSession();
    const xhr = new XMLHttpRequest();
    xhr.open("POST", path);
    if (session) xhr.setRequestHeader("Authorization", `Bearer ${session.token}`);
    xhr.upload.addEventListener("progress", (event) => {
      if (event.lengthComputable && onProgress) {
        onProgress(event.loaded / event.total);
      }
    });
    xhr.addEventListener("load", () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        resolve();
      } else {
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
