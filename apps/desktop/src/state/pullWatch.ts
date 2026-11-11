// Claim through the engine's durable cursor: reconnects and duplicate notifications
// cannot repeat an event, even when another client already handed it to the thread.
import { type AcpConnection, arr, type JsonObject, obj, str } from "../acp/jsonrpc";
import { deliver } from "./host";
import { openThread, useApp } from "./store";
import { threadKey } from "./threadKeys";

export const PULL_WATCHES = "_rustcode/pulls/watches";
export const PULL_WATCH = "_rustcode/pulls/watch";

export async function subscribePullWatches(conn: AcpConnection, host = ""): Promise<void> {
  const answer = obj(await conn.request(PULL_WATCHES, {}, 15_000));
  for (const value of arr(answer?.watches)) {
    await deliverPullWatch(conn, { watch: value }, host);
  }
}

export async function deliverPullWatch(conn: AcpConnection, params: JsonObject, host = ""): Promise<void> {
  const watch = obj(params.watch);
  const sessionId = str(watch?.sessionId);
  const number = watch?.number;
  if (watch?.link !== true || !sessionId || typeof number !== "number") return;
  if (Array.isArray(watch.events) && watch.events.length <= Number(watch.delivered ?? 0)) return;
  const id = threadKey(host, sessionId);
  // Load before claiming: a failed load leaves the durable events pending.
  await openThread(id);
  if (!useApp.getState().threads[id]?.loaded) return;
  const answer = obj(await conn.request(PULL_WATCHES, { sessionId, take: number }, 15_000));
  for (const event of arr(answer?.events)) {
    const message = str(obj(event)?.message);
    if (message) deliver(id, `Pull request #${number}: ${message}`, "", "");
  }
}
