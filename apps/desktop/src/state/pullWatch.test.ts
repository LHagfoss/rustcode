import { afterEach, expect, test } from "bun:test";
import { AcpConnection, type JsonObject } from "../acp/jsonrpc";
import { deliverPullWatch, PULL_WATCHES, subscribePullWatches } from "./pullWatch";
import { resetForTests, useApp, watchMachineConnection } from "./store";
import { emptyThread } from "./thread";

const watch = { sessionId: "pr-thread", number: 7, link: true };
afterEach(() => resetForTests());

test("subscribes and claims events once across notifications and reconnects", async () => {
  resetForTests();
  const t = { ...emptyThread("pr-thread", "/tmp"), loaded: true, running: true };
  useApp.setState({ threads: { [t.id]: t } });
  const calls: JsonObject[] = [];
  let delivered = false;
  let conn: AcpConnection;
  conn = new AcpConnection((line) => {
    const frame = JSON.parse(line) as JsonObject;
    calls.push(frame);
    const taking = (frame.params as JsonObject).take === 7;
    const result = taking
      ? { events: delivered ? [] : [{ message: "CI failed" }, { message: "Review requested" }] }
      : { watches: [watch] };
    if (taking) delivered = true;
    queueMicrotask(() => conn.receive(JSON.stringify({ jsonrpc: "2.0", id: frame.id, result })));
  });
  await subscribePullWatches(conn);
  await deliverPullWatch(conn, { watch });
  await subscribePullWatches(conn);
  expect(calls[0]?.method).toBe(PULL_WATCHES);
  expect(useApp.getState().threads[t.id]?.queue.map((q) => q.text)).toEqual([
    "Pull request #7: CI failed",
    "Pull request #7: Review requested",
  ]);
  await deliverPullWatch(conn, { watch: { ...watch, link: false } });
  expect(calls).toHaveLength(5);
  conn.close("test finished");
});

test("session_info_update applies review live without session/list", () => {
  resetForTests();
  const conn = new AcpConnection(() => {});
  watchMachineConnection("machine", conn);
  useApp.setState({ sessions: [{ id: "machine/pr-thread", cwd: "/tmp", status: "idle" }] });
  conn.receive(
    JSON.stringify({
      jsonrpc: "2.0",
      method: "session/update",
      params: {
        sessionId: "pr-thread",
        update: { sessionUpdate: "session_info_update", _meta: { rustcode: { status: "review" } } },
      },
    }),
  );
  expect(useApp.getState().sessions[0]?.status).toBe("review");
  conn.close("test finished");
});

test("watch notifications route to the linked thread on their own machine", async () => {
  resetForTests();
  const id = "machine/pr-thread";
  let conn: AcpConnection;
  let claimed = false;
  conn = new AcpConnection((line) => {
    const frame = JSON.parse(line) as JsonObject;
    const taking = (frame.params as JsonObject).take === 7;
    const result = taking ? { events: claimed ? [] : [{ message: "Checks failed" }] } : { watches: [] };
    if (taking) claimed = true;
    queueMicrotask(() => conn.receive(JSON.stringify({ jsonrpc: "2.0", id: frame.id, result })));
  });
  watchMachineConnection("machine", conn);
  useApp.setState({ threads: { [id]: { ...emptyThread(id, "/tmp"), loaded: true, running: true } } });
  const notification = JSON.stringify({ jsonrpc: "2.0", method: "_rustcode/pulls/watch", params: { watch } });
  conn.receive(notification);
  conn.receive(notification);
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(useApp.getState().threads[id]?.queue.map((row) => row.text)).toEqual([
    "Pull request #7: Checks failed",
  ]);
  expect(useApp.getState().threads[watch.sessionId]).toBeUndefined();
  conn.close("test finished");
});
