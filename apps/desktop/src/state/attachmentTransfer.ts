// Images a thread's messages carry, moved over an engine connection in chunks
// (`_rustcode/attachments/read` and `/upload`, engine/src/acp/attachments.rs).
//
// A paired computer's connection takes frames of at most 1 MiB, so an image never travels whole:
// a remote thread's pictures are read back, and a picture sent to one is uploaded before the
// prompt, 512 KiB at a time. The prompt then names the kept file by its URI instead of carrying
// the bytes. No store import here: the store and the transcript both hand in their connection.

import type { Json, JsonObject } from "../acp/jsonrpc";

export const ATTACHMENT_READ = "_rustcode/attachments/read";
export const ATTACHMENT_UPLOAD = "_rustcode/attachments/upload";

/** Base64 characters per uploaded chunk: 524,286 bytes, the engine's chunk, and a multiple of 4. */
export const UPLOAD_CHUNK_CHARS = 174_762 * 4;

export type Requester = (method: string, params: JsonObject, timeoutMs?: number) => Promise<Json>;

function field(value: Json | undefined, key: string): Json | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? value[key] : undefined;
}

/**
 * Upload one image's base64 in chunks and answer the `file://` URI the engine kept it under. Each
 * chunk continues where the engine says the file ends, so a short write is never skipped.
 */
export async function uploadImage(request: Requester, mimeType: string, data: string): Promise<string> {
  let uploadId: string | undefined;
  let at = 0;
  let offset = 0;
  for (;;) {
    const piece = data.slice(at, at + UPLOAD_CHUNK_CHARS);
    const done = at + UPLOAD_CHUNK_CHARS >= data.length;
    const answer = await request(
      ATTACHMENT_UPLOAD,
      { ...(uploadId ? { uploadId } : {}), mimeType, offset, data: piece, done },
      30_000,
    );
    const id = field(answer, "uploadId");
    const received = field(answer, "received");
    if (typeof id !== "string" || typeof received !== "number") {
      throw new Error("The other computer answered the image upload with nothing.");
    }
    uploadId = id;
    offset = received;
    at += UPLOAD_CHUNK_CHARS;
    if (done) {
      const uri = field(answer, "uri");
      if (typeof uri !== "string") throw new Error("The other computer did not keep the image.");
      return uri;
    }
  }
}

/**
 * The prompt with each inline image uploaded first and named by its URI, for a thread on another
 * computer. Other blocks are passed through as they are.
 */
export async function uploadPromptImages(request: Requester, blocks: Json[]): Promise<Json[]> {
  const out: Json[] = [];
  for (const block of blocks) {
    const type = field(block, "type");
    const data = field(block, "data");
    const mimeType = field(block, "mimeType");
    if (type === "image" && typeof data === "string" && data && typeof mimeType === "string") {
      const uri = await uploadImage(request, mimeType, data);
      out.push({ type: "image", mimeType, data: "", uri });
    } else {
      out.push(block);
    }
  }
  return out;
}

/** One stored image, read whole: its MIME type and bytes. */
export async function readImage(
  request: Requester,
  path: string,
  /** The thread whose reply names the picture: the engine then serves one in its folders too. */
  sessionId?: string,
): Promise<{ mimeType: string; bytes: Uint8Array }> {
  const parts: Uint8Array[] = [];
  let offset = 0;
  let mimeType = "application/octet-stream";
  for (let guard = 0; guard < 64; guard += 1) {
    const answer = await request(
      ATTACHMENT_READ,
      { path, offset, ...(sessionId ? { sessionId } : {}) },
      30_000,
    );
    const data = field(answer, "data");
    const mime = field(answer, "mimeType");
    if (typeof data !== "string") throw new Error("The image could not be read.");
    if (typeof mime === "string") mimeType = mime;
    const bytes = decodeBase64(data);
    parts.push(bytes);
    offset += bytes.length;
    if (field(answer, "done") === true || bytes.length === 0) break;
  }
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const bytes = new Uint8Array(total);
  let at = 0;
  for (const part of parts) {
    bytes.set(part, at);
    at += part.length;
  }
  return { mimeType, bytes };
}

export function decodeBase64(data: string): Uint8Array {
  const text = atob(data);
  const bytes = new Uint8Array(text.length);
  for (let i = 0; i < text.length; i += 1) bytes[i] = text.charCodeAt(i);
  return bytes;
}
