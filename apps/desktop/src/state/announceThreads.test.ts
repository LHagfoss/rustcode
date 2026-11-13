import { describe, expect, test } from "bun:test";
import {
  momentOf,
  spokenTitle,
  type ThreadMoment,
  threadAnnouncement,
  windowStatus,
  windowTitle,
} from "./announceThreads";
import { emptyThread, type ThreadState } from "./thread";

const idle: ThreadMoment = { running: false, failed: false, continues: false, loading: false };
const at = (over: Partial<ThreadMoment>): ThreadMoment => ({ ...idle, ...over });
const say = (prev: ThreadMoment | undefined, next: ThreadMoment, active = true) =>
  threadAnnouncement("Fix tests", prev, next, active);

describe("threadAnnouncement", () => {
  test("the open thread says it starts and finishes, politely", () => {
    expect(say(idle, at({ running: true }))).toEqual({ text: "Fix tests: working", assertive: false });
    expect(say(at({ running: true }), idle)).toEqual({ text: "Fix tests: finished", assertive: false });
  });

  test("streaming inside a running turn says nothing", () => {
    expect(say(at({ running: true }), at({ running: true }))).toBeNull();
  });

  test("a question and an approval interrupt, once per request", () => {
    const asks = at({ running: true, request: { key: "7", question: true } });
    expect(say(at({ running: true }), asks)).toEqual({ text: "Fix tests: asks a question", assertive: true });
    expect(say(asks, asks)).toBeNull();
    const approval = at({ running: true, request: { key: "8", question: false } });
    expect(say(asks, approval)).toEqual({ text: "Fix tests: needs approval", assertive: true });
  });

  test("a failure interrupts once, and is not also a finish", () => {
    const failed = at({ failed: true });
    expect(say(at({ running: true }), failed)).toEqual({ text: "Fix tests: failed", assertive: true });
    expect(say(failed, failed)).toBeNull();
  });

  test("a background thread only says what needs the user", () => {
    expect(say(idle, at({ running: true }), false)).toBeNull();
    expect(say(at({ running: true }), idle, false)).toBeNull();
    expect(say(idle, at({ request: { key: "1", question: false } }), false)?.assertive).toBe(true);
    expect(say(at({ running: true }), at({ failed: true }), false)?.text).toBe("Fix tests: failed");
  });

  test("a turn that goes on by itself, or the app's own, is not a finish", () => {
    expect(say(at({ running: true }), at({ continues: true }))).toBeNull();
    expect(say(at({ running: true, quiet: true }), idle)).toBeNull();
    expect(say(idle, at({ running: true, quiet: true }))).toBeNull();
  });

  test("a first sighting and a replay say nothing", () => {
    expect(say(undefined, at({ request: { key: "1", question: true } }))).toBeNull();
    expect(say(idle, at({ failed: true, loading: true }))).toBeNull();
  });
});

describe("momentOf", () => {
  const thread = (over: Partial<ThreadState>): ThreadState => ({ ...emptyThread("t", "/repo"), ...over });

  test("reads the oldest request and a failure", () => {
    const request = (rpcId: number, at: number, question?: boolean) =>
      ({
        rpcId,
        at,
        question: question ? { text: "?" } : undefined,
      }) as unknown as ThreadState["permissions"][number];
    const moment = momentOf(thread({ permissions: [request(2, 20), request(1, 10, true)] }), undefined);
    expect(moment.request).toEqual({ key: "1", question: true });
    expect(momentOf(thread({ error: "boom" }), undefined).failed).toBe(true);
    expect(momentOf(undefined, 5).failed).toBe(true);
    expect(momentOf(thread({ running: true, error: "old" }), undefined).failed).toBe(false);
  });
});

describe("window title", () => {
  test("names the thread and its status, and handles untitled threads", () => {
    expect(windowTitle(spokenTitle(undefined, "  "), windowStatus(at({ running: true }), undefined))).toBe(
      "New thread — Working — RustCode",
    );
    expect(windowTitle(spokenTitle("Renamed", "Old"), windowStatus(idle, "Review"))).toBe(
      "Renamed — Review — RustCode",
    );
    expect(windowTitle("Fix tests", windowStatus(idle, undefined))).toBe("Fix tests — RustCode");
    expect(windowTitle(undefined)).toBe("RustCode");
    expect(windowStatus(at({ request: { key: "1", question: false } }), undefined)).toBe("Needs approval");
  });
});
