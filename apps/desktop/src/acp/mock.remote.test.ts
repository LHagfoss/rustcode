// The mock engine's remote access answers, which is what the Settings page
// reads under `VITE_TRANSPORT=mock`: status, start, pair and stop, in the
// shapes `_rustcode/remote*` answers with (rustcode/engine/src/acp/remote.rs).

import { describe, expect, test } from "bun:test";
import type { Json, JsonObject } from "./jsonrpc";
import { type MockOptions, MockTransport } from "./mock";
import { parseRemoteStatus } from "./remote";

function mockEngine(options: MockOptions = {}) {
  const transport = new MockTransport({ delayMs: 0, ...options });
  const waiting = new Map<number, (value: Json) => void>();
  let nextId = 1;
  let opened: (() => void) | null = null;
  const open = new Promise<void>((resolve) => {
    opened = resolve;
  });
  const receive = (line: string) => {
    const message = JSON.parse(line) as { id?: number; result?: Json; error?: Json };
    if (typeof message.id !== "number") return;
    waiting.get(message.id)?.(message.result ?? message.error ?? null);
    waiting.delete(message.id);
  };
  const started = transport.start((event) => {
    if (event.type === "open") opened?.();
    else if (event.type === "line") receive(event.line);
    else if (event.type === "lines") event.lines.forEach(receive);
  });
  return {
    async request(method: string, params: JsonObject = {}): Promise<Json> {
      await Promise.all([started, open]);
      const id = nextId++;
      return await new Promise<Json>((resolve, reject) => {
        const timer = setTimeout(() => {
          waiting.delete(id);
          reject(new Error(`${method} timed out`));
        }, 2000);
        waiting.set(id, (value) => {
          clearTimeout(timer);
          resolve(value);
        });
        transport.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
      });
    },
    async done() {
      await transport.stop();
    },
  };
}

describe("mock remote access", () => {
  test("status starts idle, with the paired phone still listed", async () => {
    const engine = mockEngine();
    try {
      const status = parseRemoteStatus(
        await engine.request("_rustcode/remote", { sessionId: "mock-thread-1" }),
      );
      expect(status.available).toBe(true);
      expect(status.enabled).toBe(false);
      expect(status.shared).toBe(false);
      expect(status.gateway).toBeUndefined();
      expect(status.devices).toEqual([{ id: "7c1f3a2b", name: "Lars's iPhone" }]);
    } finally {
      await engine.done();
    }
  });

  test("start, pair and stop answer like the engine", async () => {
    const engine = mockEngine();
    try {
      const started = parseRemoteStatus(
        await engine.request("_rustcode/remote/start", {
          sessionId: "mock-thread-1",
          pair: false,
        }),
      );
      expect(started.shared).toBe(true);
      expect(started.enabled).toBe(true);
      expect(started.gateway?.connected).toBe(true);
      expect(started.gateway?.advertisedAddress).toBe("192.168.1.24:17879");
      expect(started.offer).toBeUndefined();

      const paired = parseRemoteStatus(
        await engine.request("_rustcode/remote/pair", { sessionId: "mock-thread-1" }),
      );
      expect(paired.offer?.code).toBe("4927-3318");
      expect(paired.offer?.address).toBe("192.168.1.24:17879");
      expect(paired.offer?.expiresInSecs).toBeGreaterThan(0);
      expect(paired.offer?.qrPayload).toContain('"protocol_version":1');

      // A session that is not shared is refused with a reason, not an RPC error,
      // in exactly the engine's shape (`acp/remote.rs` › `pair`).
      const refused = (await engine.request("_rustcode/remote/pair", {
        sessionId: "mock-thread-4",
      })) as JsonObject;
      expect(Object.keys(refused).sort()).toEqual(["available", "error", "shared"]);
      expect(refused.shared).toBe(false);
      expect(refused.error).toBe("this session is not shared");

      const stopped = parseRemoteStatus(
        await engine.request("_rustcode/remote/stop", { sessionId: "mock-thread-1" }),
      );
      expect(stopped.shared).toBe(false);
      expect(stopped.enabled).toBe(false);
      expect(stopped.gateway).toBeUndefined();
    } finally {
      await engine.done();
    }
  });

  test("an expired code is dropped and reported as expired", async () => {
    const engine = mockEngine({ remote: "expired" });
    try {
      await engine.request("_rustcode/remote/start", { sessionId: "mock-thread-1", pair: false });
      const paired = parseRemoteStatus(
        await engine.request("_rustcode/remote/pair", { sessionId: "mock-thread-1" }),
      );
      expect(paired.pairingExpired).toBe(true);
      expect(paired.offer).toBeUndefined();
    } finally {
      await engine.done();
    }
  });

  test("a disconnected gateway keeps the share but says it is not connected", async () => {
    const engine = mockEngine({ remote: "disconnected" });
    try {
      const started = parseRemoteStatus(
        await engine.request("_rustcode/remote/start", {
          sessionId: "mock-thread-1",
          pair: false,
        }),
      );
      expect(started.shared).toBe(true);
      expect(started.gateway?.connected).toBe(false);
      expect(started.gateway?.attachedDevices).toBe(0);
      expect(started.gateway?.connectedDevices).toBe(0);
    } finally {
      await engine.done();
    }
  });

  test("starting unknown session is the engine's invalid-params error", async () => {
    const engine = mockEngine();
    try {
      const answer = (await engine.request("_rustcode/remote/start", {
        sessionId: "no-such-thread",
      })) as JsonObject;
      expect(answer.code).toBe(-32602);
      expect(String(answer.message)).toContain("Unknown session");
    } finally {
      await engine.done();
    }
  });
});
