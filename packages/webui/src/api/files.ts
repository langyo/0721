/** The netdisk file API (files.* RPC methods + upload REST). */
import type { FileDto, PageResult, QuotaInfo, ShareDto } from "@/types";
import { wsCall } from "@/composables/useJsonRpc";
import { apiUpload } from "./client";

/** `bucket` is the §7 pushdown's explicit slug (P2a): the server filters
 *  rows AND total to that visible bucket (an invisible/unreadable slug
 *  answers an empty page — never an error). */
export function listFolder(
  folderId: string | null,
  kind?: string,
  offset = 0,
  limit = 100,
  bucket?: string,
): Promise<PageResult<FileDto>> {
  return wsCall<PageResult<FileDto>>("files.list", {
    scope: "own",
    folder_id: folderId,
    offset,
    limit,
    ...(kind ? { kind } : {}),
    ...(bucket ? { bucket } : {}),
  });
}

export interface ChainEntry {
  id: string;
  name: string;
}

/** `files.chain` gates on the TARGET folder's own bucket (§7's column);
 *  the chosen root needs no extra param here. */
export function folderChain(folderId: string): Promise<ChainEntry[]> {
  return wsCall<ChainEntry[]>("files.chain", { folder_id: folderId });
}

export function listTimeline(offset: number, limit: number): Promise<PageResult<FileDto>> {
  return wsCall<PageResult<FileDto>>("files.list", { scope: "timeline", offset, limit });
}

export function createFolder(name: string, parentId: string | null): Promise<FileDto> {
  return wsCall<FileDto>("files.create_folder", { name, parent_id: parentId });
}

export function renameFile(id: string, name: string): Promise<FileDto> {
  return wsCall<FileDto>("files.update", { id, name });
}

export function moveFile(id: string, parentId: string | null): Promise<FileDto> {
  return wsCall<FileDto>("files.move", { id, parent_id: parentId });
}

export function deleteFile(id: string): Promise<void> {
  return wsCall<void>("files.delete", { id });
}

/** Multipart upload into the netdisk tree. `bucket` is the R-I override
 *  (`?bucket=<slug>`); when absent the server resolves the default. */
export function uploadTo(
  file: File,
  folderId: string | null,
  onProgress?: (f: number) => void,
  bucket?: string,
): Promise<void> {
  const params = new URLSearchParams();
  if (folderId) params.set("folder_id", folderId);
  if (bucket) params.set("bucket", bucket);
  const query = params.size > 0 ? `?${params.toString()}` : "";
  return apiUpload(`/api/v1/files${query}`, file, onProgress);
}

// ── shares ──

export function shareCreate(fileId: string, allowPreview: boolean, expiresAt?: string): Promise<ShareDto> {
  return wsCall<ShareDto>("files.share_create", {
    file_id: fileId,
    allow_preview: allowPreview,
    ...(expiresAt ? { expires_at: expiresAt } : {}),
  });
}

export function shareList(): Promise<ShareDto[]> {
  return wsCall<ShareDto[]>("files.share_list");
}

export function shareDelete(token: string): Promise<void> {
  return wsCall<void>("files.share_delete", { token });
}

// ── quota ──

export function quotaGet(userId?: string): Promise<QuotaInfo> {
  return wsCall<QuotaInfo>("files.quota_get", userId ? { user_id: userId } : {});
}

export function quotaPut(userId: string, quotaBytes: number | null): Promise<void> {
  return wsCall<void>("files.quota_put", { user_id: userId, quota_bytes: quotaBytes });
}

// ── public share endpoints (REST, no auth) ──

export interface ShareMeta {
  file_name: string;
  file_size: number;
  kind: string;
  mime: string;
  allow_preview: boolean;
  expires_at: string | null;
}

export async function shareMeta(token: string): Promise<ShareMeta> {
  const resp = await fetch(`/api/v1/share/${encodeURIComponent(token)}/meta`);
  if (!resp.ok) throw new Error("not found");
  return resp.json();
}

export function shareDownloadUrl(token: string): string {
  return `/api/v1/share/${encodeURIComponent(token)}/download`;
}
