import { describe, expect, it, vi, beforeEach } from "vitest";

// The §7 pushdown's wire contract (P2a): the chosen root slug rides
// `files.list` as `bucket` and the upload as `?bucket=`. The server
// derives the visible set and filters rows AND total together — the
// client only ever narrows to a slug.
//
// MUT:B-P2A-ROOT-PARAM — dropping the slug from either call reddens
// these.

const wsCallMock = vi.fn();
vi.mock("@/composables/useJsonRpc", () => ({
  wsCall: (...a: unknown[]) => wsCallMock(...a),
}));
const uploadMock = vi.fn().mockResolvedValue(undefined);
vi.mock("./client", () => ({
  apiUpload: (...a: unknown[]) => uploadMock(...a),
}));

import { listFolder, uploadTo } from "./files";

beforeEach(() => {
  wsCallMock.mockReset().mockResolvedValue({ total: 0, offset: 0, limit: 0, items: [] });
  uploadMock.mockReset().mockResolvedValue(undefined);
});

describe("the per-bucket wire (P2a)", () => {
  it("carries the chosen slug as `bucket` on files.list", async () => {
    await listFolder(null, undefined, 0, 100, "media");
    expect(wsCallMock).toHaveBeenCalledWith("files.list", {
      scope: "own",
      folder_id: null,
      offset: 0,
      limit: 100,
      bucket: "media",
    });
  });

  it("omits `bucket` when no root is chosen (the default resolution)", async () => {
    await listFolder(null);
    const [, params] = wsCallMock.mock.calls[0] as [string, Record<string, unknown>];
    expect("bucket" in params).toBe(false);
  });

  it("carries `?bucket=` on the multipart upload", async () => {
    const file = new File(["x"], "a.png");
    await uploadTo(file, null, undefined, "media");
    const [path] = uploadMock.mock.calls[0] as [string];
    expect(path).toBe("/api/v1/files?bucket=media");
  });

  it("omits the upload query when no bucket or folder is chosen", async () => {
    const file = new File(["x"], "a.png");
    await uploadTo(file, null);
    const [path] = uploadMock.mock.calls[0] as [string];
    expect(path).toBe("/api/v1/files");
  });
});
