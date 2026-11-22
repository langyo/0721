/**
 * REST-whitelist calls: login mints an httpOnly session cookie (the browser
 * never sees the token), refresh rotates that cookie, logout clears it and
 * kills the session server-side. Everything else rides the JSON-RPC lane
 * (`transport/`), which authenticates with the same cookie.
 */
import type {
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

export function verify(): Promise<UserDto> {
  return wsCall<UserDto>("auth.verify");
}

/** Cookie rotation: the REST whitelist owns refresh — a WS method
 *  cannot Set-Cookie, so the RPC table refuses the name. */
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
 *  the REST whitelist — a JSON control lane carries no progress events. */
export function uploadFile(file: File, onProgress?: (fraction: number) => void): Promise<void> {
  return apiUpload("/api/v1/files", file, onProgress);
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
export type { ApiKeyCreated, ApiKeyPublic } from "@/types";

export function createKey(name: string): Promise<ApiKeyCreated> {
  return wsCall<ApiKeyCreated>('keys.create', { name });
}
export function listKeys(): Promise<ApiKeyPublic[]> {
  return wsCall<ApiKeyPublic[]>('keys.list');
}
export function revokeKey(id: string): Promise<void> {
  return wsCall<void>('keys.revoke', { id });
}

export function health(): Promise<HealthReport> {
  return apiFetch<HealthReport>("/api/health");
}
