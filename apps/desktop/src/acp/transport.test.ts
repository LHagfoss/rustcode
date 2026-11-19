import { describe, expect, test } from "bun:test";
import { droppedLineAnswer, HubTransport, hubSession, type TransportEvent } from "./transport";

describe("a line the shell would not send", () => {
  test("a request is answered with the reason", () => {
    const line = JSON.stringify({ jsonrpc: "2.0", id: 7, method: "session/prompt", params: {} });
    expect(JSON.parse(droppedLineAnswer(line, "The line was dropped.") ?? "null")).toEqual({
      jsonrpc: "2.0",
      id: 7,
      error: { code: -32000, message: "The line was dropped." },
    });
  });

  test("a notification, an answer and a broken line have nobody waiting", () => {
    expect(droppedLineAnswer(JSON.stringify({ jsonrpc: "2.0", method: "session/cancel" }), "x")).toBeNull();
    expect(droppedLineAnswer(JSON.stringify({ jsonrpc: "2.0", id: 3, result: null }), "x")).toBeNull();
    expect(droppedLineAnswer("{", "x")).toBeNull();
  });
});

describe("the hub's web app transport", () => {
  class FakeSocket {
    static last: FakeSocket | null = null;
    static OPEN = 1;
    readyState = 1;
    sent: string[] = [];
    onmessage: ((message: { data: string }) => void) | null = null;
    onclose: ((event: { reason: string }) => void) | null = null;
    constructor(readonly url: URL) {
      FakeSocket.last = this;
    }
    send(line: string) {
      this.sent.push(line);
    }
    close() {}
  }

  test("trades the cookie for a ticket, opens the socket and relays lines", async () => {
    const realFetch = globalThis.fetch;
    const realSocket = globalThis.WebSocket;
    const asked: string[] = [];
    globalThis.fetch = (async (input: string, init?: RequestInit) => {
      asked.push(`${init?.method} ${input} ${init?.credentials}`);
      return new Response(JSON.stringify({ ticket: "t+1", session: "w1" }), { status: 200 });
    }) as typeof fetch;
    globalThis.WebSocket = FakeSocket as unknown as typeof WebSocket;
    try {
      const events: TransportEvent[] = [];
      const transport = new HubTransport("https://rc.example.com");
      transport.send("early");
      const started = transport.start((event) => events.push(event));
      await Promise.resolve();
      await new Promise((resolve) => setTimeout(resolve, 0));
      const socket = FakeSocket.last as FakeSocket;
      expect(socket.url.toString()).toBe("wss://rc.example.com/app/acp?ticket=t%2B1");
      expect(socket.sent).toEqual([]);
      socket.onmessage?.({ data: '{"type":"acp_opened"}' });
      await started;
      expect(socket.sent).toEqual(["early"]);
      socket.onmessage?.({ data: '{"jsonrpc":"2.0","id":1,"result":{}}' });
      expect(events).toEqual([
        { type: "open", engine: "rc.example.com" },
        { type: "line", line: '{"jsonrpc":"2.0","id":1,"result":{}}' },
      ]);
      expect(asked).toEqual(["POST https://rc.example.com/app/ticket same-origin"]);
      expect(hubSession()).toBe("w1");
      socket.onclose?.({ reason: "the connection was closed" });
      expect(events.at(-1)).toEqual({ type: "closed", reason: "the connection was closed" });
    } finally {
      globalThis.fetch = realFetch;
      globalThis.WebSocket = realSocket;
    }
  });

  test("a browser that is not signed in is refused", async () => {
    const realFetch = globalThis.fetch;
    globalThis.fetch = (async () => new Response("{}", { status: 401 })) as unknown as typeof fetch;
    try {
      await expect(new HubTransport("https://rc.example.com").start(() => {})).rejects.toThrow("passkey");
    } finally {
      globalThis.fetch = realFetch;
    }
  });
});
