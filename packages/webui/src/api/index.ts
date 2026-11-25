/**
 * REST-whitelist calls: login mints an httpOnly session cookie (the browser
 * never sees the token), refresh rotates that cookie, logout clears it and
 * kills the session server-side. Everything else rides the JSON-RPC lane
 * (`transport/`), which authenticates with the same cookie.
 */
import type {
  AccountMethods,
  AuthSession,
  CreateUserRequest,
  FileDto,
  HealthReport,
  LoginRequest,
  LoginMethods,
  OAuthProviderPublic,
  OAuthProviderUpsert,
  PageResult,
  SiteConfig,
  SiteConfigStrict,
  UserDto,
} from "@/types";

import { wsCall } from "@/composables/useJsonRpc";
import { apiFetch, apiUpload } from "./client";

// ── auth (login = REST bootstrap; the rest ride the WS lane) ───────────

export function login(request: LoginRequest): Promise<AuthSession> {
  return apiFetch<AuthSession>("/api/v1/auth/login", {
    method: "POST",
    body: request,
  });
}

/** Cookie rotation: the REST whitelist owns refresh — a WS method
 *  cannot Set-Cookie, so the RPC table refuses the name. The response
 *  body carries the user, so this is ALSO the session probe (R-M①): no
 *  `auth.verify` round trip exists anywhere any more. */
export function refresh(): Promise<AuthSession> {
  return apiFetch<AuthSession>("/api/v1/auth/refresh", { method: "POST" });
}

/** Sign out: the server expires the httpOnly cookie and supersedes
 *  every outstanding token. */
export function logout(): Promise<void> {
  return apiFetch<void>("/api/v1/auth/logout", { method: "POST" });
}

// ── files ──────────────────────────────────────────────────────────────

export function listFiles(offset: number, limit: number): Promise<PageResult<FileDto>> {
  return wsCall<PageResult<FileDto>>("files.list", { offset, limit });
}

export function renameFile(id: string, name: string): Promise<FileDto> {
  return wsCall<FileDto>("files.update", { id, name });
}

export function deleteFile(id: string): Promise<void> {
  return wsCall<void>("files.delete", { id });
}

/** Multipart upload with per-file progress (XHR): binary bodies stay on
 *  the REST whitelist — a JSON control lane carries no progress events.
 *  `bucket` is the R-I override: the chosen bucket goes as `?bucket=`
 *  and the server's `resolve_upload_bucket` resolves/refuses it. */
export function uploadFile(
  file: File,
  onProgress?: (fraction: number) => void,
  bucket?: string,
): Promise<void> {
  const query = bucket ? `?bucket=${encodeURIComponent(bucket)}` : "";
  return apiUpload(`/api/v1/files${query}`, file, onProgress);
}


// ── users ──────────────────────────────────────────────────────────────

export function listUsers(offset: number, limit: number): Promise<PageResult<UserDto>> {
  return wsCall<PageResult<UserDto>>("users.list", { offset, limit });
}

export function createUser(request: CreateUserRequest): Promise<UserDto> {
  return wsCall<UserDto>("users.create", request as unknown as Record<string, unknown>);
}

export function deleteUser(id: string): Promise<void> {
  return wsCall<void>("users.delete", { id });
}

// ── settings ───────────────────────────────────────────────────────────

export function getSiteConfig(): Promise<SiteConfig> {
  return wsCall<SiteConfig>("settings.get");
}

export function putSiteConfig(strict: SiteConfigStrict): Promise<void> {
  return wsCall<void>(
    "settings.put",
    strict as unknown as Record<string, unknown>,
  );
}

// ── login methods & providers (the dual-mode surface) ───────────────

/** The public login-page descriptor (no secret material). */
export function loginMethods(): Promise<LoginMethods> {
  return apiFetch<LoginMethods>("/api/v1/auth/login-methods");
}

/** The account-first ladder's first step: what sign-in factors the named
 *  account has. Public (no credential yet) and rate-limited exactly like
 *  `login`; the server answers unknown names with a password factor so the
 *  call is not an enumeration oracle. */
export function accountMethods(username: string): Promise<AccountMethods> {
  return apiFetch<AccountMethods>("/api/v1/auth/account-methods", {
    method: "POST",
    body: { username },
  });
}

export function listProviders(): Promise<OAuthProviderPublic[]> {
  return wsCall<OAuthProviderPublic[]>("providers.list");
}

export function upsertProvider(row: OAuthProviderUpsert): Promise<OAuthProviderPublic> {
  return wsCall<OAuthProviderPublic>(
    "providers.upsert",
    row as unknown as Record<string, unknown>,
  );
}

export function deleteProvider(id: string): Promise<void> {
  return wsCall<void>("providers.delete", { id });
}

export function getPasswordLogin(): Promise<boolean> {
  return wsCall<boolean>("auth.password.get");
}

export function putPasswordLogin(enabled: boolean): Promise<void> {
  return wsCall<void>("auth.password.put", { enabled });
}

// ── API keys (the MCP wave) ─────────────────────────────────────────

// The wire shapes are the ts-rs bindings — never hand-written here
// (AGENTS 「类型单向」): the generated barrel is the single source.
// (Re-export AND import: `export ... from` does not bind the local
// names these functions below reference.)
import type { ApiKeyCreated, ApiKeyPublic, KeyOp } from "@/types";
export type { ApiKeyCreated, ApiKeyPublic, KeyOp } from "@/types";

/**
 * The write payload `keys.create` / `keys.update` take.
 *
 * Both methods are FULL REPLACEMENTS of the same three fields (the
 * server validates them through one shared validator), so one shape
 * serves both. `expires_at` is the only optional member:
 *  - omitted → no expiry;
 *  - `""` → no expiry too (the server reads an explicitly empty
 *    string as the webui's cleared `datetime-local` field), which is how
 *    the edit dialog CLEARS an expiry without a second argument.
 * The plaintext key is not part of any write — it exists only in the
 * `keys.create` RESPONSE, once.
 */
export interface KeyWrite {
  name: string;
  ops: KeyOp[];
  expires_at?: string;
  /**
   * The explicit `scopes.buckets` reference (Q7). PRESENT (including
   * `[]` = no buckets) → the caller's pick, sent as-is; OMITTED → the
   * server materializes the owner's current granted buckets. The edit
   * dialog for a K-era key (no `buckets` array in its scopes) submits
   * with the field present so the edit materializes/pins explicitly.
   */
  buckets?: string[];
}

export function createKey(payload: KeyWrite): Promise<ApiKeyCreated> {
  return wsCall<ApiKeyCreated>('keys.create', { ...payload });
}
export function listKeys(): Promise<ApiKeyPublic[]> {
  return wsCall<ApiKeyPublic[]>('keys.list');
}
/** Replace a key's name, permission set and expiry in one call. */
export function updateKey(id: string, payload: KeyWrite): Promise<void> {
  return wsCall<void>('keys.update', { id, ...payload });
}
export function revokeKey(id: string): Promise<void> {
  return wsCall<void>('keys.revoke', { id });
}

export function health(): Promise<HealthReport> {
  return apiFetch<HealthReport>("/api/health");
}
