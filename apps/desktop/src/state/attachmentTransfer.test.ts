import { describe, expect, test } from "bun:test";
import type { Json, JsonObject } from "../acp/jsonrpc";
import {
  ATTACHMENT_READ,
  ATTACHMENT_UPLOAD,
  decodeBase64,
  readImage,
  UPLOAD_CHUNK_CHARS,
  uploadImage,
  uploadPromptImages,
} from "./attachmentTransfer";

/** An engine that keeps one upload and serves one stored file, as acp/attachments.rs does. */
function engine(stored: Uint8Array = new Uint8Array()) {
  const calls: { method: string; params: JsonObject }[] = [];
  let kept = "";
  const request = async (method: string, params: JsonObject): Promise<Json> => {
    calls.push({ method, params });
    if (method === ATTACHMENT_UPLOAD) {
      const offset = params.offset as number;
      if (offset !== decodeBase64(kept).length) throw new Error(`offset ${offset}`);
      kept += params.data as string;
      const received = decodeBase64(kept).length;
      return params.done
        ? {
            uploadId: "1_0",
            received,
            uri: "file:///c/attachments/upload_1_0.png",
            path: "/c/attachments/upload_1_0.png",
          }
        : { uploadId: "1_0", received };
    }
    if (method === ATTACHMENT_READ) {
      const offset = params.offset as number;
      const chunk = stored.slice(offset, offset + 4);
      return {
        mimeType: "image/png",
        size: stored.length,
        offset,
        data: btoa(String.fromCharCode(...chunk)),
        done: offset + chunk.length >= stored.length,
      };
    }
    throw new Error(`unexpected ${method}`);
  };
  return { request, calls, kept: () => kept };
}

describe("attachment transfer", () => {
  test("a large image is uploaded in chunks that each continue the file", async () => {
    const bytes = new Uint8Array(UPLOAD_CHUNK_CHARS).map((_, i) => i % 251);
    let binary = "";
    for (const b of bytes) binary += String.fromCharCode(b);
    const data = btoa(binary);
    const e = engine();
    const uri = await uploadImage(e.request, "image/png", data);
    expect(uri).toBe("file:///c/attachments/upload_1_0.png");
    expect(e.calls.length).toBe(2);
    expect(e.calls[0]!.params.uploadId).toBeUndefined();
    expect(e.calls[1]!.params.uploadId).toBe("1_0");
    expect(e.calls[1]!.params.done).toBe(true);
    // Every chunk stays well under the remote connection's 1 MiB frame.
    for (const call of e.calls) expect(JSON.stringify(call.params).length).toBeLessThan(800_000);
    expect(e.kept()).toBe(data);
  });

  test("a prompt's images go by reference and its text is untouched", async () => {
    const e = engine();
    const blocks = await uploadPromptImages(e.request, [
      { type: "text", text: "look" },
      { type: "image", mimeType: "image/png", data: btoa("png") },
    ]);
    expect(blocks).toEqual([
      { type: "text", text: "look" },
      { type: "image", mimeType: "image/png", data: "", uri: "file:///c/attachments/upload_1_0.png" },
    ]);
  });

  test("a stored image is read back whole from its chunks", async () => {
    const stored = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    const e = engine(stored);
    const image = await readImage(e.request, "/c/attachments/a.png");
    expect(image.mimeType).toBe("image/png");
    expect(Array.from(image.bytes)).toEqual(Array.from(stored));
    expect(e.calls.map((c) => c.params.offset)).toEqual([0, 4, 8]);
    expect(e.calls.some((c) => "sessionId" in c.params)).toBe(false);
  });

  test("a reply's picture names its thread on every chunk", async () => {
    const e = engine(new Uint8Array([1, 2, 3, 4, 5]));
    await readImage(e.request, "/tmp/shots/a.png", "thread-1");
    expect(e.calls.map((c) => c.params)).toEqual([
      { path: "/tmp/shots/a.png", offset: 0, sessionId: "thread-1" },
      { path: "/tmp/shots/a.png", offset: 4, sessionId: "thread-1" },
    ]);
  });
});
