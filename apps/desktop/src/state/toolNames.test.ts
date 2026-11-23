import { describe, expect, test } from "bun:test";
import type { Json } from "../acp/jsonrpc";
import {
  displayTool,
  failureReason,
  humanizeTool,
  keptTitle,
  orchestrationTool,
  printedExitCode,
  sessionIdOf,
  splitToolName,
  unwrapCallTool,
  unwrapToolError,
  usefulSummary,
} from "./toolNames";

describe("tool names", () => {
  test("an `mcp__server__tool` name always splits into its server and tool", () => {
    expect(splitToolName("mcp__rustcode__start_session")).toEqual({
      server: "rustcode",
      tool: "start_session",
    });
    expect(splitToolName("mcp__mail__search_messages")).toEqual({ server: "mail", tool: "search_messages" });
    expect(splitToolName("Run mcp_rustcode_list_threads")).toEqual({
      server: "rustcode",
      tool: "list_threads",
    });
    expect(splitToolName("Run mcp_socraticode_codebase_search")).toEqual({
      server: "socraticode",
      tool: "codebase_search",
    });
  });

  test("a dotted name splits only for a server we know", () => {
    expect(splitToolName("rustcode.start_session")).toEqual({ server: "rustcode", tool: "start_session" });
    expect(splitToolName("rustcode/start_session")).toEqual({ server: "rustcode", tool: "start_session" });
    expect(splitToolName("rustcode:start_session")).toEqual({ server: "rustcode", tool: "start_session" });
    expect(splitToolName("rustcode: compact_thread")).toEqual({ server: "rustcode", tool: "compact_thread" });
    expect(displayTool("rustcode: compact_thread").title).toBe("Requested compaction");
    expect(splitToolName("rustcode__start_session")).toEqual({ server: "rustcode", tool: "start_session" });
    // Titles that happen to hold a separator are not MCP calls.
    expect(splitToolName("bin/run_tests")).toEqual({ tool: "bin/run_tests" });
    expect(splitToolName("foo.barBaz")).toEqual({ tool: "foo.barBaz" });
    expect(splitToolName("mail.search_messages")).toEqual({ tool: "mail.search_messages" });
    expect(splitToolName("mail__search_messages")).toEqual({ tool: "mail__search_messages" });
  });

  test("a plain title stays whole, and so does a file name", () => {
    expect(splitToolName("bash")).toEqual({ tool: "bash" });
    expect(splitToolName("Bash")).toEqual({ tool: "Bash" });
    expect(splitToolName("Read")).toEqual({ tool: "Read" });
    expect(splitToolName("package.json")).toEqual({ tool: "package.json" });
    expect(splitToolName("mcp__rustcode__wait_session")).toEqual({
      server: "rustcode",
      tool: "wait_session",
    });
  });

  test("an orchestration tool is RustCode's own, or a bare name", () => {
    expect(orchestrationTool("start_session")).toBe("start_session");
    expect(orchestrationTool("rustcode.start_session")).toBe("start_session");
    expect(orchestrationTool("mcp__rustcode__wait_session")).toBe("wait_session");
    expect(orchestrationTool("mcp__rustcode__wait_thread")).toBe("wait_session");
    expect(orchestrationTool("mcp__other__wait_thread")).toBeUndefined();
    // Another server's tool of the same name is not ours.
    expect(orchestrationTool("mcp__github__list_sessions")).toBeUndefined();
    expect(orchestrationTool("github.start_session")).toBeUndefined();
    expect(orchestrationTool("bash")).toBeUndefined();
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
    expect(displayTool("bin/run_tests")).toEqual({ title: "bin/run_tests", summary: undefined });
  });

  test("another server's `start_session` is that server's tool", () => {
    expect(displayTool("mcp__github__start_session")).toMatchObject({
      title: "Start session",
      source: "github",
    });
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
    // The named login the call asked for reads beside the harness and model.
    expect(
      displayTool("mcp__rustcode__start_session", {
        title: "Polish the composer",
        prompt: "…",
        harness: "claude-code",
        account: "work",
        model: "opus",
      }).summary,
    ).toBe("Polish the composer · claude-code work opus");
    expect(displayTool("mcp__rustcode__message_session").title).toBe("Messaged thread");
    expect(displayTool("mcp__rustcode__read_session").title).toBe("Read thread");
    expect(displayTool("mcp__rustcode__list_sessions").title).toBe("Listed threads");
    expect(
      displayTool("mcp__rustcode__fan_out", { harnesses: [{ harness: "a" }, { harness: "b" }] }).title,
    ).toBe("Fanned out to 2 harnesses");
  });

  test("a summary cut at an emoji drops it whole rather than ending in half of one", () => {
    const summary = displayTool("mcp__rustcode__start_session", {
      prompt: `${"a".repeat(58)}👍🏽 and more`,
    }).summary;
    expect(summary).toBe(`${"a".repeat(58)}…`);
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
    expect(sessionIdOf('{"thread_id":"abc","title":"x"}')).toBe("abc");
    expect(sessionIdOf('{"session_id":"abc","title":"x"}')).toBe("abc");
    expect(sessionIdOf('Started.\n{"status":"ok","session_id": "def"}')).toBe("def");
    expect(sessionIdOf("no thread here")).toBeUndefined();
    expect(sessionIdOf(undefined)).toBeUndefined();
  });
});

describe("why a call failed (issue 3711)", () => {
  test("the harness's error wrapper is taken off", () => {
    expect(unwrapToolError("<tool_use_error>boom</tool_use_error>")).toBe("boom");
    expect(failureReason("<tool_use_error>the user said no</tool_use_error>")).toBe("the user said no");
  });

  test("a wrapper with no reason after it says so instead of leaving the row empty", () => {
    const empty = "<tool_use_error>Error calling tool (mcp__rustcode__start_session): </tool_use_error>";
    expect(failureReason(empty)).toBe("The tool gave no reason");
    expect(failureReason(empty)).not.toContain("tool_use_error");
    expect(failureReason(undefined)).toBe("");
  });

  test("an exit code printed as the first line reads as the code", () => {
    expect(printedExitCode("Exit code 1\nfatal: not a git repository")).toBe(1);
    expect(printedExitCode("<tool_use_error>Exit code 128\nboom</tool_use_error>")).toBe(128);
    expect(printedExitCode("fatal: Exit code 1")).toBeUndefined();
    expect(printedExitCode(undefined)).toBeUndefined();
  });
});

describe("unwrapCallTool", () => {
  const call = (title: string, input: Json) => unwrapCallTool({ title, input });

  test("a deferred call reads as the tool it ran", () => {
    expect(
      call("mcp__rustcode__call_tool", { name: "start_thread", arguments: { prompt: "fix the login" } }),
    ).toEqual({ title: "mcp__rustcode__start_thread", input: { prompt: "fix the login" } });
    expect(call("rustcode.call_tool", { name: "list_jobs" })).toEqual({
      title: "mcp__rustcode__list_jobs",
      input: {},
    });
    expect(
      orchestrationTool(call("mcp__rustcode__call_tool", { name: "wait_thread", arguments: {} }).title),
    ).toBe("wait_session");
  });

  test("another server's call_tool and an input it does not recognise stay as they came", () => {
    const other = { title: "mcp__mail__call_tool", input: { name: "send" } };
    expect(unwrapCallTool(other)).toBe(other);
    const odd = { title: "mcp__rustcode__call_tool", input: { name: "x", extra: 1 } };
    expect(unwrapCallTool(odd)).toBe(odd);
  });

  test("a later update without input keeps the unwrapped title", () => {
    expect(keptTitle("mcp__rustcode__call_tool", undefined, "mcp__rustcode__start_thread")).toBe(
      "mcp__rustcode__start_thread",
    );
    expect(keptTitle(undefined, undefined, "Bash")).toBe("Bash");
    expect(keptTitle("Read", undefined, "Bash")).toBe("Read");
  });
});
