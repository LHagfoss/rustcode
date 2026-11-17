// Loops (issue 3892): a schedule in words, a loop's state and last result, and the review list.

import { describe, expect, test } from "bun:test";
import {
  agoText,
  cronInWords,
  cronProblem,
  type Loop,
  type LoopRun,
  lastResult,
  loopState,
  nextRunText,
  parseLoops,
  pauseReason,
  presetOf,
  reviewItems,
  saveParams,
  scheduleInWords,
} from "./loops";

const NOW = Date.parse("2026-11-17T12:00:00Z");
const HOUR = 3_600_000;

function run(id: string, state: LoopRun["state"], hoursAgo: number, extra: Partial<LoopRun> = {}): LoopRun {
  const at = NOW - hoursAgo * HOUR;
  return { id, state, scheduledAt: at, startedAt: at, finishedAt: at, ...extra };
}

function loop(extra: Partial<Loop> = {}): Loop {
  return {
    id: "loop-1",
    name: "CI watch",
    paused: false,
    schedule: { kind: "cron", expression: "0 */4 * * *" },
    workspace: "/code/app",
    prompt: "Check CI on main",
    readOnly: true,
    nextDueAt: NOW + 2 * HOUR,
    runs: [],
    ...extra,
  };
}

describe("cronInWords", () => {
  test("says the shapes people write by hand", () => {
    expect(cronInWords("*/15 * * * *")).toBe("Every 15 minutes");
    expect(cronInWords("0 * * * *")).toBe("Every hour");
    expect(cronInWords("30 * * * *")).toBe("Every hour at :30");
    expect(cronInWords("0 */4 * * *")).toBe("Every 4 hours");
    expect(cronInWords("0 9 * * *")).toBe("Every day at 09:00");
    expect(cronInWords("30 8 * * 1-5")).toBe("Weekdays at 08:30");
    expect(cronInWords("0 10 * * 0,6")).toBe("Weekends at 10:00");
    expect(cronInWords("0 9 * * 1")).toBe("Every Monday at 09:00");
    expect(cronInWords("0 9 1 * *")).toBe("Monthly on the 1st at 09:00");
    expect(cronInWords("0 9 22 * *")).toBe("Monthly on the 22nd at 09:00");
  });

  test("leaves what it cannot say to the cron itself", () => {
    expect(cronInWords("0 9 * 3 *")).toBeNull();
    expect(cronInWords("0 9-17 * * *")).toBeNull();
    expect(cronInWords("0 9 * *")).toBeNull();
    expect(scheduleInWords({ kind: "cron", expression: "0 9-17 * * *" })).toBe("Cron 0 9-17 * * *");
  });

  test("says the daemon's other schedule kinds too", () => {
    expect(scheduleInWords({ kind: "daily", hour: 7, minute: 5 })).toBe("Every day at 07:05");
    expect(scheduleInWords({ kind: "monthly", day: 3, hour: 9, minute: 0 })).toBe(
      "Monthly on the 3rd at 09:00",
    );
  });
});

describe("the schedule field", () => {
  test("opens an edit on the loop's own preset", () => {
    expect(presetOf("0 */4 * * *")).toBe("4h");
    expect(presetOf(" 0  9 * * 1-5 ")).toBe("weekdays");
    expect(presetOf("15 3 * * *")).toBe("custom");
  });

  test("checks a cron before the engine sees it", () => {
    expect(cronProblem("0 9 * * *")).toBeNull();
    expect(cronProblem("")).toContain("Write");
    expect(cronProblem("0 9 * *")).toContain("five fields");
    expect(cronProblem("0 9 * * $(rm)")).toContain("not a cron");
  });

  test("saves a cron in the user's zone and leaves out empty agent fields", () => {
    const params = saveParams(
      {
        name: " CI ",
        workspace: "/code/app",
        prompt: " Check CI ",
        cron: " 0  */4 * * * ",
        harness: "codex",
        model: " ",
        readOnly: true,
      },
      "Europe/Oslo",
    );
    expect(params).toEqual({
      name: "CI",
      workspace: "/code/app",
      prompt: "Check CI",
      schedule: { kind: "cron", expression: "0 */4 * * *", timezone: "Europe/Oslo" },
      readOnly: true,
      harness: "codex",
    });
  });
});

describe("a loop's state", () => {
  test("a loop paused by three failed runs says why", () => {
    const failing = loop({
      paused: true,
      runs: [
        run("r3", "failed", 1, { error: "Codex is turned off" }),
        run("r2", "failed", 5),
        run("r1", "ambiguous", 9),
      ],
    });
    expect(pauseReason(failing)).toBe("Paused after 3 failed runs: Codex is turned off");
    expect(loopState(failing)).toBe("failing");
  });

  test("a loop the user paused is only paused", () => {
    const paused = loop({ paused: true, runs: [run("r2", "failed", 1), run("r1", "succeeded", 5)] });
    expect(pauseReason(paused)).toBeNull();
    expect(loopState(paused)).toBe("paused");
    expect(nextRunText(paused, NOW)).toBe("Paused");
  });

  test("a run going shows as running", () => {
    expect(loopState(loop({ runs: [run("r1", "running", 0, { finishedAt: undefined })] }))).toBe("running");
    expect(lastResult(loop({ runs: [run("r1", "running", 0, { finishedAt: undefined })] }), NOW).text).toBe(
      "Running now",
    );
  });

  test("the last result says how and when", () => {
    expect(lastResult(loop(), NOW)).toEqual({ text: "No runs yet", tone: "muted" });
    expect(lastResult(loop({ runs: [run("r1", "succeeded", 2)] }), NOW)).toEqual({
      text: "Finished 2 h ago",
      tone: "ok",
    });
    expect(lastResult(loop({ runs: [run("r1", "failed", 1, { error: "boom" })] }), NOW).text).toBe(
      "Failed 1 h ago: boom",
    );
    expect(
      lastResult(loop({ runs: [run("r1", "cancelled", 1, { skipped: "RustCode was not open" })] }), NOW).text,
    ).toBe("Skipped 1 h ago: RustCode was not open");
  });

  test("the next run counts down", () => {
    expect(nextRunText(loop(), NOW)).toBe("in 2 h");
    expect(nextRunText(loop({ nextDueAt: NOW + 10 * 60_000 }), NOW)).toBe("in 10 min");
    expect(nextRunText(loop({ nextDueAt: NOW - 1 }), NOW)).toBe("Due now");
    expect(agoText(NOW - 30_000, NOW)).toBe("just now");
    expect(agoText(NOW - 3 * 24 * HOUR, NOW)).toBe("3 d ago");
  });
});

describe("reviewItems", () => {
  const loops = [
    loop({
      runs: [
        run("open", "succeeded", 1, { threadId: "t-open" }),
        run("archived", "succeeded", 2, { threadId: "t-archived" }),
        run("failed", "failed", 3, { error: "boom" }),
        run("skipped", "cancelled", 4, { skipped: "RustCode was not open" }),
        run("old", "failed", 24 * 8),
      ],
    }),
  ];
  const isOpen = (id: string) => id === "t-open";

  test("lists failed runs and open threads from the last week, newest first", () => {
    const items = reviewItems(loops, isOpen, new Set(), NOW);
    expect(items.map((item) => [item.run.id, item.kind])).toEqual([
      ["open", "open"],
      ["failed", "failed"],
    ]);
  });

  test("a reviewed run leaves the list", () => {
    expect(reviewItems(loops, isOpen, new Set(["open"]), NOW).map((item) => item.run.id)).toEqual(["failed"]);
  });
});

describe("parseLoops", () => {
  test("reads the engine's answer and fills what it left out", () => {
    const snapshot = parseLoops({
      daemonRunning: true,
      loops: [
        {
          id: "loop-1",
          name: "CI watch",
          paused: false,
          schedule: {
            kind: "cron",
            expression: "0 */4 * * *",
            timezone: "Europe/Oslo",
            misfire_policy: "run_once",
          },
          workspace: "/code/app",
          prompt: "Check CI",
          harness: "codex",
          readOnly: false,
          nextDueAt: "2026-11-17T14:00:00Z",
          runs: [{ id: "r1", state: "succeeded", scheduledAt: "2026-11-17T10:00:00Z", threadId: "t-1" }],
        },
        { name: "no id" },
      ],
    });
    expect(snapshot.daemonRunning).toBe(true);
    expect(snapshot.loops).toHaveLength(1);
    const [first] = snapshot.loops;
    expect(first?.schedule).toEqual({ kind: "cron", expression: "0 */4 * * *", timezone: "Europe/Oslo" });
    expect(first?.readOnly).toBe(false);
    expect(first?.nextDueAt).toBe(Date.parse("2026-11-17T14:00:00Z"));
    expect(first?.runs[0]?.threadId).toBe("t-1");
    expect(parseLoops(null)).toEqual({ daemonRunning: false, loops: [] });
  });
});
