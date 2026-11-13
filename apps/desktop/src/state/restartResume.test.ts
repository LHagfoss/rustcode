import { describe, expect, test } from "bun:test";
import { markStopped, pruneResumed, RESUME_LOOP_MS, type ResumeBook, resumeVerdict } from "./restartResume";
import { RESUME_TEXT } from "./store";
import { isContinueMessage, wakeLine } from "./thread";

const known = new Set(["a", "b"]);
const empty: ResumeBook = { stopped: [], resumedAt: {} };
const NOW = 1_000_000_000;

describe("which interrupted threads a restart resumes", () => {
  test("a thread the restart cut off continues", () => {
    expect(resumeVerdict("a", known, empty, NOW)).toBe("resume");
  });

  test("a thread the app cannot see is not resumed", () => {
    expect(resumeVerdict("gone", known, empty, NOW)).toBe("unknown");
  });

  test("a turn the user stopped stays stopped", () => {
    expect(resumeVerdict("a", known, { ...empty, stopped: ["a"] }, NOW)).toBe("stopped");
    expect(resumeVerdict("b", known, { ...empty, stopped: ["a"] }, NOW)).toBe("resume");
  });

  test("a resume cut off again soon is not repeated, one long ago is", () => {
    const recent = { ...empty, resumedAt: { a: NOW - 60_000 } };
    expect(resumeVerdict("a", known, recent, NOW)).toBe("again");
    const old = { ...empty, resumedAt: { a: NOW - RESUME_LOOP_MS - 1 } };
    expect(resumeVerdict("a", known, old, NOW)).toBe("resume");
  });

  test("the record forgets entries that no longer guard anything", () => {
    const kept = pruneResumed({ a: NOW - 1_000, b: NOW - RESUME_LOOP_MS - 1 }, NOW);
    expect(Object.keys(kept)).toEqual(["a"]);
  });

  test("a stop is listed once", () => {
    expect(markStopped(markStopped([], "a"), "a")).toEqual(["a"]);
  });
});

describe("the restart resume's transcript row", () => {
  test("reads as a 'Resumed after restart' divider, never as the user's message", () => {
    expect(isContinueMessage(RESUME_TEXT)).toBe(true);
    expect(wakeLine(RESUME_TEXT)).toBe("Resumed after restart.");
  });

  test("still tells the harness what happened", () => {
    expect(RESUME_TEXT).toContain("Continue where you left off.");
  });
});
