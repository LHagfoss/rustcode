import { describe, expect, test } from "bun:test";
import { bareTool, displayTool, humanizeTool, sessionIdOf, splitToolName, usefulSummary } from "./toolNames";

describe("tool names", () => {
  test("an MCP name splits into its server and tool, in every form the harnesses use", () => {
    expect(splitToolName("mcp__rustcode__start_session")).toEqual({
      server: "rustcode",
      tool: "start_session",
    });
    expect(splitToolName("mcp__mail__search_messages")).toEqual({ server: "mail", tool: "search_messages" });
    expect(splitToolName("rustcode__start_session")).toEqual({ server: "rustcode", tool: "start_session" });
    expect(splitToolName("mail.search_messages")).toEqual({ server: "mail", tool: "search_messages" });
    expect(splitToolName("mail/search_messages")).toEqual({ server: "mail", tool: "search_messages" });
  });

  test("a plain title stays whole, and so does a file name", () => {
    expect(splitToolName("bash")).toEqual({ tool: "bash" });
    expect(splitToolName("Read")).toEqual({ tool: "Read" });
    expect(splitToolName("package.json")).toEqual({ tool: "package.json" });
    expect(bareTool("mcp__rustcode__wait_session")).toBe("wait_session");
  });

  test("snake_case reads as a sentence", () => {
    expect(humanizeTool("read_session")).toBe("Read session");
    expect(humanizeTool("search_messages")).toBe("Search messages");
    expect(humanizeTool("echo")).toBe("Echo");
  });

  test("a summary of fence markers or space says nothing", () => {
    expect(usefulSummary("```")).toBeUndefined();
    expect(usefulSummary("` `\n")).toBeUndefined();
    expect(usefulSummary("   ")).toBeUndefined();
    expect(usefulSummary(undefined)).toBeUndefined();
    expect(usefulSummary("Search for the caller")).toBe("Search for the caller");
  });
});

describe("displayTool", () => {
  test("another server's tool keeps its name and says where it comes from", () => {
    expect(displayTool("mcp__mail__search_messages", { summary: "inbox" })).toEqual({
      title: "Search messages",
      source: "mail",
      summary: "inbox",
    });
  });

  test("a title with no server is left alone", () => {
    expect(displayTool("bash", { command: "ls" })).toEqual({ title: "bash", summary: undefined });
    expect(displayTool("Apply patch")).toEqual({ title: "Apply patch", summary: undefined });
  });

  test("orchestration calls say what they did", () => {
    expect(
      displayTool("mcp__rustcode__start_session", { prompt: "Fix the login bug\nand the rest" }),
    ).toEqual({
      title: "Started thread",
      source: "rustcode",
      summary: "Fix the login bug",
    });
    expect(
      displayTool("mcp__rustcode__start_session", {
        title: "Polish the composer",
        prompt: "…",
        harness: "claude",
        model: "opus",
      }),
    ).toEqual({ title: "Started thread", source: "rustcode", summary: "Polish the composer · claude opus" });
    expect(displayTool("mcp__rustcode__start_session", { prompt: "x".repeat(80) }).summary).toHaveLength(60);
    expect(displayTool("mcp__rustcode__message_session").title).toBe("Messaged thread");
    expect(displayTool("mcp__rustcode__read_session").title).toBe("Read thread");
    expect(displayTool("mcp__rustcode__list_sessions").title).toBe("Listed threads");
    expect(
      displayTool("mcp__rustcode__fan_out", { harnesses: [{ harness: "a" }, { harness: "b" }] }).title,
    ).toBe("Fanned out to 2 harnesses");
  });

  test("a wait speaks in the tense its row is in", () => {
    const input = { link_ids: ["o/1", "o/2", "o/3"] };
    expect(displayTool("mcp__rustcode__wait_session", input).title).toBe("Waited for 3 threads");
    expect(displayTool("mcp__rustcode__wait_session", input, "in_progress").title).toBe(
      "Waiting for 3 threads",
    );
    expect(displayTool("mcp__rustcode__wait_session", {}).title).toBe("Waited for 1 thread");
  });

  test("a bare orchestration name is RustCode's own", () => {
    expect(displayTool("start_session", { title: "T" })).toMatchObject({ title: "Started thread" });
  });
});

describe("sessionIdOf", () => {
  test("finds the thread a call started, however the output is wrapped", () => {
    expect(sessionIdOf('{"session_id":"abc","title":"x"}')).toBe("abc");
    expect(sessionIdOf('Started.\n{"status":"ok","session_id": "def"}')).toBe("def");
    expect(sessionIdOf("no thread here")).toBeUndefined();
    expect(sessionIdOf(undefined)).toBeUndefined();
  });
});
