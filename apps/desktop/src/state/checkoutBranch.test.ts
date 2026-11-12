import { afterEach, expect, test } from "bun:test";
import { AcpConnection, type JsonObject } from "../acp/jsonrpc";
import { readCheckoutBranch } from "./checkoutBranch";
import { resetForTests, watchMachineConnection } from "./store";

afterEach(() => resetForTests());

test("checkout branch reads are shared and a non-git folder has no branch", async () => {
  const frames: JsonObject[] = [];
  const conn = new AcpConnection((line) => {
    const frame = JSON.parse(line) as JsonObject;
    if (frame.method === "_rustcode/project/read") frames.push(frame);
  });
  const host = "gw-123456789abcdef0";
  watchMachineConnection(host, conn);
  const first = readCheckoutBranch(`${host}/a`, "/tmp/non-git", host);
  const second = readCheckoutBranch(`${host}/b`, "/tmp/non-git", host);
  expect(frames).toHaveLength(1);
  conn.receive(JSON.stringify({ jsonrpc: "2.0", id: frames[0]!.id, result: { branch: null } }));
  expect(await first).toBeUndefined();
  expect(await second).toBeUndefined();
});
