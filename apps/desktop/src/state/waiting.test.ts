import { describe, expect, test } from "bun:test";
import type { Link, LinkState } from "./orchestration";
import { type Waiting, waitingIds, waitingLabel, waitingNext, waitingOn, waitingTitle } from "./waiting";

const link = (id: string, orchestrator: string, target: string, state: LinkState): Link => ({
  id,
  orchestrator,
  target,
  orchestratorTitle: "Parent",
  targetTitle: `Thread ${target}`,
  message: "do it",
  state,
  createdAt: 0,
});

/** The reads `waitingOn` needs of the app: the showing state, and what owes it an answer. */
const read = (
  shown: Record<string, LinkState>,
  rest: { running?: boolean; commands?: number; links: Link[]; quiet?: string[]; runs?: string[] },
) =>
  waitingOn({
    id: "p",
    running: rest.running ?? false,
    commands: rest.commands,
    links: rest.links,
    state: (l) => shown[l.id] ?? l.state,
    // `quiet` lists the targets with work still going under them; `runs` the ones whose own turn runs.
    quiet: (target) => !(rest.quiet ?? []).includes(target) && !(rest.runs ?? []).includes(target),
    quietBelow: (target) => !(rest.quiet ?? []).includes(target),
  });

describe("waiting on background work", () => {
  test("a live link it started or messaged is what it waits on", () => {
    const links = [
      link("p/1", "p", "a", "working"),
      link("p/2", "p", "b", "queued"),
      link("p/3", "p", "c", "needs_user"),
    ];
    expect(read({}, { links })?.links.map((l) => l.id)).toEqual(["p/1", "p/2", "p/3"]);
  });

  test("a settled link whose own subtree still works is not an answer yet", () => {
    const links = [link("p/1", "p", "a", "done")];
    // Quiet: done is done.
    expect(read({}, { links })).toBeUndefined();
    // The child's own child still works: the parent is not finished either.
    expect(read({}, { links, quiet: ["a"] })?.links.map((l) => l.id)).toEqual(["p/1"]);
  });

  test("a turn the target runs after it answered is not this thread's to wait on", () => {
    // It messaged "a" once and had its answer; the user now writes to "a" in its own thread.
    const links = [link("p/1", "p", "a", "done")];
    expect(read({ "p/1": "working" }, { links, runs: ["a"] })).toBeUndefined();
    // A link still waiting for its answer keeps waiting while its target works.
    const live = [link("p/1", "p", "a", "working")];
    expect(read({}, { links: live, runs: ["a"] })?.links.map((l) => l.id)).toEqual(["p/1"]);
  });

  test("the state as shown counts: a child waiting on you is still worked with", () => {
    const links = [link("p/1", "p", "a", "working")];
    expect(read({ "p/1": "needs_user" }, { links })?.links).toHaveLength(1);
    // Its own turn ended.
    expect(read({ "p/1": "done" }, { links })).toBeUndefined();
  });

  test("a turn of its own means it works, not that it waits", () => {
    const links = [link("p/1", "p", "a", "working")];
    expect(read({}, { links, running: true })).toBeUndefined();
    expect(read({}, { links, running: true, commands: 2 })).toBeUndefined();
  });

  test("background commands its last turn parked on count on their own", () => {
    expect(read({}, { links: [], commands: 2 })?.commands).toBe(2);
    expect(read({}, { links: [], commands: 0 })).toBeUndefined();
    expect(read({}, { links: [] })).toBeUndefined();
  });

  test("another thread's links are not what this one waits on", () => {
    const links = [link("q/1", "q", "a", "working"), link("p/1", "p", "b", "working")];
    expect(read({}, { links })?.links.map((l) => l.id)).toEqual(["p/1"]);
  });

  test("the line names the threads, then the commands", () => {
    const one = (links: Link[], commands = 0): Waiting => ({ links, commands });
    expect(waitingLabel(one([link("p/1", "p", "a", "working")]))).toBe("Waiting on 1 thread");
    expect(waitingLabel(one([link("p/1", "p", "a", "working"), link("p/2", "p", "b", "working")]))).toBe(
      "Waiting on 2 threads",
    );
    expect(waitingLabel(one([], 1))).toBe("Waiting on a background command");
    expect(waitingLabel(one([], 3))).toBe("Waiting on 3 background commands");
    expect(waitingLabel(one([link("p/1", "p", "a", "working")], 2))).toBe(
      "Waiting on 1 thread and 2 background commands",
    );
    // The tooltip names them: the line has no room for a title.
    expect(waitingTitle(one([link("p/1", "p", "a", "working"), link("p/2", "p", "b", "working")]))).toBe(
      "Thread a, Thread b",
    );
  });

  test("a delegate messaged twice is one thread on the line, at its latest message", () => {
    const links = [
      { ...link("p/1", "p", "a", "done"), createdAt: 1, settledAt: 2 },
      { ...link("p/2", "p", "a", "working"), createdAt: 3 },
    ];
    const wait = read({}, { links, quiet: ["a"] })!;
    // The live message speaks for the thread, and the footer counts the thread once.
    expect(wait.links.map((l) => l.id)).toEqual(["p/2"]);
    expect(waitingLabel(wait)).toBe("Waiting on 1 thread");
    expect(waitingTitle(wait)).toBe("Thread a");
    // The line counts threads even when handed the links themselves.
    expect(waitingLabel({ links, commands: 0 })).toBe("Waiting on 1 thread");
  });
});

describe("the threads the list draws with the ones that work", () => {
  const ids = ["p", "q", "r"];
  const wait = (
    links: Link[],
    rest: { running?: string[]; parked?: Record<string, number>; quiet?: string[]; runs?: string[] } = {},
  ) => [
    ...waitingIds({
      ids,
      running: new Set(rest.running ?? []),
      parked: new Map(Object.entries(rest.parked ?? {})),
      links,
      state: (l) => l.state,
      quiet: (target) => !(rest.quiet ?? []).includes(target) && !(rest.runs ?? []).includes(target),
      quietBelow: (target) => !(rest.quiet ?? []).includes(target),
    }),
  ];

  test("a thread waiting on a link it handed out is one, a thread that runs is not", () => {
    const links = [link("p/1", "p", "a", "working"), link("q/1", "q", "b", "working")];
    expect(wait(links)).toEqual(["p", "q"]);
    // A turn of its own: it works, and is drawn as working, not as waiting.
    expect(wait(links, { running: ["q"] })).toEqual(["p"]);
  });

  test("a settled link whose subtree still works keeps its parent waiting, a quiet one does not", () => {
    const links = [link("p/1", "p", "a", "done"), link("q/1", "q", "b", "done")];
    expect(wait(links, { quiet: ["a"] })).toEqual(["p"]);
  });

  test("a settled thread that once messaged another does not come back while the user works there", () => {
    expect(wait([link("p/1", "p", "a", "done")], { runs: ["a"] })).toEqual([]);
  });

  test("commands a thread parked on count on their own, and idle threads are not waiting", () => {
    expect(wait([], { parked: { r: 2 } })).toEqual(["r"]);
    expect(wait([], { parked: { p: 0 } })).toEqual([]);
    expect(wait([link("p/1", "p", "a", "done")])).toEqual([]);
  });
});

describe("waitingNext", () => {
  test("says the thread picks up by itself, for one thing or several", () => {
    expect(waitingNext({ links: [], commands: 1 })).toBe("continues on its own when it finishes");
    expect(waitingNext({ links: [], commands: 2 })).toBe("continues on its own when they finish");
  });
});
