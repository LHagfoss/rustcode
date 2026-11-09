// The pull request cache: one read per answer, painted at once and refreshed
// behind, saved on the device, and read ahead when the pointer rests on a row.

import { afterEach, beforeEach, describe, expect, setSystemTime, test } from "bun:test";
import type { Json, JsonObject } from "../acp/jsonrpc";
import type { AgentInfo } from "../acp/types";
import {
  clearPullQueries,
  hydratePullQueries,
  loadPullDiff,
  PULL_FRESH_MS,
  PULL_LIMIT,
  prefetchPullDetail,
  pullDetail,
  pullDetailError,
  pullDetailLive,
  pullDiff,
  pullDiffError,
  pullList,
  pullListError,
  pullQueriesVersion,
  pullSummary,
  readPullDetail,
  readPullDiff,
  readPullList,
  SAVED_DETAILS,
  SAVED_KEY,
  SAVED_MAX_AGE_MS,
  SAVED_PROJECTS,
  SAVED_TOTAL_BYTES,
  SAVED_VERSION,
  savedPayload,
  savePullQueries,
  setPullReader,
} from "./pullQueries";
import { loadPulls, PULLS_TTL } from "./pulls";
import { useApp } from "./store";

const NOW = 1_760_000_000_000;
const CWD = "/repo";
const at = (minutesAgo: number) => new Date(NOW - minutesAgo * 60_000);

function wirePull(over: JsonObject = {}): JsonObject {
  return {
    number: 12,
    title: "feat(desktop): a branch",
    author: "you",
    state: "open",
    draft: false,
    checks: { passed: 2, failed: 0, pending: 1, skipped: 0 },
    headRef: "feat/desktop-branch",
    baseRef: "main",
    headSha: "a1b2c3d4",
    updatedAt: at(20).toISOString(),
    url: "https://github.com/you/repo/pull/12",
    additions: 12,
    deletions: 3,
    changedFiles: 2,
    ...over,
  };
}

const listAnswer = (pulls: JsonObject[], hasMore = false): Json => ({
  list: { provider: "github", hasMore, pulls },
});

const detailPull = (over: JsonObject = {}): JsonObject => ({
  ...wirePull(over),
  body: "The body.",
  checkRuns: [{ name: "desktop / check", status: "passed" }],
  files: [{ path: "apps/desktop/src/a.ts", additions: 12, deletions: 3 }],
  activity: [{ author: "reviewer", kind: "COMMENTED", at: at(40).toISOString(), body: "Looks good." }],
});

const detailAnswer = (over: JsonObject = {}): Json => ({ pull: detailPull(over) });

const failure = (code: string, message: string): Json => ({ error: { code, message } });

/** What the store asked the engine, in order. */
let asked: { method: string; params: JsonObject }[] = [];
/** The answer each method gets. */
let answers: Record<string, (params: JsonObject) => Json | Promise<Json>> = {};

function reader(method: string, params: JsonObject): Promise<Json> {
  asked.push({ method, params });
  const answer = answers[method];
  if (!answer) return Promise.reject(new Error(`no answer for ${method}`));
  return Promise.resolve(answer(params));
}

function answerWith(next: Record<string, (params: JsonObject) => Json | Promise<Json>>) {
  answers = next;
}

function deferred<T>() {
  let settle: (value: T) => void = () => undefined;
  const promise = new Promise<T>((resolve) => {
    settle = resolve;
  });
  return { promise, settle };
}

/** A stand-in for the window's storage, which the test process does not have. */
function fakeStorage() {
  const data = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", {
    value: {
      getItem: (key: string) => data.get(key) ?? null,
      setItem: (key: string, value: string) => void data.set(key, value),
      removeItem: (key: string) => void data.delete(key),
    },
    configurable: true,
    writable: true,
  });
  return {
    read: () => data.get(SAVED_KEY),
    write: (value: unknown) => data.set(SAVED_KEY, JSON.stringify(value)),
  };
}

beforeEach(() => {
  setSystemTime(new Date(NOW));
  asked = [];
  answers = {};
  setPullReader(reader);
  clearPullQueries();
});

afterEach(() => {
  clearPullQueries();
  useApp.setState({ agent: undefined, toasts: [] });
  setSystemTime();
  Reflect.deleteProperty(globalThis, "localStorage");
});

/** Everything the app is saying to the user right now. */
const toasts = () => useApp.getState().toasts.map((shown) => shown.text);

/** Waits for a read nobody awaits (`loadPulls` starts one and returns). */
const settle = () => new Promise((done) => setTimeout(done, 0));

describe("one read per answer", () => {
  test("two askers of the same list share one read", async () => {
    answerWith({ "_rustcode/pulls": () => listAnswer([wirePull()]) });
    await Promise.all([readPullList(CWD, "open"), readPullList(CWD, "open")]);
    expect(asked).toHaveLength(1);
    expect(pullList(CWD, "open")?.pulls.map((pull) => pull.number)).toEqual([12]);
  });

  test("a row's prefetch and the click are the same read", async () => {
    answerWith({ "_rustcode/pulls/view": () => detailAnswer() });
    const prefetch = readPullDetail(CWD, 12);
    const click = readPullDetail(CWD, 12);
    await Promise.all([prefetch, click]);
    expect(asked).toHaveLength(1);
    expect(pullDetail(CWD, 12)?.body).toBe("The body.");
  });

  test("an answer younger than 30 seconds is not read again", async () => {
    answerWith({ "_rustcode/pulls/view": () => detailAnswer() });
    await readPullDetail(CWD, 12);
    setSystemTime(new Date(NOW + PULL_FRESH_MS - 1));
    await readPullDetail(CWD, 12);
    expect(asked).toHaveLength(1);
    // A reload the user asked for goes out regardless.
    await readPullDetail(CWD, 12, { force: true });
    expect(asked).toHaveLength(2);
  });

  test("an older answer is painted and a read behind it replaces it", async () => {
    answerWith({ "_rustcode/pulls/view": () => detailAnswer() });
    await readPullDetail(CWD, 12);
    setSystemTime(new Date(NOW + PULL_FRESH_MS + 1));
    answerWith({ "_rustcode/pulls/view": () => detailAnswer({ title: "A newer title" }) });

    // What the panel draws while the read runs is the answer already held.
    const refresh = readPullDetail(CWD, 12);
    expect(pullDetail(CWD, 12)?.title).toBe("feat(desktop): a branch");
    await refresh;
    expect(pullDetail(CWD, 12)?.title).toBe("A newer title");
    expect(asked).toHaveLength(2);
  });

  test("an answer to a superseded read never lands", async () => {
    const slow = deferred<Json>();
    let call = 0;
    answerWith({
      "_rustcode/pulls": () => {
        call += 1;
        return call === 1 ? slow.promise : listAnswer([wirePull({ number: 13 })]);
      },
    });
    const superseded = readPullList(CWD, "open");
    const newer = readPullList(CWD, "open", { force: true });
    await newer;
    expect(pullList(CWD, "open")?.pulls.map((pull) => pull.number)).toEqual([13]);

    slow.settle(listAnswer([wirePull({ number: 11 })]));
    await superseded;
    expect(pullList(CWD, "open")?.pulls.map((pull) => pull.number)).toEqual([13]);
  });

  test("a refresh that fails keeps the answer and reports the error", async () => {
    answerWith({ "_rustcode/pulls/diff": () => ({ diff: "diff --git a/x b/x" }) });
    await readPullDiff(CWD, 12);
    answerWith({ "_rustcode/pulls/diff": () => failure("timed_out", "No answer.") });
    const answer = await loadPullDiff(CWD, 12, { force: true });
    expect(pullDiff(CWD, 12)).toBe("diff --git a/x b/x");
    expect(answer.error).toBeUndefined();
    expect(pullDiffError(CWD, 12)?.code).toBe("timed_out");

    // With nothing to draw, the same failure is the panel's error.
    await readPullDiff(CWD, 99);
    expect(pullDiff(CWD, 99)).toBeUndefined();
    expect(pullDiffError(CWD, 99)?.code).toBe("timed_out");
  });

  test("a failing read's error goes when a later read answers", async () => {
    answerWith({ "_rustcode/pulls": () => failure("gh_missing", "gh is not installed") });
    await readPullList(CWD, "open");
    expect(pullList(CWD, "open")).toBeUndefined();
    expect(pullListError(CWD, "open")?.code).toBe("gh_missing");

    answerWith({ "_rustcode/pulls": () => listAnswer([wirePull()]) });
    await readPullList(CWD, "open", { force: true });
    expect(pullListError(CWD, "open")).toBeUndefined();
    expect(pullList(CWD, "open")?.pulls).toHaveLength(1);

    answerWith({ "_rustcode/pulls/view": () => failure("not_connected", "GitHub is not connected") });
    await readPullDetail(CWD, 12);
    expect(pullDetailError(CWD, 12)?.code).toBe("not_connected");
    expect(pullSummary(CWD, 12)?.number).toBe(12);
  });
});

describe("a list is a row of another list", () => {
  test("a detail's header comes from the list row that carried it", async () => {
    answerWith({
      "_rustcode/pulls": () =>
        listAnswer([wirePull({ checks: { passed: 5, failed: 0, pending: 0, skipped: 0 } })]),
    });
    await readPullList(CWD, "open");
    const slow = deferred<Json>();
    answerWith({ "_rustcode/pulls/view": () => slow.promise });
    const opening = readPullDetail(CWD, 12);

    // The panel has no detail yet, so it draws the row it already had.
    expect(pullDetail(CWD, 12)).toBeUndefined();
    expect(pullSummary(CWD, 12)?.title).toBe("feat(desktop): a branch");
    expect(pullSummary(CWD, 12)?.checks.passed).toBe(5);

    slow.settle(detailAnswer({ checks: { passed: 6, failed: 0, pending: 0, skipped: 0 } }));
    await opening;
    expect(pullSummary(CWD, 12)?.checks.passed).toBe(6);
  });

  test("a cut-short wider list paints a state, and the state's own read still runs", async () => {
    answerWith({
      "_rustcode/pulls": () =>
        listAnswer(
          [wirePull({ number: 20, state: "merged" }), wirePull({ number: 21, state: "open" })],
          true,
        ),
    });
    await readPullList(CWD, "all", { limit: 50 });
    expect(asked).toHaveLength(1);

    // It paints what the app holds, without claiming to be the whole state:
    // the "All" list was cut short, so filtering it may hide open pulls.
    const stand = pullList(CWD, "open", PULL_LIMIT);
    expect(stand?.pulls.map((pull) => pull.number)).toEqual([21]);
    expect(stand?.complete).toBe(false);
    expect(stand?.hasMore).toBe(true);

    answerWith({
      "_rustcode/pulls": () =>
        listAnswer([wirePull({ number: 21, state: "open" }), wirePull({ number: 22, state: "open" })]),
    });
    await readPullList(CWD, "open");
    expect(asked).toHaveLength(2);
    expect(asked[1]?.params).toEqual({ cwd: CWD, state: "open", limit: PULL_LIMIT });
    expect(pullList(CWD, "open", PULL_LIMIT)?.pulls.map((pull) => pull.number)).toEqual([21, 22]);
  });

  test("an exhaustive wider list answers a state with no read of its own", async () => {
    answerWith({
      "_rustcode/pulls": () =>
        listAnswer([wirePull({ number: 20, state: "merged" }), wirePull({ number: 21, state: "open" })]),
    });
    await readPullList(CWD, "all", { limit: 50 });
    const answer = pullList(CWD, "open", PULL_LIMIT);
    expect(answer?.pulls.map((pull) => pull.number)).toEqual([21]);
    expect(answer?.complete).toBe(true);
    expect(answer?.hasMore).toBe(false);

    await readPullList(CWD, "open");
    expect(asked).toHaveLength(1);
    // "All" still draws the wider answer, not the filtered one.
    expect(pullList(CWD, "all", PULL_LIMIT)?.pulls.map((pull) => pull.number)).toEqual([20, 21]);
  });

  test("hasMore says there are more rows than are shown, narrowed or filtered", async () => {
    answerWith({
      "_rustcode/pulls": () =>
        listAnswer([wirePull({ number: 1 }), wirePull({ number: 2 }), wirePull({ number: 3 })]),
    });
    await readPullList(CWD, "open", { limit: 3 });
    const narrowed = pullList(CWD, "open", 2);
    expect(narrowed?.pulls.map((pull) => pull.number)).toEqual([1, 2]);
    expect(narrowed?.hasMore).toBe(true);
    expect(narrowed?.complete).toBe(true);
    // The whole list is shown at its own limit, so nothing is hidden.
    expect(pullList(CWD, "open", 3)?.hasMore).toBe(false);
  });

  test("a state's own list outranks a newer filtered one", async () => {
    answerWith({ "_rustcode/pulls": () => listAnswer([wirePull({ number: 30 })]) });
    await readPullList(CWD, "open");
    setSystemTime(new Date(NOW + 1000));
    answerWith({
      "_rustcode/pulls": () =>
        listAnswer([wirePull({ number: 31 }), wirePull({ number: 32, state: "merged" })]),
    });
    await readPullList(CWD, "all", { limit: 50 });
    expect(asked).toHaveLength(2);

    const answer = pullList(CWD, "open", PULL_LIMIT);
    expect(answer?.pulls.map((pull) => pull.number)).toEqual([30]);
    expect(answer?.complete).toBe(true);
  });

  test("a written answer outranks the list row it came from", async () => {
    answerWith({ "_rustcode/pulls": () => listAnswer([wirePull({ title: "From the list" })]) });
    await readPullList(CWD, "open");
    answerWith({ "_rustcode/pulls/view": () => detailAnswer({ title: "From the detail" }) });
    await readPullDetail(CWD, 12);
    expect(pullSummary(CWD, 12)?.title).toBe("From the detail");
  });
});

describe("answers saved on the device", () => {
  test("what is saved is trimmed to the device's bounds", async () => {
    fakeStorage();
    // Nine projects read in turn: only the newest eight are kept.
    for (let project = 0; project < 9; project += 1) {
      setSystemTime(new Date(NOW + project));
      answerWith({ "_rustcode/pulls": () => listAnswer([wirePull()]) });
      for (const state of ["open", "closed", "merged", "all"] as const) {
        await readPullList(`/repo/${project}`, state);
      }
    }
    const payload = savedPayload(NOW + 100);
    expect(new Set(payload.lists.map((list) => list.cwd)).size).toBe(SAVED_PROJECTS);
    expect(payload.lists.some((list) => list.cwd === "/repo/0")).toBe(false);

    // Details: only the newest forty are kept.
    answerWith({ "_rustcode/pulls/view": (params) => detailAnswer({ number: Number(params.number) }) });
    for (let number = 1; number <= SAVED_DETAILS + 5; number += 1) {
      setSystemTime(new Date(NOW + number));
      await readPullDetail("/detail-repo", number);
    }
    const details = savedPayload(NOW + 200).details;
    expect(details).toHaveLength(SAVED_DETAILS);
    expect(details.some((detail) => detail.number === 1)).toBe(false);
    expect(details.some((detail) => detail.number === SAVED_DETAILS + 5)).toBe(true);

    expect(JSON.stringify(savedPayload(NOW + 200)).length).toBeLessThan(SAVED_TOTAL_BYTES);
  });

  test("an answer over 256 KB, or older than a week, is not saved", async () => {
    fakeStorage();
    answerWith({ "_rustcode/pulls": () => listAnswer([wirePull({ title: "x".repeat(300_000) })]) });
    await readPullList("/big", "open");
    answerWith({ "_rustcode/pulls": () => listAnswer([wirePull()]) });
    await readPullList("/small", "open");

    // A week later nothing is worth keeping.
    setSystemTime(new Date(NOW + SAVED_MAX_AGE_MS + 1));
    expect(savedPayload().lists).toHaveLength(0);

    // Within the week only the answer that fits is kept.
    setSystemTime(new Date(NOW + 1000));
    expect(savedPayload().lists.map((list) => list.cwd)).toEqual(["/small"]);
  });

  test("a saved answer paints at once and is read again behind", async () => {
    const disk = fakeStorage();
    disk.write({
      version: SAVED_VERSION,
      lists: [
        {
          cwd: CWD,
          state: "open",
          limit: PULL_LIMIT,
          readAt: NOW - 60_000,
          hasMore: false,
          pulls: [wirePull()],
        },
      ],
      details: [],
    });
    answerWith({ "_rustcode/pulls": () => listAnswer([wirePull({ number: 14 })]) });

    // The first look paints the saved list, then the engine's answer replaces it.
    const painting = pullList(CWD, "open");
    expect(painting?.pulls.map((pull) => pull.number)).toEqual([12]);
    expect(painting?.saved).toBe(true);
    await readPullList(CWD, "open");
    expect(asked).toHaveLength(1);
    expect(pullList(CWD, "open")?.pulls.map((pull) => pull.number)).toEqual([14]);
    expect(pullList(CWD, "open")?.saved).toBe(false);
  });

  test("a saved detail's header and body survive a reload", async () => {
    const disk = fakeStorage();
    answerWith({ "_rustcode/pulls/view": () => detailAnswer() });
    await readPullDetail(CWD, 12);
    savePullQueries();

    const saved = JSON.parse(disk.read() ?? "{}");
    expect(saved.version).toBe(SAVED_VERSION);
    // Timestamps are saved as the numbers they were read as, not as strings.
    expect(saved.details[0].pull.updatedAt).toBe(NOW - 20 * 60_000);
    expect(saved.details[0].pull.activity[0].at).toBe(NOW - 40 * 60_000);

    // A fresh process: the answers come off the device on the first look.
    clearPullQueries();
    hydratePullQueries();
    expect(pullDetail(CWD, 12)?.title).toBe("feat(desktop): a branch");
    expect(pullDetail(CWD, 12)?.activity[0]?.at).toBe(NOW - 40 * 60_000);
    expect(pullSummary(CWD, 12)?.number).toBe(12);
  });

  test("a file of another version, or a week old, is ignored", () => {
    const disk = fakeStorage();
    disk.write({
      version: SAVED_VERSION + 1,
      lists: [
        { cwd: CWD, state: "open", limit: PULL_LIMIT, readAt: NOW, hasMore: false, pulls: [wirePull()] },
      ],
      details: [],
    });
    hydratePullQueries();
    expect(pullList(CWD, "open")).toBeUndefined();

    clearPullQueries();
    disk.write({
      version: SAVED_VERSION,
      lists: [
        {
          cwd: CWD,
          state: "open",
          limit: PULL_LIMIT,
          readAt: NOW - SAVED_MAX_AGE_MS - 1,
          hasMore: false,
          pulls: [wirePull()],
        },
      ],
      details: [],
    });
    hydratePullQueries();
    expect(pullList(CWD, "open")).toBeUndefined();
  });

  test("a window without storage still reads and paints", async () => {
    Reflect.deleteProperty(globalThis, "localStorage");
    answerWith({ "_rustcode/pulls": () => listAnswer([wirePull()]) });
    await readPullList(CWD, "open");
    expect(() => savePullQueries()).not.toThrow();
    expect(pullList(CWD, "open")?.pulls).toHaveLength(1);
  });
});

describe("a detail is read ahead of the click", () => {
  test("a prefetched detail is the one the click draws, with no second read", async () => {
    answerWith({ "_rustcode/pulls/view": () => detailAnswer() });
    await readPullDetail(CWD, 12);
    expect(asked).toHaveLength(1);
    expect(asked[0]?.params).toEqual({ cwd: CWD, number: 12 });
    // The click asks for the same key and finds it fresh.
    await readPullDetail(CWD, 12);
    expect(asked).toHaveLength(1);
    expect(pullDetail(CWD, 12)?.files[0]?.path).toBe("apps/desktop/src/a.ts");
  });

  test("a diff read ahead keeps the review view waiting for nothing", async () => {
    answerWith({ "_rustcode/pulls/diff": () => ({ diff: "diff --git a/b.ts b/b.ts\n+++ b/b.ts\n" }) });
    await readPullDiff(CWD, 12);
    const answer = await loadPullDiff(CWD, 12);
    expect(asked).toHaveLength(1);
    expect(answer.diff).toContain("diff --git a/b.ts b/b.ts");
  });
});

describe("what a failed read says", () => {
  test("a reload the user asked for says so, and keeps the rows", async () => {
    answerWith({ "_rustcode/pulls": () => listAnswer([wirePull()]) });
    await readPullList(CWD, "open");
    answerWith({ "_rustcode/pulls": () => failure("timed_out", "No answer.") });
    await readPullList(CWD, "open", { force: true });

    expect(pullList(CWD, "open")?.pulls).toHaveLength(1);
    expect(toasts()).toEqual(["GitHub did not answer in time: No answer."]);
  });

  test("a refresh behind the panel fails quietly", async () => {
    answerWith({ "_rustcode/pulls": () => listAnswer([wirePull()]) });
    await readPullList(CWD, "open");
    setSystemTime(new Date(NOW + PULL_FRESH_MS + 1));
    answerWith({ "_rustcode/pulls": () => failure("timed_out", "No answer.") });
    await readPullList(CWD, "open");

    expect(pullListError(CWD, "open")?.code).toBe("timed_out");
    expect(pullList(CWD, "open")?.pulls).toHaveLength(1);
    expect(toasts()).toEqual([]);
  });
});

describe("the sidebar's own read retries once", () => {
  test("a failed read is remembered for its TTL", async () => {
    useApp.setState({ agent: { pullRequests: true } as unknown as AgentInfo });
    answerWith({ "_rustcode/pulls": () => failure("gh_missing", "gh is not installed") });
    loadPulls(CWD);
    await settle();
    expect(asked).toHaveLength(1);

    // The list re-renders and its effect asks again: no second `gh` process.
    loadPulls(CWD);
    expect(asked).toHaveLength(1);

    // Once the failure is as old as an answer would be, it tries again.
    setSystemTime(new Date(NOW + PULLS_TTL + 1));
    answerWith({ "_rustcode/pulls": () => listAnswer([wirePull()]) });
    loadPulls(CWD);
    await settle();
    expect(asked).toHaveLength(2);
    expect(asked[1]?.params).toEqual({ cwd: CWD, state: "all", limit: 50 });
  });
});

describe("the first paint off the device", () => {
  test("hydrating from render tells nobody: it is not a read landing", () => {
    const disk = fakeStorage();
    disk.write({
      version: SAVED_VERSION,
      lists: [
        {
          cwd: CWD,
          state: "open",
          limit: PULL_LIMIT,
          readAt: NOW - 60_000,
          hasMore: false,
          pulls: [wirePull()],
        },
      ],
      details: [],
    });
    const version = pullQueriesVersion();
    expect(pullList(CWD, "open")?.pulls).toHaveLength(1);
    expect(pullQueriesVersion()).toBe(version);
  });

  test("a saved detail is not a commit to post a review at", async () => {
    const disk = fakeStorage();
    disk.write({
      version: SAVED_VERSION,
      lists: [],
      details: [{ cwd: CWD, number: 12, readAt: NOW - 60_000, pull: detailPull() }],
    });
    hydratePullQueries();
    expect(pullDetail(CWD, 12)?.body).toBe("The body.");
    expect(pullDetailLive(CWD, 12)).toBe(false);

    // A read this session makes it live.
    answerWith({ "_rustcode/pulls/view": () => detailAnswer() });
    await readPullDetail(CWD, 12, { force: true });
    expect(pullDetailLive(CWD, 12)).toBe(true);
  });

  test("the file written is the same JSON the payload stringifies to", async () => {
    const disk = fakeStorage();
    answerWith({ "_rustcode/pulls": () => listAnswer([wirePull()]) });
    await readPullList(CWD, "open");
    answerWith({ "_rustcode/pulls/view": () => detailAnswer() });
    await readPullDetail(CWD, 12);

    savePullQueries(NOW);
    expect(disk.read()).toBe(JSON.stringify(savedPayload(NOW)));
  });
});

describe("a rest on a row that is already answered", () => {
  test("a fresh or in-flight read starts nothing", async () => {
    answerWith({ "_rustcode/pulls/view": () => detailAnswer() });
    await readPullDetail(CWD, 12);
    const version = pullQueriesVersion();

    // Fresh: the rest reads nothing and re-renders nothing.
    prefetchPullDetail(CWD, 12);
    expect(asked).toHaveLength(1);
    expect(pullQueriesVersion()).toBe(version);

    // In flight: the rest joins that read instead of adding a process.
    const slow = deferred<Json>();
    answerWith({ "_rustcode/pulls/view": () => slow.promise });
    const opening = readPullDetail(CWD, 13);
    prefetchPullDetail(CWD, 13);
    expect(asked).toHaveLength(2);
    slow.settle(detailAnswer({ number: 13 }));
    await opening;
    expect(asked).toHaveLength(2);
  });
});
