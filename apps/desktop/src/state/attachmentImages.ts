// The pictures a replayed message names (`![image](file://…)`), read back from the computer the
// thread runs on: this one's engine, or a paired machine's over its connection
// (state/attachmentTransfer.ts). Each one is read once per computer and kept as an object URL for
// as long as the app runs.

import { ConnectionClosedError } from "../acp/jsonrpc";
import { readImage } from "./attachmentTransfer";
import { connectionForThread } from "./store";
import { splitThreadKey } from "./threadKeys";

/** `null`: the computer answered that the image is not there (or cannot serve it). */
const loaded = new Map<string, string | null>();
const loading = new Map<string, Promise<string | null>>();

function keyOf(threadKey: string, path: string): string {
  return `${splitThreadKey(threadKey).host}\u0000${path}`;
}

/** What is known of an image now: its URL, `null` when it is gone, `undefined` before an answer. */
export function storedImageNow(threadKey: string, path: string): string | null | undefined {
  return loaded.get(keyOf(threadKey, path));
}

/**
 * The image's object URL, read from the thread's computer. `null` when that computer says the image
 * is not there, or runs a RustCode that cannot hand it back. A computer that is not connected is
 * asked again the next time, so its pictures come once it is back.
 */
export function storedImage(threadKey: string, path: string): Promise<string | null> {
  const key = keyOf(threadKey, path);
  if (loaded.has(key)) return Promise.resolve(loaded.get(key) ?? null);
  const pending = loading.get(key);
  if (pending) return pending;
  const conn = connectionForThread(threadKey);
  if (!conn) return Promise.resolve(null);
  const read = readImage((method, params, timeout) => conn.request(method, params, timeout), path)
    .then(({ mimeType, bytes }) => {
      const url = URL.createObjectURL(new Blob([bytes as BlobPart], { type: mimeType }));
      loaded.set(key, url);
      return url;
    })
    .catch((error: unknown) => {
      // A dropped connection is not an answer about the image.
      if (!(error instanceof ConnectionClosedError)) loaded.set(key, null);
      return null;
    })
    .finally(() => loading.delete(key));
  loading.set(key, read);
  return read;
}
