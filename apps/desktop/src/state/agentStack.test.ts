import { describe, expect, test } from "bun:test";
import {
  agentStack,
  delegateCount,
  liveSubagents,
  STACK_VISIBLE,
  type StackAgent,
  type StackState,
  stackDescription,
} from "./agentStack";
import type { TranscriptItem } from "./thread";

const agent = (id: string, state: StackState, harness = "codex"): StackAgent => ({
  id,
  state,
  harness,
  label: `Thread ${id}`,
});

const running = (n: number) => Array.from({ length: n }, (_, i) => agent(`a${i}`, "working"));

describe("the icons a stack draws", () => {
  test("nothing running draws nothing", () => {
    expect(agentStack([])).toMatchObject({ visible: [], extra: 0, order: [], running: false });
    expect(agentStack([agent("done", "done")])).toMatchObject({
      visible: [],
      extra: 0,
      running: false,
    });
  });

  test("one, three, four and five running: three icons at most, then the count", () => {
    expect(agentStack(running(1)).visible.map((a) => a.id)).toEqual(["a0"]);
    expect(agentStack(running(1)).extra).toBe(0);
    expect(agentStack(running(3)).visible).toHaveLength(STACK_VISIBLE);
    expect(agentStack(running(3)).extra).toBe(0);
    expect(agentStack(running(4)).visible.map((a) => a.id)).toEqual(["a0", "a1", "a2"]);
    expect(agentStack(running(4)).extra).toBe(1);
    expect(agentStack(running(5)).extra).toBe(2);
    // The label counts every one, not only the icons drawn.
    expect(agentStack(running(5)).label).toBe("5 working");
  });

  test("what needs you comes first, however it was ordered before", () => {
    const agents = [agent("work", "working"), agent("done", "done"), agent("ask", "needs")];
    expect(agentStack(agents).order.map((a) => a.id)).toEqual(["ask", "work"]);
    expect(agentStack(agents).visible.map((a) => a.id)).toEqual(["ask", "work"]);
    expect(agentStack(agents).label).toBe("1 needs you · 1 working");
  });

  test("ties keep their order, so the icons do not jump on an update that changes nothing", () => {
    const agents = [agent("b", "working"), agent("a", "working"), agent("c", "needs")];
    expect(agentStack(agents).order.map((a) => a.id)).toEqual(["c", "b", "a"]);
    // The same list answers the same way twice.
    expect(agentStack(agents).order.map((a) => a.id)).toEqual(agentStack(agents).order.map((a) => a.id));
  });

  test("finished delegates are left out while anything runs, and on their own they hide", () => {
    const agents = [agent("done", "done"), agent("work", "working"), agent("failed", "failed")];
    expect(agentStack(agents).order.map((a) => a.id)).toEqual(["work"]);
    expect(agentStack([agent("done", "done"), agent("failed", "failed")]).order).toEqual([]);
  });

  test("a group with nothing live can keep its finished delegates", () => {
    const agents = [agent("done", "done"), agent("failed", "failed")];
    const view = agentStack(agents, { idle: "show" });
    expect(view.order.map((a) => a.id)).toEqual(["done", "failed"]);
    expect(view.running).toBe(false);
    expect(view.label).toBe("2 finished");
    // While anything runs, only what runs is shown, even when idle is shown.
    const mixed = [agent("done", "done"), agent("work", "working")];
    expect(agentStack(mixed, { idle: "show" }).order.map((a) => a.id)).toEqual(["work"]);
  });

  test("the whole stack answers with one state, for the one badge it draws", () => {
    // What needs you outranks work under way, which outranks what has finished.
    expect(agentStack([agent("w", "working"), agent("n", "needs")]).state).toBe("needs");
    expect(agentStack(running(2)).state).toBe("working");
    // A finished stack is `done`, so no badge is drawn: the text beside it already says so.
    expect(agentStack([agent("d", "done")], { idle: "show" }).state).toBe("done");
    expect(agentStack([]).state).toBe("done");
  });

  test("a failed mark shows on its own once nothing runs, under what needs you", () => {
    expect(agentStack([agent("f", "failed"), agent("d", "done")], { idle: "show" }).state).toBe("failed");
    // A live mark wins over a failed one that sits beside it: the stack only badges one state.
    expect(agentStack([agent("f", "failed"), agent("w", "working")]).state).toBe("working");
    expect(agentStack([agent("f", "failed"), agent("n", "needs")]).state).toBe("needs");
  });

  test("threads and subagents are counted apart, so the tip can say which is which", () => {
    const view = agentStack([
      { id: "t", state: "working", kind: "thread", label: "Child" },
      { id: "s", state: "needs", kind: "subagent", label: "Explorer" },
    ]);
    expect(view.threads).toBe(1);
    expect(view.subagents).toBe(1);
    expect(view.summary).toBe("1 thread and 1 subagent");
    // The kind does not change what the state label says.
    expect(view.label).toBe("1 needs you · 1 working");
    // A caller that leaves the kind out still gets a stack, with no kinds to state.
    expect(agentStack([agent("a", "working")])).toMatchObject({
      threads: 0,
      subagents: 0,
      summary: "",
    });
  });

  test("the accessible description names every mark, its state and its kind", () => {
    const view = agentStack([
      { id: "t", state: "working", kind: "thread", label: "Child" },
      { id: "s", state: "needs", kind: "subagent", label: "Explorer" },
    ]);
    expect(stackDescription(view)).toBe(
      "Delegated work: 1 thread and 1 subagent · 1 needs you · 1 working. needs you Explorer (subagent); working Child (thread)",
    );
  });
});

describe("a thread's own subagents", () => {
  const call = (id: string, status: "in_progress" | "completed", toolName = "Agent") =>
    ({
      kind: "tool",
      id,
      title: "Agent",
      toolName,
      status,
      input: { description: "Build the kit", subagent_type: "general-purpose" },
    }) as unknown as TranscriptItem;

  test("a built-in subagent at work is a mark of the thread's harness", () => {
    const marks = liveSubagents([call("a", "in_progress"), call("b", "completed")], [], true, "claude-code");
    expect(marks).toEqual([
      { id: "a", state: "working", kind: "subagent", harness: "claude-code", label: "Build the kit" },
    ]);
  });

  test("other tools are not agents, and a turn that ended has none", () => {
    expect(liveSubagents([call("a", "in_progress", "Read")], [], true, "claude-code")).toEqual([]);
    expect(liveSubagents([call("a", "in_progress")], [], false, "claude-code")).toEqual([]);
  });
});

describe("a row's delegate count", () => {
  test("nothing running counts nothing", () => {
    expect(delegateCount(undefined)).toBeUndefined();
    expect(delegateCount([agent("d", "done")])).toBeUndefined();
  });

  test("names the running delegates by title, else by harness", () => {
    const untitled: StackAgent = { id: "u", state: "working", harness: "claude-code" };
    expect(
      delegateCount([agent("a", "working"), untitled], (id) => (id === "claude-code" ? "Claude Code" : id)),
    ).toEqual({
      count: 2,
      title: "2 delegates working: Thread a, Claude Code",
    });
  });

  test("one delegate is singular, and what needs you is said", () => {
    expect(delegateCount([agent("a", "working")])?.title).toBe("1 delegate working: Thread a");
    expect(delegateCount([agent("a", "needs"), agent("b", "working")])?.title).toBe(
      "2 delegates working (1 needs you): Thread a, Thread b",
    );
  });
});
