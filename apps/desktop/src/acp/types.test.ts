import { describe, expect, test } from "bun:test";
import {
  fallbackHarnessSnapshot,
  initializeParams,
  parseAgentInfo,
  parseHarnessSnapshot,
  parsePermissionRequest,
  parseSessionList,
  parseSessionUpdate,
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

  test("parses session/list with model and harness", () => {
    const { sessions } = parseSessionList({
      sessions: [
        {
          sessionId: "a",
          cwd: "/x",
          title: "T",
          updatedAt: "2026-11-02T10:23:56.100299+00:00",
          _meta: { model: "chatgpt/gpt-6-luna", rustcode: { harness: "codex" } },
        },
        { cwd: "/no-id" },
      ],
    });
    expect(sessions).toHaveLength(1);
    expect(sessions[0]).toMatchObject({ id: "a", model: "chatgpt/gpt-6-luna", harness: "codex" });
    expect(sessions[0]!.updatedAt).toBeGreaterThan(0);
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
});
