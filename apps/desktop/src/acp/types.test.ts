import { describe, expect, test } from "bun:test";
import type { Json } from "./jsonrpc";
import {
  delegateParent,
  displayPath,
  fallbackHarnessSnapshot,
  initializeParams,
  parseAgentInfo,
  parseHarnessSnapshot,
  parsePermissionRequest,
  parseRemoteStatus,
  parseSessionList,
  parseSessionUpdate,
  projectName,
  questionOptionsAnswer,
  questionTextAnswer,
} from "./types";

describe("ACP types", () => {
  test("reads agent capabilities like the engine sends them", () => {
    const info = parseAgentInfo({
      protocolVersion: 1,
      agentCapabilities: {
        loadSession: true,
        promptCapabilities: { image: true },
        sessionCapabilities: { list: {}, close: {} },
        _meta: { rustcode: { steer: 1, agents: 2, rewind: 1, pullRequests: 3 } },
      },
    });
    expect(info).toMatchObject({
      loadSession: true,
      listSessions: true,
      steer: true,
      plan: true,
      promptImages: true,
    });
  });

  test("rejects another protocol version", () => {
    expect(() => parseAgentInfo({ protocolVersion: 2 })).toThrow(/ACP version 2/);
  });

  test("advertises no client fs or terminal", () => {
    const params = initializeParams("0.1.0") as { clientCapabilities: { fs: unknown; terminal: boolean } };
    expect(params.clientCapabilities.terminal).toBe(false);
    expect(params.clientCapabilities.fs).toEqual({ readTextFile: false, writeTextFile: false });
  });

  test("asks for Discord presence only when the client wants it", () => {
    const meta = (options: Parameters<typeof initializeParams>[1]) =>
      (initializeParams("0.1.0", options) as { clientCapabilities: { _meta: { rustcode: Json } } })
        .clientCapabilities._meta.rustcode;
    expect(meta({ presence: 1 })).toMatchObject({ presence: 1 });
    expect(meta({})).not.toHaveProperty("presence");
    expect(meta({ presence: 0 })).not.toHaveProperty("presence");
  });

  test("says it lists the threads the engine starts for a paired device", () => {
    const meta = (options: Parameters<typeof initializeParams>[1]) =>
      (initializeParams("0.1.0", options) as { clientCapabilities: { _meta: { rustcode: Json } } })
        .clientCapabilities._meta.rustcode;
    expect(meta({ threads: 1 })).toMatchObject({ threads: 1 });
    expect(meta({})).not.toHaveProperty("threads");
  });

  test("parses session/list with model and harness", () => {
    const { sessions } = parseSessionList({
      sessions: [
        {
          sessionId: "a",
          cwd: "/x",
          title: "T",
          updatedAt: "2026-11-02T10:23:56.100299+00:00",
          _meta: {
            model: "chatgpt/gpt-6-luna",
            rustcode: {
              harness: "codex",
              parent: { sessionId: "p", title: "Parent" },
            },
          },
        },
        {
          sessionId: "old",
          cwd: "/x",
          title: "Older",
          _meta: { rustcode: { parent: { title: "A parent that is gone" } } },
        },
        { cwd: "/no-id" },
      ],
    });
    expect(sessions).toHaveLength(2);
    expect(sessions[0]).toMatchObject({
      id: "a",
      model: "chatgpt/gpt-6-luna",
      harness: "codex",
      parent: { id: "p", title: "Parent", relationship: "delegate" },
    });
    expect(sessions[1]!.parent).toEqual({
      id: undefined,
      title: "A parent that is gone",
      relationship: "delegate",
    });
    expect(sessions[0]!.updatedAt).toBeGreaterThan(0);
  });

  test("reads how a thread relates to its parent, and a record without one is a delegate", () => {
    const listed = (relationship?: string) =>
      parseSessionList({
        sessions: [
          {
            sessionId: "c",
            cwd: "/x",
            _meta: {
              rustcode: {
                parent: { sessionId: "p", title: "Parent", ...(relationship ? { relationship } : {}) },
              },
            },
          },
        ],
      }).sessions[0]!.parent;
    expect(listed("fork")?.relationship).toBe("fork");
    expect(listed("side_chat")?.relationship).toBe("side_chat");
    expect(listed("delegate")?.relationship).toBe("delegate");
    expect(listed(undefined)?.relationship).toBe("delegate");
    // A name a later engine wrote is neither a delegate nor a branch.
    expect(listed("mirror")?.relationship).toBe("other");
    expect(delegateParent(listed("fork"))).toBeUndefined();
    expect(delegateParent(listed(undefined))?.id).toBe("p");
    expect(delegateParent({ id: "p" })?.id).toBe("p");
  });

  test("parses tool call updates with output delta, diff and exit code", () => {
    const update = parseSessionUpdate({
      sessionId: "s",
      update: {
        sessionUpdate: "tool_call_update",
        toolCallId: "c",
        status: "completed",
        content: [
          {
            type: "diff",
            path: "a.ts",
            oldText: "a",
            newText: "b",
            _meta: { rustcode: { oldStart: 4, newStart: 5 } },
          },
        ],
        rawOutput: { content: "ok", exitCode: 0 },
        _meta: { rustcode: { outputDelta: "x" } },
      },
    });
    expect(update).toMatchObject({ kind: "toolCallUpdate", output: "ok", outputDelta: "x", exitCode: 0 });
    if (update.kind === "toolCallUpdate")
      expect(update.diffs[0]).toMatchObject({ path: "a.ts", oldStart: 4, newStart: 5 });
  });

  test("a result the harness fenced is the output, without the fence", () => {
    const update = parseSessionUpdate({
      sessionId: "s",
      update: {
        sessionUpdate: "tool_call_update",
        toolCallId: "c",
        status: "completed",
        content: [
          {
            type: "content",
            content: {
              type: "text",
              text: "```console\nCommand running in background with ID: bc82qcgu4.\nYou will be notified when it completes.\n```",
            },
          },
        ],
      },
    });
    expect(update).toMatchObject({
      kind: "toolCallUpdate",
      status: "completed",
      output: "Command running in background with ID: bc82qcgu4.\nYou will be notified when it completes.",
    });
  });

  test("an update that repeats the call's input has no output", () => {
    // What Claude Code's adapter sends once it has the arguments: the call again, with the
    // description as content. Read as output, the description looked like what the command printed.
    const update = parseSessionUpdate({
      sessionId: "s",
      update: {
        sessionUpdate: "tool_call_update",
        toolCallId: "c",
        rawInput: { command: "bun test src", description: "Run the unit tests" },
        content: [{ type: "content", content: { type: "text", text: "Run the unit tests" } }],
      },
    });
    expect(update).toMatchObject({ kind: "toolCallUpdate", output: undefined });
  });

  test("a call restated alongside its input is no output, whatever status it carries", () => {
    const update = parseSessionUpdate({
      sessionId: "s",
      update: {
        sessionUpdate: "tool_call_update",
        toolCallId: "c",
        status: "in_progress",
        rawInput: { command: "bun test src", description: "Run the unit tests" },
        content: [{ type: "content", content: { type: "text", text: "Run the unit tests" } }],
      },
    });
    expect(update).toMatchObject({ kind: "toolCallUpdate", output: undefined });
  });

  test("a replayed call keeps what it printed next to the arguments it was given", () => {
    // The engine's replay of a resumed transcript sends a finished call, its arguments and its
    // recorded result as one update (`harness/transcript.rs`): the input is no proof of an echo.
    const update = parseSessionUpdate({
      sessionId: "s",
      update: {
        sessionUpdate: "tool_call",
        toolCallId: "c",
        title: "ls",
        status: "completed",
        rawInput: { command: "ls" },
        content: [{ type: "content", content: { type: "text", text: "a.ts\nb.ts\n" } }],
      },
    });
    expect(update).toMatchObject({ kind: "toolCall", output: "a.ts\nb.ts\n" });
  });

  test("a result that arrives with the call's input is still the output", () => {
    const update = parseSessionUpdate({
      sessionId: "s",
      update: {
        sessionUpdate: "tool_call_update",
        toolCallId: "c",
        status: "completed",
        rawInput: { command: "bun test src", description: "Run the unit tests" },
        content: [{ type: "content", content: { type: "text", text: "24 pass\n" } }],
      },
    });
    expect(update).toMatchObject({ kind: "toolCallUpdate", output: "24 pass\n" });
  });

  test("an update that carries the call's input can still answer with a result", () => {
    const update = parseSessionUpdate({
      sessionId: "s",
      update: {
        sessionUpdate: "tool_call_update",
        toolCallId: "c",
        status: "completed",
        rawInput: { path: "a.ts" },
        rawOutput: { content: "```\nhello\n```" },
      },
    });
    expect(update).toMatchObject({ kind: "toolCallUpdate", output: "hello" });
  });

  test("a call whose arguments are empty still shows what it printed", () => {
    const update = parseSessionUpdate({
      sessionId: "s",
      update: {
        sessionUpdate: "tool_call",
        toolCallId: "c",
        title: "read",
        status: "completed",
        rawInput: null,
        content: [{ type: "content", content: { type: "text", text: "use std::net;" } }],
      },
    });
    expect(update).toMatchObject({ kind: "toolCall", output: "use std::net;" });
  });

  test("a backgrounded call's marker reaches the app", () => {
    const update = parseSessionUpdate({
      sessionId: "s",
      update: {
        sessionUpdate: "tool_call_update",
        toolCallId: "c",
        status: "completed",
        _meta: { jetbrains: { air: { asyncTasks: { backgrounded: true } } } },
      },
    });
    expect(update).toMatchObject({ kind: "toolCallUpdate", backgrounded: true });
  });

  test("a resumable notice is not reply text", () => {
    const update = parseSessionUpdate({
      sessionId: "s",
      update: {
        sessionUpdate: "agent_message_chunk",
        content: { type: "text", text: "Stopped" },
        _meta: { rustcode: { kind: "notice", resumable: true } },
      },
    });
    expect(update).toEqual({ kind: "notice", text: "Stopped", resumable: true });
  });

  test("question requests and their answers", () => {
    const request = parsePermissionRequest(3, {
      sessionId: "s",
      toolCall: {
        toolCallId: "q",
        title: "Q",
        rawInput: { header: "H", question: "Which?", descriptions: ["a"], multiple: true },
      },
      options: [
        { optionId: "question_option_0", name: "A", kind: "allow_once" },
        { optionId: "question_option_1", name: "B", kind: "allow_once" },
        { optionId: "question_cancel", name: "Cancel", kind: "reject_once" },
      ],
      _meta: { rustcode: { kind: "question", version: 2 } },
    });
    expect(request?.question).toEqual({ header: "H", text: "Which?", descriptions: ["a"], multiple: true });
    const [a, b] = request!.options;
    expect(questionOptionsAnswer([a!, b!])).toEqual({
      outcome: { outcome: "selected", optionId: "question_option_0" },
      _meta: { rustcode: { answer: { options: ["A", "B"] } } },
    });
    expect(questionTextAnswer("free", a!)._meta).toEqual({ rustcode: { answer: { text: "free" } } });
  });

  test("an unknown question version is a plain approval", () => {
    const request = parsePermissionRequest(1, {
      sessionId: "s",
      toolCall: { toolCallId: "q" },
      options: [],
      _meta: { rustcode: { kind: "question", version: 99 } },
    });
    expect(request?.question).toBeUndefined();
  });

  test("parses a harness snapshot and falls back without one", () => {
    const snapshot = parseHarnessSnapshot({
      harnesses: [
        {
          id: "codex",
          name: "Codex",
          kind: "codex",
          installed: true,
          enabled: true,
          auth: "ok",
          capabilities: { steer: "next_turn", effort: true },
          models: [{ id: "m", efforts: ["low"] }],
        },
      ],
      default: { harness: "codex", model: "m", effort: "low" },
      onboarded: false,
    });
    expect(snapshot.harnesses[0]).toMatchObject({ id: "codex", kind: "codex", auth: "ok" });
    expect(snapshot.harnesses[0]!.capabilities).toMatchObject({
      steer: "next_turn",
      effort: true,
      cancel: false,
    });
    expect(snapshot.default).toEqual({ harness: "codex", model: "m", effort: "low" });
    expect(fallbackHarnessSnapshot()).toMatchObject({ onboarded: true, unsupported: true });
  });

  test("the engine says which harnesses are standard", () => {
    const row = (id: string, known?: boolean) => ({
      id,
      name: id,
      kind: "acp",
      installed: true,
      enabled: false,
      auth: "unknown",
      ...(known === undefined ? {} : { known }),
    });
    const snapshot = parseHarnessSnapshot({
      harnesses: [
        row("rustcode", true),
        row("pi", true),
        row("my-agent", false),
        // An engine that predates the field: the standard ids it shipped stand in.
        row("claude-code"),
        row("someone-elses-agent"),
      ],
    });
    const known = snapshot.harnesses.map((h) => [h.id, h.known]);
    expect(known).toEqual([
      ["rustcode", true],
      ["pi", true],
      ["my-agent", false],
      ["claude-code", true],
      ["someone-elses-agent", false],
    ]);
    expect(fallbackHarnessSnapshot().harnesses[0]!.known).toBe(true);
  });
});

describe("displayPath", () => {
  test("home as ~ and /private/tmp as /tmp", () => {
    expect(displayPath("/Users/me/code/app")).toBe("~/code/app");
    expect(displayPath("/private/tmp/app")).toBe("/tmp/app");
    expect(displayPath("/private/tmpx")).toBe("/private/tmpx");
  });

  test("a Windows extended-length path as the path it wraps", () => {
    expect(displayPath("\\\\?\\F:\\code\\app")).toBe("F:\\code\\app");
    expect(displayPath("\\\\?\\UNC\\server\\share\\app")).toBe("\\\\server\\share\\app");
    expect(displayPath("F:\\code\\app")).toBe("F:\\code\\app");
  });
});

describe("projectName", () => {
  test("the last folder of a POSIX path", () => {
    expect(projectName("/Users/me/code/rustcode")).toBe("rustcode");
    expect(projectName("/Users/me/code/rustcode/")).toBe("rustcode");
    expect(projectName("/private/tmp/rustcode-worktree")).toBe("rustcode-worktree");
  });

  test("the last folder of a Windows path, prefix and all", () => {
    const long = "\\\\?\\F:\\Users\\lagos\\code\\rustcode";
    expect(projectName(long)).toBe("rustcode");
    expect(projectName("C:\\Users\\lagos\\code\\rustcode")).toBe("rustcode");
    expect(projectName("\\\\?\\UNC\\server\\share\\rustcode")).toBe("rustcode");
    expect(projectName("C:\\Users\\lagos\\code\\rustcode\\")).toBe("rustcode");
    // A drive root has no folder of its own: it names itself.
    expect(projectName("\\\\?\\F:\\")).toBe("F:");
  });

  test("a path with no folder left says itself", () => {
    expect(projectName("")).toBe("");
    expect(projectName("/")).toBe("/");
  });
});

describe("remote access", () => {
  test("reads the machine's gateway, devices and pairing challenge", () => {
    const status = parseRemoteStatus({
      available: true,
      enabled: true,
      shared: false,
      gateway: {
        connected: true,
        advertisedAddress: "192.168.31.185:17879",
        loopbackOnly: false,
        attachedDevices: [],
        connectedDevices: ["Lars's iPhone"],
        pairedDevices: 1,
        pairing: { expiresInSecs: 87, attemptsLeft: 4 },
      },
      offer: {
        qrPayload: '{"protocol_version":1}',
        qrRows: ["0110", "1001"],
        code: "4821-7730",
        address: "192.168.31.185:17879",
        expiresInSecs: 87,
        loopbackOnly: false,
        challenge: 7,
      },
      pairedDevices: 1,
      devices: [{ id: "device-1", name: "Lars's iPhone" }],
    });
    expect(status.enabled).toBe(true);
    expect(status.shared).toBe(false);
    expect(status.gateway?.advertisedAddress).toBe("192.168.31.185:17879");
    expect(status.gateway?.pairing?.attemptsLeft).toBe(4);
    expect(status.offer?.qrRows).toEqual(["0110", "1001"]);
    expect(status.offer?.code).toBe("4821-7730");
    expect(status.devices).toEqual([
      { id: "device-1", name: "Lars's iPhone", kind: "phone", platform: "unknown" },
    ]);
  });

  test("a thread's share names the device attached to it", () => {
    const status = parseRemoteStatus({
      available: true,
      enabled: true,
      shared: true,
      sessionId: "session-1",
      target: "claude-code",
      gateway: {
        connected: true,
        attachedDevices: ["Lars's iPhone"],
        connectedDevices: ["Lars's iPhone"],
      },
      pairedDevices: 1,
      devices: [{ id: "device-1", name: "Lars's iPhone" }],
    });
    expect(status.shared).toBe(true);
    expect(status.target).toBe("claude-code");
    expect(status.gateway?.attachedDevices).toEqual(["Lars's iPhone"]);
  });

  test("an answer with nothing set reads as off, never as on", () => {
    const status = parseRemoteStatus({});
    expect(status.enabled).toBe(false);
    expect(status.shared).toBe(false);
    expect(status.pairedDevices).toBe(0);
    expect(status.devices).toEqual([]);
    expect(status.offer).toBeUndefined();
  });
});

describe("home path display across platforms", () => {
  test("shortens Windows and POSIX home folders on segment boundaries", () => {
    for (const [path, expected] of [
      ["C:\\Users\\ada\\code", "~\\code"],
      ["c:/Users/ada/code", "~/code"],
      ["C:\\Users\\ada", "~"],
      ["/home/ada/code", "~/code"],
      ["/Users/ada/code", "~/code"],
      ["\\\\?\\C:\\Users\\ada\\code", "~\\code"],
      ["\\\\server\\share\\code", "\\\\server\\share\\code"],
      ["/home", "/home"],
      ["C:\\Users", "C:\\Users"],
    ])
      expect(displayPath(path!)).toBe(expected!);
  });
});
