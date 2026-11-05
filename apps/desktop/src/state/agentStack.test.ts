import { describe, expect, test } from "bun:test";
import { agentStack, STACK_VISIBLE, type StackAgent, type StackState } from "./agentStack";

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
});
