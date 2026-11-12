// A message steered into a delegate's running turn (`message_session` with `steer: true`) has no
// turn of its own to settle its link, so the running turn does when it ends (issue 3532): the
// sidebar and the parent's delegate row stop saying "working" without the thread being opened.

import { afterAll, beforeEach, describe, expect, test } from "bun:test";
import { linkSteered, requeueUnreadSteers, takeSteeredLinks, threadTurnEnded } from "./host";
import { emptyBook, type Link, liveState } from "./orchestration";
import { resetForTests, statusOf, updateThread, useApp } from "./store";
import { emptyThread, requeueSteers } from "./thread";

const PARENT = "parent";
const TARGET = "delegate";
const CWD = "/Users/you/code/rustcode";

const link = (id: string, extra: Partial<Link> = {}): Link => ({
  id,
  orchestrator: PARENT,
  target: TARGET,
  orchestratorTitle: "Fix the tests",
  targetTitle: "Run the tests",
  message: id,
  state: "working",
  started: true,
  createdAt: 1_000,
  startedAt: 2_000,
  // Already read by the parent: no wake turn starts after the test.
  notifiedAt: 3_000,
  ...extra,
});

const state = (id: string) => useApp.getState().links.links.find((l) => l.id === id);
/** What the sidebar and the parent's delegate row read for the delegate's link. */
const shown = (id: string) => liveState(state(id)!, statusOf(useApp.getState(), TARGET));

beforeEach(() => {
  resetForTests();
  useApp.setState((s) => ({
    selectedId: null,
    links: { ...emptyBook(), links: [link("own"), link("steer-1"), link("steer-2")] },
    sessions: [
      { id: PARENT, cwd: CWD, title: "Fix the tests", updatedAt: 1_000 },
      { id: TARGET, cwd: CWD, title: "Run the tests", updatedAt: 1_000 },
    ],
    threads: {
      ...s.threads,
      [TARGET]: { ...emptyThread(TARGET, CWD, "Run the tests"), loaded: true, running: true },
    },
  }));
});

afterAll(() => resetForTests());

const turnEnds = (outcome: "completed" | "cancelled" | "failed", failed?: string) => {
  useApp.setState((s) => ({
    threads: { ...s.threads, [TARGET]: { ...s.threads[TARGET]!, running: false } },
  }));
  threadTurnEnded(TARGET, false, outcome, failed, [
    { kind: "assistant", id: "a1", text: "All 12 tests pass" },
  ]);
};

describe("a delegate steered mid-turn", () => {
  test("steer, steer, end: both links stop working with the delegate closed", () => {
    linkSteered(TARGET, "steer-1", "first change");
    linkSteered(TARGET, "steer-2", "second change");
    expect([shown("steer-1"), shown("steer-2")]).toEqual(["working", "working"]);
    turnEnds("completed");
    expect([state("steer-1")?.state, state("steer-2")?.state]).toEqual(["done", "done"]);
    expect(state("steer-1")?.answer).toBe("All 12 tests pass");
    expect([shown("steer-1"), shown("steer-2")]).toEqual(["done", "done"]);
    // The link the turn itself started is settled by its own prompt, not here.
    expect(state("own")?.state).toBe("working");
  });

  test("a stopped turn cancels the links steered into it, a failed one fails them", () => {
    linkSteered(TARGET, "steer-1", "first change");
    turnEnds("cancelled");
    expect(state("steer-1")?.state).toBe("cancelled");
    useApp.setState((s) => ({
      threads: { ...s.threads, [TARGET]: { ...s.threads[TARGET]!, running: true } },
    }));
    linkSteered(TARGET, "steer-2", "second change");
    turnEnds("failed", "the provider refused");
    expect(state("steer-2")?.state).toBe("failed");
  });

  test("a steer the turn never read keeps its link working and rides the requeued turn", () => {
    linkSteered(TARGET, "steer-1", "wrap up now");
    linkSteered(TARGET, "steer-2", "also this");
    // The engine read the second one; the first came back with the turn's answer (`unusedSteers`).
    updateThread(TARGET, (t) => requeueSteers(t, ["wrap up now"]));
    requeueUnreadSteers(TARGET, ["wrap up now"]);
    turnEnds("completed");
    expect(state("steer-2")?.state).toBe("done");
    // Not settled with the reply of a turn that never read it, and not woken for.
    expect(state("steer-1")?.state).toBe("working");
    expect(state("steer-1")?.answer).toBeUndefined();
    expect(useApp.getState().threads[TARGET]?.queue.map((q) => [q.text, q.link])).toEqual([
      ["wrap up now", "steer-1"],
    ]);
  });

  test("a lost remote turn carries the steered links on, and a later turn does not settle them", () => {
    linkSteered(TARGET, "steer-1", "first change");
    linkSteered(TARGET, "steer-2", "second change");
    expect(takeSteeredLinks(TARGET)).toEqual(["steer-1", "steer-2"]);
    expect(takeSteeredLinks(TARGET)).toEqual([]);
    turnEnds("completed");
    expect([state("steer-1")?.state, state("steer-2")?.state]).toEqual(["working", "working"]);
  });

  test("a steer that lands after the turn ended goes as a turn of its own, never with an older reply", () => {
    useApp.setState((s) => ({
      threads: { ...s.threads, [TARGET]: { ...s.threads[TARGET]!, running: false } },
    }));
    // The engine handed it back already: the queued copy carries the link.
    updateThread(TARGET, (t) => ({ ...t, queue: [{ id: "q1", text: "wrap up now" }] }));
    linkSteered(TARGET, "steer-1", "wrap up now");
    expect(useApp.getState().threads[TARGET]?.queue.map((q) => q.link)).toEqual(["steer-1"]);
    expect(state("steer-1")?.state).toBe("working");
    // Nothing queued: it is sent now (no engine here, so it only stays unanswered).
    linkSteered(TARGET, "steer-2", "second change");
    expect(state("steer-2")?.state).toBe("working");
    expect(state("steer-2")?.answer).toBeUndefined();
  });

  test("a link steered into one turn is not settled by the next", () => {
    linkSteered(TARGET, "steer-1", "first change");
    turnEnds("completed");
    useApp.setState((s) => ({
      threads: { ...s.threads, [TARGET]: { ...s.threads[TARGET]!, running: true } },
    }));
    linkSteered(TARGET, "steer-2", "second change");
    expect(state("steer-2")?.state).toBe("working");
    turnEnds("completed");
    expect(state("steer-2")?.state).toBe("done");
  });
});
