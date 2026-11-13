import { describe, expect, test } from "bun:test";
import {
  DRIFT_GRACE_MS,
  engineBusy,
  FORCE_STOP_MS,
  noDrift,
  POLL_ACTIVE_MS,
  POLL_IDLE_MS,
  pollDelay,
  reconcileRuns,
  stopOverdue,
} from "./runState";

const NOW = 1_000_000;
const old = NOW - DRIFT_GRACE_MS - 1;

describe("reconcileRuns", () => {
  test("a running turn the engine calls idle is settled on the second read, not the first", () => {
    const local = [{ id: "a", running: true, startedAt: old }];
    const first = reconcileRuns(local, [{ id: "a", status: "idle" }], noDrift, NOW);
    expect(first.settle).toEqual([]);
    const second = reconcileRuns(local, [{ id: "a", status: "done" }], first.reads, NOW + 5_000);
    expect(second.settle).toEqual(["a"]);
  });

  test("a turn that only just started is left alone however the list reads", () => {
    const local = [{ id: "a", running: true, startedAt: NOW - 1_000 }];
    let reads = noDrift;
    for (let i = 0; i < 4; i++) {
      const result = reconcileRuns(local, [{ id: "a", status: "idle" }], reads, NOW);
      expect(result.settle).toEqual([]);
      reads = result.reads;
    }
  });

  test("a turn with no recorded start counts as old", () => {
    const local = [{ id: "a", running: true }];
    const first = reconcileRuns(local, [{ id: "a", status: "review" }], noDrift, NOW);
    const second = reconcileRuns(local, [{ id: "a", status: "review" }], first.reads, NOW);
    expect(second.settle).toEqual(["a"]);
  });

  test("one read that agrees again clears the count", () => {
    const local = [{ id: "a", running: true, startedAt: old }];
    const first = reconcileRuns(local, [{ id: "a", status: "idle" }], noDrift, NOW);
    const agreed = reconcileRuns(local, [{ id: "a", status: "running" }], first.reads, NOW);
    expect(agreed.reads.stale).toEqual({});
    const again = reconcileRuns(local, [{ id: "a", status: "idle" }], agreed.reads, NOW);
    expect(again.settle).toEqual([]);
  });

  test("a thread the engine runs and the window thinks idle is adopted on the second read", () => {
    const local = [{ id: "a", running: false }];
    const first = reconcileRuns(local, [{ id: "a", status: "running" }], noDrift, NOW);
    expect(first.adopt).toEqual([]);
    const second = reconcileRuns(local, [{ id: "a", status: "running" }], first.reads, NOW);
    expect(second.adopt).toEqual(["a"]);
    // It stays adopted for as long as the engine keeps saying so.
    const third = reconcileRuns(local, [{ id: "a", status: "needs_input" }], second.reads, NOW);
    expect(third.adopt).toEqual(["a"]);
  });

  test("a thread this window never opened is adopted the same way", () => {
    const first = reconcileRuns([], [{ id: "x", status: "running" }], noDrift, NOW);
    const second = reconcileRuns([], [{ id: "x", status: "running" }], first.reads, NOW);
    expect(second.adopt).toEqual(["x"]);
  });

  test("an adopted thread is let go when the engine stops naming it busy", () => {
    const local = [{ id: "a", running: false }];
    const one = reconcileRuns(local, [{ id: "a", status: "running" }], noDrift, NOW);
    const two = reconcileRuns(local, [{ id: "a", status: "running" }], one.reads, NOW);
    const over = reconcileRuns(local, [{ id: "a", status: "idle" }], two.reads, NOW);
    expect(over.adopt).toEqual([]);
    expect(over.reads.missed).toEqual({});
  });

  test("an engine that names no status settles and adopts nothing", () => {
    const local = [{ id: "a", running: true, startedAt: old }];
    let reads = noDrift;
    for (let i = 0; i < 3; i++) {
      const result = reconcileRuns(local, [{ id: "a" }], reads, NOW);
      expect(result).toMatchObject({ settle: [], adopt: [] });
      reads = result.reads;
    }
  });

  test("a thread that drops out of the list loses its count", () => {
    const local = [{ id: "a", running: true, startedAt: old }];
    const first = reconcileRuns(local, [{ id: "a", status: "idle" }], noDrift, NOW);
    const gone = reconcileRuns(local, [], first.reads, NOW);
    expect(gone.reads).toEqual(noDrift);
  });

  test("agreeing threads are untouched", () => {
    const local = [
      { id: "a", running: true, startedAt: old },
      { id: "b", running: false },
    ];
    const result = reconcileRuns(
      local,
      [
        { id: "a", status: "running" },
        { id: "b", status: "idle" },
      ],
      noDrift,
      NOW,
    );
    expect(result).toEqual({ settle: [], adopt: [], reads: noDrift });
  });
});

describe("timing", () => {
  test("engineBusy names running and needs_input only", () => {
    expect(engineBusy("running")).toBe(true);
    expect(engineBusy("needs_input")).toBe(true);
    for (const status of ["idle", "review", "done", undefined] as const)
      expect(engineBusy(status)).toBe(false);
  });

  test("the list is read quickly while something runs and slowly when all is idle", () => {
    expect(pollDelay(true)).toBe(POLL_ACTIVE_MS);
    expect(pollDelay(false)).toBe(POLL_IDLE_MS);
    expect(POLL_ACTIVE_MS).toBeLessThanOrEqual(5_000);
  });

  test("a stop is forced once it has waited the limit", () => {
    expect(stopOverdue(NOW, NOW + FORCE_STOP_MS - 1)).toBe(false);
    expect(stopOverdue(NOW, NOW + FORCE_STOP_MS)).toBe(true);
  });
});
