import { describe, expect, test } from "bun:test";
import { droppedLineAnswer } from "./transport";

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
