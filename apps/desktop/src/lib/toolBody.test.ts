import { describe, expect, test } from "bun:test";
import { argRow, argRows, commandOf, toolBody } from "./toolBody";
import { stripAnsi } from "./toolOutput";

describe("tool body by kind", () => {
  test("a call with diffs keeps its diff view", () => {
    expect(toolBody({ diffs: [{}], input: { path: "a" } }, "edit")).toEqual({ kind: "diff" });
  });

  test("a command shows its command line, output in plain text and in colour, and exit code", () => {
    const body = toolBody(
      {
        diffs: [],
        input: { command: "bun test", cwd: "/repo", description: "Run tests", summary: "tests" },
        output: "\u001b[31mfail\u001b[0m 1\n",
        exitCode: 1,
      },
      "execute",
    );
    expect(body).toEqual({
      kind: "command",
      command: "bun test",
      output: "fail 1",
      styled: [
        [
          { text: "fail", style: { color: "danger" } },
          { text: " 1", style: {} },
        ],
      ],
      exitCode: 1,
      args: [{ key: "cwd", value: "/repo", shape: "inline" }],
    });
  });

  test("a command falls back to the row's detail", () => {
    const body = toolBody({ diffs: [], detail: "ls -la" }, "execute");
    expect(body).toMatchObject({ kind: "command", command: "ls -la" });
  });

  test("argv joins as a shell reads it", () => {
    expect(commandOf({ command: ["/bin/zsh", "-lc", "echo it's"] })).toBe(`/bin/zsh -lc 'echo it'\\''s'`);
    expect(commandOf({ cmd: "ls" })).toBe("ls");
    expect(commandOf({ other: 1 })).toBeUndefined();
  });

  test("a search shows its pattern and results; a read its path and content", () => {
    const search = toolBody(
      { diffs: [], input: { pattern: "foo", path: "src" }, output: "a.ts:1" },
      "search",
    );
    expect(search).toEqual({
      kind: "lookup",
      args: [
        { key: "pattern", value: "foo", shape: "inline" },
        { key: "path", value: "src", shape: "inline" },
      ],
      output: "a.ts:1",
      outputLabel: "Results",
    });
    expect(toolBody({ diffs: [], input: { file_path: "a.ts" }, output: "x" }, "read")).toMatchObject({
      outputLabel: "Content",
    });
  });

  test("any other call shows its arguments as rows", () => {
    const body = toolBody(
      { diffs: [], input: { query: "q", limit: 5 }, output: "<tool_use_error>no</tool_use_error>" },
      "other",
    );
    expect(body.kind).toBe("args");
  });

  test("normalizes Antigravity CommandLine and PascalCase read arguments", () => {
    const cmd = toolBody(
      {
        diffs: [],
        input: {
          CommandLine: "git status",
          WaitMsBeforeAsync: 5000,
          toolSummary: "Check git",
          IsDaemon: false,
        },
        output: "clean",
        exitCode: 0,
      },
      "execute",
    );
    expect(cmd).toEqual({
      kind: "command",
      command: "git status",
      output: "clean",
      exitCode: 0,
      args: [],
    });
    const read = toolBody(
      {
        diffs: [],
        input: { Path: "/home/lucas/code/rustcode/AGENTS.md", StartLine: 1, EndLine: 80 },
        output: "# AGENTS",
      },
      "read",
    );
    expect(read).toEqual({
      kind: "lookup",
      args: [
        { key: "path", value: "~/code/rustcode/AGENTS.md", shape: "inline" },
        { key: "lines", value: "1–80", shape: "inline" },
      ],
      output: "# AGENTS",
      outputLabel: "Content",
    });
  });
});

describe("argument rows", () => {
  test("scalars sit inline, long or multi-line strings wrap, objects are JSON for that value only", () => {
    expect(argRow("n", 3)).toEqual({ key: "n", value: "3", shape: "inline" });
    expect(argRow("ok", true)).toEqual({ key: "ok", value: "true", shape: "inline" });
    expect(argRow("none", null)).toEqual({ key: "none", value: "null", shape: "inline" });
    expect(argRow("body", "x".repeat(81)).shape).toBe("text");
    expect(argRow("body", "a\nb").shape).toBe("text");
    expect(argRow("opts", { a: 1 })).toEqual({ key: "opts", value: '{\n  "a": 1\n}', shape: "json" });
    expect(argRow("list", [1, 2]).shape).toBe("json");
  });

  test("the summary is left out, and a bare value is one row", () => {
    expect(argRows({ summary: "s", a: "b" })).toEqual([{ key: "a", value: "b", shape: "inline" }]);
    expect(argRows("raw")).toEqual([{ key: "input", value: "raw", shape: "inline" }]);
    expect(argRows(undefined)).toEqual([]);
    expect(argRows(null)).toEqual([]);
  });
});

describe("stripAnsi", () => {
  test("drops colour, cursor and title sequences", () => {
    expect(stripAnsi("\u001b[1;32mok\u001b[0m")).toBe("ok");
    expect(stripAnsi("\u001b]0;title\u0007done")).toBe("done");
    expect(stripAnsi("\u001b[2Kline")).toBe("line");
    expect(stripAnsi("plain")).toBe("plain");
  });
});
