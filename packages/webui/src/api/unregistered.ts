/**
 * The unregistered-object surface (P3): admin-only, gated by the
 * `storage.unregistered.get/put` switch (R-E). `storage.unregistered.list`
 * answers `PageResult<UnregisteredObject>`; adopt is the only write (there
 * is NO delete — an unregistered object is adopted first, then removed
 * through the row's reclaim chain).
 */
import type { UnregisteredObject } from "@/types";
import { wsCall } from "@/composables/useJsonRpc";

export function getUnregisteredShow(): Promise<{ show: boolean }> {
  return wsCall<{ show: boolean }>("storage.unregistered.get");
}

export function setUnregisteredShow(show: boolean): Promise<void> {
  return wsCall<void>("storage.unregistered.put", { show });
}

export interface UnregisteredPage {
  total: number;
  offset: number;
  limit: number;
  items: UnregisteredObject[];
}

export function listUnregistered(
  bucket: string,
  offset = 0,
  limit = 50,
): Promise<UnregisteredPage> {
  return wsCall<UnregisteredPage>("storage.unregistered.list", {
    bucket,
    offset,
    limit,
  });
}

export interface AdoptInput {
  bucket: string;
  key: string;
  owner_id?: string;
  parent_id?: string;
  name?: string;
}

export function adoptUnregistered(input: AdoptInput): Promise<void> {
  return wsCall<void>("storage.unregistered.adopt", { ...input });
}
