// The pure half of visible steering: which plan step was active when a note was read, where each
// note's marker goes, and how long a step has run (`state/steer.ts`).

import { describe, expect, test } from "bun:test";
import type { PlanEntry } from "../acp/types";
import {
  activeStep,
  advancePlanClocks,
  planCardShown,
  planClock,
  type SteerNote,
  steerLine,
  steerMarks,
  stepElapsed,
} from "./steer";

const step = (content: string, status: PlanEntry["status"] = "pending"): PlanEntry => ({ content, status });
const note = (id: string, at?: { index: number; content: string }): SteerNote => ({
  id,
  text: `${id} noted`,
  at: 1000,
  step: at,
});

describe("the step a turn is on", () => {
  test("is the one in progress, and nothing when the plan has no step on", () => {
    expect(activeStep([step("a", "completed"), step("b", "in_progress"), step("c", "in_progress")])).toEqual({
      index: 1,
      content: "b",
    });
    expect(activeStep([step("a", "completed"), step("b")])).toBeUndefined();
    expect(activeStep([])).toBeUndefined();
  });
});

describe("plan step clocks", () => {
  test("a clock starts when the turn comes to a step and stops when it finishes it", () => {
    let clocks = advancePlanClocks({}, [step("a", "completed"), step("b", "in_progress")], 1000, true);
    expect(clocks).toEqual({ b: { startedAt: 1000 } });
    // The same step in a later update keeps its start: the clock runs from when it came up.
    clocks = advancePlanClocks(clocks, [step("a", "completed"), step("b", "in_progress")], 4000, true);
    expect(clocks).toEqual({ b: { startedAt: 1000 } });
    expect(stepElapsed(step("b", "in_progress"), planClock(clocks, "b"), 7000, true)).toBe(6000);
    clocks = advancePlanClocks(clocks, [step("a", "completed"), step("b", "completed")], 8000, true);
    expect(clocks).toEqual({ b: { startedAt: 1000, endedAt: 8000 } });
  });

  test("a finished step keeps its whole time, and a running one is only clocked while the turn runs", () => {
    const clock = { startedAt: 1000, endedAt: 8000 };
    expect(stepElapsed(step("b", "completed"), clock, 99_000, false)).toBe(7000);
    expect(stepElapsed(step("b", "completed"), clock, 99_000, true)).toBe(7000);
    // A step still on in a thread that is not running says nothing: its end was never seen.
    expect(stepElapsed(step("b", "in_progress"), { startedAt: 1000 }, 99_000, false)).toBeUndefined();
    // And a step the app never saw start has no duration at all.
    expect(stepElapsed(step("b", "completed"), undefined, 99_000, true)).toBeUndefined();
  });

  test("a step the app only sees finished is not given a made-up duration", () => {
    const clocks = advancePlanClocks({}, [step("a", "completed")], 5000, true);
    expect(clocks).toEqual({});
    expect(stepElapsed(step("a", "completed"), planClock(clocks, "a"), 6000, true)).toBeUndefined();
  });

  test("nothing is clocked for a thread that is not running: a replay is not a timeline", () => {
    expect(advancePlanClocks({}, [step("a", "in_progress")], 1000, false)).toEqual({});
    const before = { a: { startedAt: 10 } };
    expect(advancePlanClocks(before, [step("a", "completed")], 9000, false)).toBe(before);
  });

  test("a step that comes up again starts a new clock, and a dropped step takes its clock with it", () => {
    let clocks = advancePlanClocks({}, [step("a", "in_progress")], 1000, true);
    clocks = advancePlanClocks(clocks, [step("a", "completed")], 3000, true);
    clocks = advancePlanClocks(clocks, [step("a", "in_progress")], 5000, true);
    expect(clocks).toEqual({ a: { startedAt: 5000 } });
    expect(advancePlanClocks(clocks, [step("b", "in_progress")], 6000, true)).toEqual({
      b: { startedAt: 6000 },
    });
  });

  test("a step put back to pending keeps the clock it had", () => {
    expect(advancePlanClocks({ a: { startedAt: 10 } }, [step("a")], 4000, true)).toEqual({
      a: { startedAt: 10 },
    });
  });
});

describe("where a note's marker goes", () => {
  test("a note sits on its plan step while the plan still lists it", () => {
    const marked = steerMarks([note("n1", { index: 1, content: "b" })], [step("a", "completed"), step("b")]);
    expect([...marked.byStep.keys()]).toEqual(["b"]);
    expect(marked.byStep.get("b")?.map((n) => n.id)).toEqual(["n1"]);
    expect(marked.inline).toEqual([]);
    // Two notes the turn read on one step keep the order it read them in.
    const two = steerMarks(
      [note("n1", { index: 1, content: "b" }), note("n2", { index: 1, content: "b" })],
      [step("b", "in_progress")],
    );
    expect(two.byStep.get("b")?.map((n) => n.id)).toEqual(["n1", "n2"]);
  });

  test("a note with no step, and one the plan moved past, draw in the transcript", () => {
    const notes = [note("n1", { index: 1, content: "b" }), note("n2")];
    const marked = steerMarks(notes, [step("a", "completed"), step("b", "completed")]);
    expect(marked.inline.map((n) => n.id)).toEqual(["n2"]);
    // A plan rewritten past that step gives the note back to the transcript, so it is never lost.
    expect(steerMarks(notes, [step("a", "completed")]).inline.map((n) => n.id)).toEqual(["n1", "n2"]);
  });

  test("the card shows for work left or for a note it holds, and not for an empty plan", () => {
    expect(planCardShown([], [])).toBe(false);
    expect(planCardShown([step("a", "in_progress")], [])).toBe(true);
    expect(planCardShown([step("a", "completed")], [])).toBe(false);
    // All done, but a note landed on a step: the card is the note's only place.
    const note1 = note("n1", { index: 0, content: "a" });
    expect(planCardShown([step("a", "completed")], [note1])).toBe(true);
    expect(planCardShown([step("a", "completed")], [note("n2")])).toBe(false);
  });
});

describe("a note as one line", () => {
  test("is its first line, squashed, and clipped where it is long", () => {
    expect(steerLine("  use the fast path\nand the cache")).toBe("use the fast path");
    expect(steerLine("one\n\ntwo")).toBe("one");
    const long = `${"a".repeat(200)}`;
    expect(steerLine(long, 20)).toBe(`${"a".repeat(19)}…`);
    expect(steerLine("", 20)).toBe("");
  });
});
