import { describe, expect, test } from "bun:test";
import { AcpConnection, ConnectionClosedError, type Inbound, RpcError, TimeoutError } from "./jsonrpc";

function pair() {
  const sent: string[] = [];
  const conn = new AcpConnection((line) => sent.push(line));
  const inbound: Inbound[] = [];
  conn.onInbound((m) => inbound.push(m));
  return { conn, sent, inbound };
}

describe("AcpConnection", () => {
  test("resolves a request by id", async () => {
    const { conn, sent } = pair();
    const result = conn.request("initialize", { protocolVersion: 1 });
    const out = JSON.parse(sent[0]!);
    expect(out).toMatchObject({ jsonrpc: "2.0", id: 1, method: "initialize" });
    conn.receive(JSON.stringify({ jsonrpc: "2.0", id: 1, result: { ok: true } }));
    expect(await result).toEqual({ ok: true });
  });

  test("rejects with the engine's error", async () => {
    const { conn } = pair();
    const result = conn.request("_rustcode/harnesses");
    conn.receive(
      JSON.stringify({ jsonrpc: "2.0", id: 1, error: { code: -32601, message: "Method not found" } }),
    );
    const error = await result.catch((e) => e);
    expect(error).toBeInstanceOf(RpcError);
    expect((error as RpcError).isMethodNotFound).toBe(true);
  });

  test("delivers notifications and incoming requests in order", () => {
    const { conn, inbound } = pair();
    conn.receive('{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s"}}');
    conn.receive('{"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{}}');
    conn.receive("not json");
    expect(inbound.map((m) => m.type)).toEqual(["notification", "request"]);
    expect(inbound[1]).toMatchObject({ id: 7, method: "session/request_permission" });
  });

  test("times out a bounded request", async () => {
    const { conn } = pair();
    const error = await conn.request("slow", {}, 5).catch((e) => e);
    expect(error).toBeInstanceOf(TimeoutError);
  });

  test("close fails pending requests and later ones", async () => {
    const { conn, inbound } = pair();
    const pending = conn.request("session/prompt");
    conn.close("engine exited");
    expect(await pending.catch((e) => e)).toBeInstanceOf(ConnectionClosedError);
    expect(await conn.request("x").catch((e) => e)).toBeInstanceOf(ConnectionClosedError);
    expect(inbound.at(-1)).toEqual({ type: "closed", reason: "engine exited" });
  });

  test("responds to engine requests", () => {
    const { conn, sent } = pair();
    conn.respond(9, { outcome: { outcome: "cancelled" } });
    expect(JSON.parse(sent[0]!)).toEqual({
      jsonrpc: "2.0",
      id: 9,
      result: { outcome: { outcome: "cancelled" } },
    });
  });
});
