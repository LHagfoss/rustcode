import { describe, expect, test } from "bun:test";
import {
  buildDebugReport,
  type DebugReportInput,
  filterLog,
  formatDuration,
  logLevel,
  parseOperationalEvents,
  redact,
  reportFileName,
  serverState,
} from "./diagnostics";

describe("redact", () => {
  test("API keys and tokens of known shapes", () => {
    const text = [
      "openai sk-proj-abcdefghijklmnopqrstuvwx",
      "anthropic sk-ant-api03-ABCDEFGHIJKLMNOP_qrstu",
      "gh ghp_0123456789abcdefghijABCDEFGHIJ",
      "pat github_pat_11ABCDEFG0123456789_abcdefghij",
      "slack xoxb-1234567890-abcdefghij",
      "aws AKIAIOSFODNN7EXAMPLE",
      "jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
    ].join("\n");
    const out = redact(text);
    expect(out).not.toMatch(/sk-proj|sk-ant|ghp_|github_pat_|xoxb-|AKIA|eyJhbGci/);
    expect(out.match(/\[redacted\]/g)?.length).toBe(7);
  });

  test("values after a key that names a secret", () => {
    expect(redact("api_key=abc123def")).toBe("api_key=[redacted]");
    expect(redact('{"access_token": "abc.def", "model": "gpt"}')).toBe(
      '{"access_token": "[redacted]", "model": "gpt"}',
    );
    expect(redact("password: hunter2;")).toBe("password: [redacted];");
    expect(redact("Authorization: Bearer abcdefghijkl")).toBe("Authorization: Bearer [redacted]");
    expect(redact("x-api-key=zzzzzz&next=1")).toBe("x-api-key=[redacted]&next=1");
  });

  test("token counts stay readable", () => {
    const line = "turn.summary input_tokens=1200 output_tokens=40 max_tokens: 4096";
    expect(redact(line)).toBe(line);
  });

  test("email addresses", () => {
    expect(redact("signed in as lars.h@example.co.uk (pro)")).toBe("signed in as [email] (pro)");
    // A package scope or a version is not an address.
    expect(redact("@types/node 1.2.3 pkg@1.2.3")).toBe("@types/node 1.2.3 pkg@1.2.3");
  });

  test("the home folder becomes ~, on every platform's spelling", () => {
    expect(redact("/Users/lars/code/app and /Users/larsen/x", "/Users/lars")).toBe("~/code/app and ~/x");
    expect(redact("/home/lars/.config/rustcode", "/home/lars/")).toBe("~/.config/rustcode");
    const win = String.raw`C:\Users\lars`;
    expect(redact(String.raw`started C:\Users\lars\AppData\rustcode.exe`, win)).toBe(
      String.raw`started ~\AppData\rustcode.exe`,
    );
    // JSON-escaped and forward-slash spellings of the same folder.
    expect(redact(String.raw`{"cwd":"C:\\Users\\lars\\code"}`, win)).toBe(String.raw`{"cwd":"~\\code"}`);
    expect(redact("C:/Users/lars/code", win)).toBe("~/code");
  });

  test("another home folder's shape is hidden even when the home is unknown", () => {
    expect(redact("/Users/someone/x /home/ci/y")).toBe("~/x ~/y");
    expect(redact(String.raw`D:\Users\ci\z`)).toBe(String.raw`~\z`);
  });
});

describe("log levels", () => {
  test("reads tracing words, keyed levels and brackets", () => {
    expect(logLevel("2026-11-08T10:00:00Z  WARN rustcode::acp: slow")).toBe("warn");
    expect(logLevel('{"level":"ERROR","msg":"x"}')).toBe("error");
    expect(logLevel("level=debug msg=hi")).toBe("debug");
    expect(logLevel("[INFO] ready")).toBe("info");
    expect(logLevel("TRACE frame")).toBe("trace");
    expect(logLevel("thread 'main' panicked at src/main.rs")).toBe("error");
    expect(logLevel("desktop: started rustcode --acp (pid 1)")).toBe("info");
  });

  test("a filter keeps its level and everything more severe, matching the search", () => {
    const lines = ["ERROR a", "WARN b", "INFO c", "DEBUG d", "TRACE alpha"];
    expect(filterLog(lines, "warn", "")).toEqual(["ERROR a", "WARN b"]);
    expect(filterLog(lines, "all", "ALPHA")).toEqual(["TRACE alpha"]);
    expect(filterLog(lines, "error", "b")).toEqual([]);
  });
});

describe("server state", () => {
  const exit = (expected: boolean) => ({ code: 1, signal: null, atMs: 1, reason: "x", expected });
  test("a process that ended without being asked to crashed", () => {
    expect(serverState("closed", { running: false, crashRestarts: 1, lastExit: exit(false) })).toBe(
      "crashed",
    );
    expect(serverState("closed", { running: false, crashRestarts: 0, lastExit: exit(true) })).toBe("stopped");
    expect(serverState("connecting", { running: true, crashRestarts: 0 })).toBe("starting");
    expect(serverState("ready", { running: true, crashRestarts: 0 })).toBe("running");
  });

  test("without the shell, the connection decides", () => {
    expect(serverState("ready")).toBe("running");
    expect(serverState("closed", null)).toBe("stopped");
  });
});

test("durations read as the two largest units", () => {
  expect(formatDuration(12_000)).toBe("12s");
  expect(formatDuration(252_000)).toBe("4m 12s");
  expect(formatDuration(3 * 3600_000 + 4 * 60_000)).toBe("3h 4m");
  expect(formatDuration(26 * 3600_000)).toBe("1d 2h");
  expect(formatDuration(-5)).toBe("0s");
});

describe("debug report", () => {
  const at = new Date("2026-11-08T12:00:00Z");
  const base: DebugReportInput = {
    generatedAt: at,
    state: "crashed",
    stateDetail: "The server exited (code 101).",
    transport: "tauri",
    server: {
      running: false,
      crashRestarts: 2,
      path: "/Users/lars/code/rustcode/target/debug/rustcode",
      version: "1.21.0",
      configDir: "/Users/lars/.config/rustcode",
      lastExit: {
        code: 101,
        signal: null,
        atMs: at.getTime() - 5000,
        reason: "The server exited (code 101).",
        expected: false,
      },
    },
    system: {
      os: "linux",
      osVersion: "Ubuntu 24.04.1 LTS",
      kernel: "6.8.0",
      arch: "x86_64",
      appVersion: "1.21.0",
      tauriVersion: "2.12.5",
      webview: "WebKitGTK",
      webviewVersion: "2.46.1",
      home: "/Users/lars",
      graphicsEnv: ["XDG_SESSION_TYPE=wayland"],
    },
    page: {
      devicePixelRatio: 2,
      platform: "linux",
      gpu: "Mesa Intel(R) Xe",
      viewport: "1512×945",
      userAgent: "UA",
    },
    harnesses: [
      {
        id: "codex",
        name: "Codex",
        enabled: true,
        installed: true,
        command: ["codex", "app-server"],
        path: "/Users/lars/.local/bin/codex",
        version: "0.50.0",
        auth: "needs_login",
        error: "token expired | sign in again",
      },
    ],
    log: Array.from({ length: 250 }, (_, i) => `INFO line ${i}`).concat([
      "ERROR auth failed for lars@example.com with sk-ant-api03-ABCDEFGHIJKLMNOPQRST",
    ]),
    events: [
      { at: "2026-11-08T11:59:00Z", eventType: "turn.summary", threadId: "t1", fields: '{"rounds":3}' },
    ],
  };

  test("carries every section, the last 200 log lines and the events", () => {
    const report = buildDebugReport(base);
    expect(report).toContain("## RustCode debug report");
    expect(report).toContain("| State | Crashed (The server exited (code 101).) |");
    expect(report).toContain("| Restarts after a crash | 2 |");
    expect(report).toContain(
      "| Last exit | code 101, 2026-11-08T11:59:55.000Z: The server exited (code 101). |",
    );
    expect(report).toContain("| OS | linux Ubuntu 24.04.1 LTS kernel 6.8.0 |");
    expect(report).toContain("| Web view | WebKitGTK 2.46.1 |");
    expect(report).toContain("| Display scale | 2x |");
    expect(report).toContain("<summary>Server log (last 200 lines)</summary>");
    expect(report).not.toContain("INFO line 50\n");
    expect(report).toContain("INFO line 51\n");
    expect(report).toContain('2026-11-08T11:59:00Z  turn.summary  t1  {"rounds":3}');
    // A pipe in a cell does not split the table.
    expect(report).toContain("token expired \\| sign in again");
  });

  test("is redacted as a whole", () => {
    const report = buildDebugReport(base);
    expect(report).not.toContain("/Users/lars");
    expect(report).not.toContain("lars@example.com");
    expect(report).not.toContain("sk-ant");
    expect(report).toContain("| Binary | ~/code/rustcode/target/debug/rustcode |");
    expect(report).toContain("ERROR auth failed for [email] with [redacted]");
  });

  test("a log with backticks gets a longer fence", () => {
    const report = buildDebugReport({ ...base, log: ["```", "x"] });
    expect(report).toContain("````text\n```\nx\n````");
  });

  test("says what is missing instead of leaving gaps", () => {
    const report = buildDebugReport({
      generatedAt: at,
      state: "stopped",
      harnesses: [],
      log: [],
      events: [],
      eventsError: "This server has no data store.",
    });
    expect(report).toContain("| Version | — |");
    expect(report).toContain("No harnesses listed.");
    expect(report).toContain("Nothing logged.");
    expect(report).toContain("Could not read them: This server has no data store.");
  });
});

test("operational events are read from query rows", () => {
  expect(
    parseOperationalEvents({
      columns: ["at", "event_type", "thread_id", "fields_json"],
      rows: [{ at: "a", event_type: "turn.start", thread_id: "t", fields_json: "{}" }, { at: "b" }],
    }),
  ).toEqual([{ at: "a", eventType: "turn.start", threadId: "t", fields: "{}" }]);
  expect(parseOperationalEvents(null)).toEqual([]);
});

test("file names carry the local time", () => {
  const at = new Date(2026, 10, 8, 9, 5);
  expect(reportFileName(at)).toBe("rustcode-debug-2026-11-08-0905.md");
  expect(reportFileName(at, "server-log")).toBe("rustcode-server-log-2026-11-08-0905.log");
});
