import { describe, expect, test } from "bun:test";
import { parseSessionUpdate, type SessionUpdate } from "../acp/types";
import {
  appendSteer,
  applyUpdate,
  attachAutoAllowed,
  beginTurn,
  checkCommands,
  diffCounts,
  diffLines,
  emptyThread,
  finishLoad,
  finishTurn,
  foldingBatch,
  formatClock,
  formatDuration,
  idleSettled,
  requeueSteers,
  shortAge,
  splitSubagentSteps,
  subagentOf,
  thoughtHeadings,
  wakeTurn,
  workingLabel,
} from "./thread";

const apply = (updates: SessionUpdate[], state = emptyThread("s", "/p")) =>
  updates.reduce((s, u) => applyUpdate(s, u, 1000), state);

describe("thread reducer", () => {
  test("streams reply chunks into one message and drops the leading blank lines", () => {
    const s = apply([
      { kind: "user", text: "hi" },
      { kind: "agent", text: "\n\nHel" },
      { kind: "agent", text: "lo" },
    ]);
    expect(s.items.map((i) => i.kind)).toEqual(["user", "assistant"]);
    expect(s.items[1]).toMatchObject({ text: "Hello" });
  });

  test("a thought closes when the reply starts", () => {
    const s = apply([
      { kind: "thought", text: "**A**", summary: true },
      { kind: "thought", text: " more", summary: true },
      { kind: "agent", text: "x" },
    ]);
    expect(s.items[0]).toMatchObject({ kind: "thought", text: "**A** more", done: true });
  });

  test("tool calls update in place, accumulate output deltas and keep diffs", () => {
    const s = apply([
      { kind: "toolCall", id: "c", title: "bash", diffs: [] },
      { kind: "toolCallUpdate", id: "c", outputDelta: "a\n", diffs: [] },
      { kind: "toolCallUpdate", id: "c", outputDelta: "b\n", diffs: [] },
      {
        kind: "toolCallUpdate",
        id: "c",
        status: "completed",
        diffs: [{ path: "f", newText: "x", oldStart: 1, newStart: 1 }],
        exitCode: 0,
      },
      { kind: "toolCallUpdate", id: "unknown", status: "failed", diffs: [] },
    ]);
    expect(s.items).toHaveLength(1);
    expect(s.items[0]).toMatchObject({ kind: "tool", status: "completed", output: "a\nb\n", exitCode: 0 });
  });

  test("an auto-allowed call says so on its own row, and later updates keep it", () => {
    const s = apply([
      { kind: "user", text: "go" },
      { kind: "toolCall", id: "c", title: "mcp__rustcode__start_session", diffs: [] },
      { kind: "toolCall", id: "other", title: "bash", diffs: [] },
    ]);
    const marked = attachAutoAllowed(s, "start_session", "Full access");
    expect(marked.attached).toBe(true);
    expect(marked.state.items[1]).toMatchObject({ id: "c", autoAllowed: "Full access" });
    expect(marked.state.items[2]).not.toHaveProperty("autoAllowed");
    const after = applyUpdate(
      marked.state,
      { kind: "toolCallUpdate", id: "c", status: "completed", diffs: [] },
      2000,
    );
    expect(after.items[1]).toMatchObject({ status: "completed", autoAllowed: "Full access" });
    const again = applyUpdate(
      marked.state,
      { kind: "toolCall", id: "c", title: "mcp__rustcode__start_session", status: "in_progress", diffs: [] },
      2000,
    );
    expect(again.items[1]).toMatchObject({ status: "in_progress", autoAllowed: "Full access" });
  });

  test("with no call in flight for that tool, there is nothing to mark", () => {
    const s = apply([
      { kind: "toolCall", id: "c", title: "mcp__rustcode__start_session", status: "completed", diffs: [] },
    ]);
    expect(attachAutoAllowed(s, "start_session", "Full access")).toEqual({ state: s, attached: false });
    expect(attachAutoAllowed(s, "message_session", "Full access").attached).toBe(false);
  });

  test("two calls in flight mark two rows, and only RustCode's own call is marked", () => {
    const s = apply([
      { kind: "user", text: "go" },
      { kind: "toolCall", id: "a", title: "mcp__rustcode__start_session", diffs: [] },
      { kind: "toolCall", id: "b", title: "mcp__rustcode__start_session", diffs: [] },
      { kind: "toolCall", id: "other", title: "mcp__github__start_session", diffs: [] },
    ]);
    const first = attachAutoAllowed(s, "start_session", "Full access");
    const second = attachAutoAllowed(first.state, "start_session", "Full access");
    expect([first.attached, second.attached]).toEqual([true, true]);
    const marked = second.state.items.flatMap((i) => (i.kind === "tool" ? [i.autoAllowed] : []));
    expect(marked).toEqual(["Full access", "Full access", undefined]);
    // Both rows are spoken for: a third call has no row of its own to mark.
    expect(attachAutoAllowed(second.state, "start_session", "Full access").attached).toBe(false);
  });

  test("replayed turn timing becomes a footer before the next user message and at the end of the load", () => {
    const s = finishLoad(
      apply([
        { kind: "user", text: "1" },
        { kind: "agent", text: "a", turn: { elapsedMs: 5000, outcome: "completed" } },
        { kind: "user", text: "2" },
        { kind: "agent", text: "b", turn: { elapsedMs: 7000, outcome: "completed" } },
      ]),
    );
    expect(s.items.map((i) => i.kind)).toEqual([
      "user",
      "assistant",
      "turnEnd",
      "user",
      "assistant",
      "turnEnd",
    ]);
    expect(s.items[5]).toMatchObject({ elapsedMs: 7000 });
    expect(s.loaded).toBe(true);
  });

  test("a turn ends: running tools settle, permissions clear, and the footer has the elapsed time", () => {
    let s = beginTurn(emptyThread("s", "/p"), "go", 0);
    s = applyUpdate(s, { kind: "toolCall", id: "c", title: "bash", status: "in_progress", diffs: [] }, 10);
    s = finishTurn(s, "cancelled", 4000);
    expect(s.running).toBe(false);
    expect(s.items[1]).toMatchObject({ status: "failed" });
    expect(s.items.at(-1)).toMatchObject({ kind: "turnEnd", outcome: "cancelled", elapsedMs: 4000 });
  });

  test("unused steers leave the transcript and go back to the front of the queue", () => {
    let s = beginTurn(emptyThread("s", "/p"), "go", 0);
    s = { ...s, queue: [{ id: "q", text: "later" }] };
    s = appendSteer(s, "also this");
    s = requeueSteers(s, ["also this"]);
    expect(s.items.some((i) => i.kind === "user" && i.text === "also this")).toBe(false);
    expect(s.queue.map((q) => q.text)).toEqual(["also this", "later"]);
  });

  test("plan, title, usage and config options", () => {
    const s = apply([
      { kind: "plan", entries: [{ content: "a", status: "completed" }] },
      { kind: "title", title: "T" },
      { kind: "usage", usage: { used: 1, size: 2, estimated: false } },
      {
        kind: "configOptions",
        options: [{ id: "model", name: "Model", type: "select", current: "m", choices: [] }],
      },
    ]);
    expect(s).toMatchObject({
      title: "T",
      plan: [{ content: "a" }],
      usage: { used: 1 },
      configOptions: [{ id: "model" }],
    });
  });
});

describe("helpers", () => {
  test("line diff keeps context and numbers lines", () => {
    const lines = diffLines({
      path: "f",
      oldText: "a\nb\nc",
      newText: "a\nB\nc\nd",
      oldStart: 10,
      newStart: 10,
    });
    expect(lines.map((l) => `${l.kind[0]}${l.text}`)).toEqual(["ca", "rb", "aB", "cc", "ad"]);
    expect(lines[4]).toMatchObject({ newNumber: 13 });
    expect(
      diffCounts([{ path: "f", oldText: "a\nb\nc", newText: "a\nB\nc\nd", oldStart: 1, newStart: 1 }]),
    ).toEqual({ added: 2, removed: 1 });
  });

  test("a created file is all additions", () => {
    expect(
      diffLines({ path: "f", newText: "x\ny", oldStart: 1, newStart: 1 }).every((l) => l.kind === "added"),
    ).toBe(true);
  });

  test("durations and thought headings", () => {
    expect(formatDuration(12_000)).toBe("12s");
    expect(formatDuration(65_000)).toBe("1m 5s");
    expect(thoughtHeadings("**Reading**\ntext **Editing files**")).toEqual(["Reading", "Editing files"]);
  });
});

describe("folding a batch", () => {
  const updates: SessionUpdate[] = [];
  for (let i = 0; i < 50; i++) {
    updates.push({ kind: "user", text: `q${i}` });
    updates.push({ kind: "thought", text: "**T**", summary: true });
    updates.push({ kind: "toolCall", id: `c${i}`, title: "rg", diffs: [] });
    updates.push({ kind: "toolCallUpdate", id: `c${i}`, status: "completed", outputDelta: "ok", diffs: [] });
    updates.push({ kind: "agent", text: "Hel" });
    updates.push({ kind: "agent", text: "lo" });
  }

  test("gives the same thread as folding one update at a time", () => {
    const plain = apply(updates);
    let batched = emptyThread("s", "/p");
    foldingBatch(() => {
      batched = apply(updates, batched);
    });
    const shape = (s: typeof plain) => ({
      items: s.items.map(({ id: _id, ...rest }) => rest),
      tools: Object.keys(s.toolIndex).length,
    });
    expect(shape(batched)).toEqual(shape(plain));
  });

  test("never changes arrays made before the batch", () => {
    const before = apply(updates.slice(0, 6));
    const items = before.items;
    const copy = items.slice();
    const toolIndex = { ...before.toolIndex };
    foldingBatch(() => {
      apply(updates.slice(6), before);
    });
    expect(before.items).toBe(items);
    expect(items).toEqual(copy);
    expect(before.toolIndex).toEqual(toolIndex);
  });

  test("arrays from a finished batch are not changed by the next one", () => {
    let first = emptyThread("s", "/p");
    foldingBatch(() => {
      first = apply(updates.slice(0, 6), first);
    });
    const length = first.items.length;
    foldingBatch(() => {
      apply(updates.slice(6, 12), first);
    });
    expect(first.items.length).toBe(length);
  });

  test("a streamed chunk closes only the open thought at the tail", () => {
    const s = apply([
      { kind: "thought", text: "a", summary: false },
      { kind: "agent", text: "x" },
      { kind: "thought", text: "b", summary: false },
      { kind: "user", text: "next" },
    ]);
    expect(s.items.filter((i) => i.kind === "thought").every((t) => t.kind === "thought" && t.done)).toBe(
      true,
    );
  });
});

describe("sidebar times", () => {
  test("a running turn's clock", () => {
    expect(formatClock(0)).toBe("0:00");
    expect(formatClock(42_900)).toBe("0:42");
    expect(formatClock(12 * 60_000 + 3_000)).toBe("12:03");
    expect(formatClock(3_723_000)).toBe("1:02:03");
  });

  test("a compact age", () => {
    const now = 10 * 86_400_000;
    expect(shortAge(undefined, now)).toBe("");
    expect(shortAge(now - 30_000, now)).toBe("now");
    expect(shortAge(now - 5 * 60_000, now)).toBe("5m");
    expect(shortAge(now - 3 * 3_600_000, now)).toBe("3h");
    expect(shortAge(now - 2 * 86_400_000, now)).toBe("2d");
  });
});

describe("the working line", () => {
  const user = { kind: "user", id: "u", text: "go" } as const;
  test("says what the newest step of the turn does", () => {
    expect(workingLabel([user])).toBe("Working");
    expect(
      workingLabel([
        user,
        {
          kind: "thought",
          id: "t",
          text: "**Reading the code**\n…\n**Planning the fix**",
          summary: false,
          startedAt: 0,
          done: false,
        },
      ]),
    ).toBe("Thinking · Planning the fix");
    expect(
      workingLabel([
        user,
        {
          kind: "tool",
          id: "x",
          title: "bash",
          toolKind: "execute",
          summary: "bun test",
          status: "in_progress",
          diffs: [],
        },
      ]),
    ).toBe("Running bun test");
    expect(workingLabel([user, { kind: "assistant", id: "a", text: "Here" }])).toBe("Writing the reply");
    expect(
      workingLabel([
        user,
        { kind: "thought", id: "t", text: "done", summary: false, startedAt: 0, done: true },
      ]),
    ).toBe("Working");
  });
});

describe("compaction", () => {
  test("a harness's compaction card becomes one quiet row", () => {
    const update = parseSessionUpdate({
      sessionId: "s",
      update: {
        sessionUpdate: "agent_message_chunk",
        content: { type: "text", text: "Context compacted: Codex summarized the conversation.\n\nSummary…" },
        _meta: { rustcode: { kind: "compaction" } },
      },
    });
    expect(update.kind).toBe("compaction");
    const state = applyUpdate(emptyThread("s", "/p"), update);
    expect(state.items.at(-1)).toMatchObject({
      kind: "notice",
      tone: "info",
      text: "Context compacted: Codex summarized the conversation.",
    });
  });
});

describe("auto-settle", () => {
  test("a thread untouched for three days settles", () => {
    const now = 10 * 86_400_000;
    expect(idleSettled(undefined, now)).toBe(false);
    expect(idleSettled(now - 2 * 86_400_000, now)).toBe(false);
    expect(idleSettled(now - 3 * 86_400_000, now)).toBe(true);
  });
});

describe("a turn the engine wakes", () => {
  test("runs again with a quiet line, and a parked count clears", () => {
    const parked = { ...finishTurn(beginTurn(emptyThread("s", "/p"), "go", 0), "completed", 5), waiting: 2 };
    const woken = wakeTurn(parked, 10);
    expect(woken).toMatchObject({ running: true, woken: true, waiting: undefined, turnStartedAt: 10 });
    expect(woken.items.at(-1)).toMatchObject({ kind: "notice", tone: "info" });
    // Already running (the prompt has not answered yet): nothing to add.
    const running = beginTurn(emptyThread("s", "/p"), "go", 0);
    expect(wakeTurn(running, 10).items).toHaveLength(running.items.length);
    expect(finishTurn(woken, "completed", 20).woken).toBeUndefined();
  });
});

describe("Claude Code subagents", () => {
  const call = (body: Record<string, unknown>) =>
    parseSessionUpdate({ sessionId: "s", update: { sessionUpdate: "tool_call", ...body } } as never);
  const agent = call({
    toolCallId: "a1",
    title: "Map the code",
    status: "in_progress",
    rawInput: { description: "Map the code", subagent_type: "Explore" },
    _meta: { claudeCode: { toolName: "Agent" } },
  });
  const step = call({
    toolCallId: "s1",
    title: "Read Composer.tsx",
    kind: "read",
    status: "in_progress",
    _meta: { claudeCode: { toolName: "Read", parentToolUseId: "a1" } },
  });

  test("its steps fold under the call that started it", () => {
    let t = beginTurn(emptyThread("s", "/p"), "go");
    t = applyUpdate(applyUpdate(t, agent), step);
    const { items, steps } = splitSubagentSteps(t.items);
    expect(items.some((i) => i.id === "s1")).toBe(false);
    expect(steps.get("a1")?.map((i) => i.id)).toEqual(["s1"]);
  });

  test("the working line names the agent, not its step", () => {
    let t = beginTurn(emptyThread("s", "/p"), "go");
    t = applyUpdate(applyUpdate(t, agent), step);
    expect(workingLabel(t.items)).toBe("Explore · Map the code");
  });

  const result = (body: Record<string, unknown>) =>
    parseSessionUpdate({
      sessionId: "s",
      update: { sessionUpdate: "tool_call_update", ...body },
    } as never);

  test("a subagent sent to the background keeps working until the turn ends", () => {
    let t = beginTurn(emptyThread("s", "/p"), "go");
    t = applyUpdate(t, agent);
    t = applyUpdate(
      t,
      result({
        toolCallId: "a1",
        status: "completed",
        content: [{ type: "content", content: { type: "text", text: "Async agent launched successfully." } }],
      }),
    );
    expect(t.items.at(-1)).toMatchObject({ status: "in_progress", background: "agent" });
    t = finishTurn(t, "completed");
    expect(t.items.find((i) => i.id === "a1")).toMatchObject({ status: "completed" });
  });

  test("a backgrounded command outlives the turn until the engine ends its call", () => {
    let t = beginTurn(emptyThread("s", "/p"), "go");
    t = applyUpdate(t, call({ toolCallId: "b1", title: "npm test", kind: "execute", status: "in_progress" }));
    t = applyUpdate(
      t,
      result({
        toolCallId: "b1",
        status: "completed",
        _meta: { jetbrains: { air: { asyncTasks: { backgrounded: true } } } },
      }),
    );
    t = finishTurn(t, "completed");
    expect(t.items.find((i) => i.id === "b1")).toMatchObject({ status: "in_progress", background: "task" });
    t = applyUpdate(t, result({ toolCallId: "b1", status: "completed" }));
    expect(t.items.find((i) => i.id === "b1")).toMatchObject({ status: "completed" });
  });

  test("a replayed background launch is over", () => {
    let t = applyUpdate(emptyThread("s", "/p"), agent);
    t = applyUpdate(
      t,
      result({
        toolCallId: "a1",
        status: "completed",
        content: [{ type: "content", content: { type: "text", text: "Async agent launched successfully." } }],
      }),
    );
    expect(t.items.at(-1)).toMatchObject({ status: "completed" });
  });

  test("a RustCode spawn_agent call is a subagent too", () => {
    const t = applyUpdate(
      emptyThread("s", "/p"),
      call({
        toolCallId: "r1",
        title: "spawn_agent",
        rawInput: { task: "Map the code\nmore", agent_type: "explorer" },
      }),
    );
    expect(subagentOf(t.items[0]!)).toEqual({ name: "explorer", task: "Map the code" });
  });

  test("a step whose parent is unknown stays in the thread", () => {
    let t = beginTurn(emptyThread("s", "/p"), "go");
    t = applyUpdate(t, step);
    expect(splitSubagentSteps(t.items).items.some((i) => i.id === "s1")).toBe(true);
  });
});

describe("a harness switch", () => {
  test("draws a divider where the thread moved", () => {
    const update = parseSessionUpdate({
      sessionId: "s",
      update: {
        sessionUpdate: "agent_message_chunk",
        content: { type: "text", text: "Switched from Claude Code to Codex" },
        _meta: { rustcode: { kind: "handoff", from: "claude-code", to: "codex", model: "gpt-6-luna" } },
      },
    } as never);
    const t = applyUpdate(emptyThread("s", "/p"), update);
    expect(t.items).toEqual([
      expect.objectContaining({ kind: "handoff", from: "claude-code", to: "codex", model: "gpt-6-luna" }),
    ]);
  });
});

describe("checkCommands", () => {
  test("finds the checks a command line runs", () => {
    expect(checkCommands("/bin/zsh -lc 'bun --version && bun test'")).toEqual(["bun test"]);
    expect(checkCommands("cargo clippy --all-targets -- -D warnings")).toEqual([
      "cargo clippy --all-targets -- -D warnings",
    ]);
    expect(checkCommands("bun run check")).toEqual(["bun run check"]);
    expect(checkCommands("npx tsc --noEmit; pytest -q")).toEqual(["npx tsc --noEmit", "pytest -q"]);
    expect(checkCommands("CI=1 cargo +nightly test -p x")).toEqual(["cargo +nightly test -p x"]);
  });

  test("a word in a path or an option is not a check", () => {
    expect(checkCommands("/bin/zsh -lc 'nl -ba src/queue.test.ts'")).toEqual([]);
    expect(checkCommands("git diff --check")).toEqual([]);
    expect(checkCommands("cat build/test.log")).toEqual([]);
    expect(checkCommands("bun --version")).toEqual([]);
  });
});
