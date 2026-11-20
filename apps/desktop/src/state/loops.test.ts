// Loops (issues 3892, 3937): a schedule in words, a loop's state and last result, and the review list.

import { describe, expect, test } from "bun:test";
import {
  agoText,
  cronInWords,
  cronOfForm,
  cronProblem,
  defaultForm,
  formOfCron,
  formProblem,
  isJobRunThread,
  type Loop,
  type LoopRun,
  lastResult,
  loopIdsByThread,
  loopRunThreads,
  loopState,
  loopThreadNames,
  nextRunText,
  parseLoops,
  pauseReason,
  reviewItems,
  runDurationText,
  runTimestampText,
  saveParams,
  scheduleInWords,
  timeZoneChoices,
  webhookOf,
  withRunAccount,
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
    webhook: false,
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
    expect(cronInWords("0 9 * * MON-FRI")).toBe("Weekdays at 09:00");
    expect(cronInWords("0 9 * * MON,WED,FRI")).toBe("Mon, Wed and Fri at 09:00");
    expect(cronInWords("0 9 * * 0")).toBe("Every Sunday at 09:00");
    expect(cronInWords("0 9 * * 7")).toBe("Every Sunday at 09:00");
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
  test("opens an edit on the loop's own schedule", () => {
    expect(formOfCron("0 */4 * * *")).toMatchObject({ repeat: "hours", everyHours: 4 });
    expect(formOfCron(" 0  9 * * 1-5 ")).toMatchObject({
      repeat: "days",
      days: [1, 2, 3, 4, 5],
      time: "09:00",
    });
    expect(formOfCron("15 3 * * *")).toMatchObject({
      repeat: "days",
      days: [0, 1, 2, 3, 4, 5, 6],
      time: "03:15",
    });
    expect(formOfCron("0 9 * * MON,WED")).toMatchObject({ repeat: "days", days: [1, 3] });
    expect(formOfCron("30 8 15 * *")).toMatchObject({ repeat: "monthly", monthDay: 15, time: "08:30" });
    expect(formOfCron("0 9-17 * * *")).toMatchObject({ repeat: "cron", cron: "0 9-17 * * *" });
    expect(formOfCron("0 */5 * * *")).toMatchObject({ repeat: "cron" });
  });

  test("builds a cron with weekday names, which every cron reads alike", () => {
    const base = defaultForm();
    expect(cronOfForm(base)).toBe("0 */4 * * *");
    expect(cronOfForm({ ...base, everyHours: 1 })).toBe("0 * * * *");
    expect(cronOfForm({ ...base, repeat: "days", time: "08:30", days: [1, 2, 3, 4, 5] })).toBe(
      "30 8 * * MON-FRI",
    );
    expect(cronOfForm({ ...base, repeat: "days", days: [5, 1, 3] })).toBe("0 9 * * MON,WED,FRI");
    expect(cronOfForm({ ...base, repeat: "days", days: [0, 1, 2, 3, 4, 5, 6] })).toBe("0 9 * * *");
    expect(cronOfForm({ ...base, repeat: "monthly", monthDay: 3, time: "07:05" })).toBe("5 7 3 * *");
    for (const cron of ["30 8 * * MON-FRI", "0 9 * * MON,WED,FRI", "5 7 3 * *", "15 */6 * * *"])
      expect(cronOfForm(formOfCron(cron))).toBe(cron);
  });

  test("says what the form still needs", () => {
    const now = new Date(2026, 10, 17, 12, 0).getTime();
    const base = defaultForm(now);
    expect(formProblem(base, now)).toBeNull();
    expect(formProblem({ ...base, repeat: "days", days: [] }, now)).toContain("day");
    expect(formProblem({ ...base, repeat: "once", onceAt: "" }, now)).toContain("date");
    expect(formProblem({ ...base, repeat: "once", onceAt: "2026-11-17T11:00" }, now)).toContain("passed");
    expect(formProblem({ ...base, repeat: "once", onceAt: "2026-11-18T09:00" }, now)).toBeNull();
    expect(formProblem({ ...base, repeat: "cron", cron: "0 9 * *" }, now)).toContain("five fields");
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

  test("a loop paused by a signed-out login names it after one run", () => {
    const signedOut = loop({
      paused: true,
      runs: [
        run("r1", "failed", 1, {
          error:
            "Claude Code is not signed in on its default login (~/.claude). Sign in, then resume the job.",
          signedOut: true,
        }),
      ],
    });
    expect(pauseReason(signedOut)).toBe(
      "Paused: Claude Code is not signed in on its default login (~/.claude). Sign in, then resume the job.",
    );
    expect(loopState(signedOut)).toBe("failing");
    expect(
      parseLoops({ loops: [{ id: "l", runs: [{ id: "r", state: "failed", signedOut: true }] }] }).loops[0]
        ?.runs[0]?.signedOut,
    ).toBe(true);
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

describe("loop follow-ups (issue 3937)", () => {
  test("a one-time loop saves an `at` schedule, and a loop's own thread is sent along", () => {
    const once = new Date("2026-11-17T14:00:00Z");
    const params = saveParams(
      {
        name: "Once",
        workspace: "/code/app",
        prompt: "Check",
        cron: "0 9 * * *",
        once,
        readOnly: true,
        threadId: "t-9",
      },
      "Europe/Oslo",
    );
    expect(params.schedule).toEqual({ kind: "at", at: "2026-11-17T14:00:00.000Z" });
    expect(params.threadId).toBe("t-9");
  });

  test("a script is saved trimmed, a blank one is left out, and a silent run says so", () => {
    const base = { name: "CI", workspace: "/code/app", prompt: "Check", cron: "0 9 * * *", readOnly: true };
    expect(saveParams({ ...base, script: "  gh run list  " }, "Europe/Oslo").script).toBe("gh run list");
    expect("script" in saveParams({ ...base, script: "   " }, "Europe/Oslo")).toBe(false);
    const silent = loop({ runs: [run("r1", "succeeded", 1, { quiet: true, silent: true })] });
    expect(lastResult(silent, NOW)).toEqual({ text: "Script printed nothing 1 h ago", tone: "ok" });
    const parsed = parseLoops({
      daemonRunning: true,
      loops: [
        { id: "l", name: "L", script: "gh run list", runs: [{ id: "r", state: "succeeded", silent: true }] },
      ],
    });
    expect(parsed.loops[0]?.script).toBe("gh run list");
    expect(parsed.loops[0]?.runs[0]?.silent).toBe(true);
  });

  test("a quiet run says it found nothing, and is parsed from the list", () => {
    const quiet = loop({ runs: [run("r1", "succeeded", 1, { threadId: "t-1", quiet: true })] });
    expect(lastResult(quiet, NOW)).toEqual({ text: "Nothing to report 1 h ago", tone: "ok" });
    const parsed = parseLoops({
      daemonRunning: true,
      loops: [
        { id: "l", name: "L", threadId: "t-own", runs: [{ id: "r", state: "succeeded", quiet: true }] },
      ],
    });
    expect(parsed.loops[0]?.threadId).toBe("t-own");
    expect(parsed.loops[0]?.runs[0]?.quiet).toBe(true);
  });

  test("a webhook-only loop saves a manual schedule and says it waits for its webhook", () => {
    const base = { name: "Triage", workspace: "/code/app", prompt: "Triage", cron: "", readOnly: true };
    const params = saveParams({ ...base, manual: true, webhook: true }, "Europe/Oslo");
    expect(params.schedule).toEqual({ kind: "manual" });
    expect(params.webhook).toBe(true);
    expect("rotateWebhook" in params).toBe(false);
    expect(saveParams({ ...base, webhook: true, rotateWebhook: true }, "Europe/Oslo").rotateWebhook).toBe(
      true,
    );
    // Turning the webhook off sends nothing, which drops it; a stale rotate is not sent either.
    const off = saveParams({ ...base, webhook: false, rotateWebhook: true }, "Europe/Oslo");
    expect("webhook" in off || "rotateWebhook" in off).toBe(false);

    const parsed = parseLoops({
      daemonRunning: true,
      loops: [{ id: "l", name: "L", schedule: { kind: "manual" }, webhook: true, nextDueAt: null, runs: [] }],
    });
    const manual = parsed.loops[0];
    expect(manual?.schedule).toEqual({ kind: "manual" });
    expect(manual && scheduleInWords(manual.schedule)).toBe("No schedule");
    expect(manual && nextRunText(manual, NOW)).toBe("On webhook");
    expect(nextRunText(loop({ schedule: { kind: "manual" }, nextDueAt: undefined }), NOW)).toBe(
      "Run by hand",
    );
  });

  test("a save shows the webhook URL only when it made one", () => {
    expect(webhookOf({ loop: {} })).toBeNull();
    expect(
      webhookOf({ loop: {}, webhook: { url: "https://h.example/hooks/l/s", public: true, secret: "s" } }),
    ).toEqual({ url: "https://h.example/hooks/l/s", public: true, secret: "s" });
    expect(webhookOf({ webhook: { url: null, localUrl: null, secret: "s" } })).toBeNull();
  });

  test("every thread a loop ran in carries its name for the sidebar badge", () => {
    const loops = [
      loop({ name: "CI", runs: [run("r1", "succeeded", 1, { threadId: "t-1" }), run("r2", "cancelled", 2)] }),
      loop({ id: "loop-2", name: "Triage", threadId: "t-own", runs: [] }),
    ];
    expect(loopThreadNames(loops)).toEqual({ "t-1": "CI", "t-own": "Triage" });
  });

  test("a run's thread leads back to its job by id", () => {
    const loops = [
      loop({ runs: [run("r1", "succeeded", 1, { threadId: "t-1" }), run("r2", "cancelled", 2)] }),
      loop({ id: "loop-2", threadId: "t-own", runs: [] }),
    ];
    expect(loopIdsByThread(loops)).toEqual({ "t-1": "loop-1", "t-own": "loop-2" });
  });

  test("a run's fresh thread stays out of the thread list, a loop's own thread stays in", () => {
    const loops = [
      loop({ name: "CI", runs: [run("r1", "succeeded", 1, { threadId: "t-1" })] }),
      loop({
        id: "loop-2",
        name: "Triage",
        threadId: "t-own",
        runs: [run("r2", "succeeded", 1, { threadId: "t-own" })],
      }),
    ];
    const runs = loopRunThreads(loops);
    expect(runs).toEqual({ "t-1": true });
    expect(isJobRunThread({ id: "t-1" }, runs)).toBe(true);
    expect(isJobRunThread({ id: "t-own" }, runs)).toBe(false);
    // The engine marks every run thread it starts, whether or not the history still lists the run.
    expect(isJobRunThread({ id: "t-old", scheduledJob: "CI" }, {})).toBe(true);
    expect(isJobRunThread({ id: "t-user" }, {})).toBe(false);
  });
});

describe("a job's account", () => {
  const draft = { name: "Watch", workspace: "/repo", prompt: "Check", cron: "0 * * * *", readOnly: true };

  test("a job that names none runs on the account a new thread would get", () => {
    expect(withRunAccount(draft, "claude-code", "paral-2")).toMatchObject({
      harness: "claude-code",
      account: "paral-2",
    });
    expect(saveParams(withRunAccount(draft, "claude-code", "paral-2"))).toMatchObject({
      harness: "claude-code",
      account: "paral-2",
    });
  });

  test("a job's own account, its own thread, or no named account is left as it is", () => {
    const own = { ...draft, harness: "claude-code", account: "personal-1" };
    expect(withRunAccount(own, "claude-code", "paral-2")).toBe(own);
    const thread = { ...draft, threadId: "t-own" };
    expect(withRunAccount(thread, "claude-code", "paral-2")).toBe(thread);
    expect(withRunAccount(draft, "pi", null)).toBe(draft);
  });
});

describe("hub Loops (#3957)", () => {
  const base = { name: "CI", workspace: "/home/me/app", prompt: "Check", cron: "0 9 * * *", readOnly: true };

  test("a Loop for a member saves its machine, no script, and no account of this computer's", () => {
    const params = saveParams({ ...base, machine: " laptop ", script: "gh run list" }, "Europe/Oslo");
    expect(params.machine).toBe("laptop");
    expect("script" in params).toBe(false);
    const draft = withRunAccount({ ...base, machine: "laptop" }, "codex", "work");
    expect(draft.account).toBeUndefined();
  });

  test("its runs say where they went, and a held run waits for the machine", () => {
    const parsed = parseLoops({
      loops: [
        {
          id: "l",
          name: "L",
          machine: "laptop",
          runs: [{ id: "r", state: "succeeded", machine: "laptop", queued: true }],
        },
      ],
    });
    expect(parsed.loops[0]?.machine).toBe("laptop");
    expect(parsed.loops[0]?.runs[0]?.queued).toBe(true);
    const held = loop({ runs: [run("r1", "succeeded", 1, { machine: "laptop", queued: true })] });
    expect(lastResult(held, NOW)).toEqual({ text: "Waiting for laptop (queued 1 h ago)", tone: "muted" });
    const started = loop({ runs: [run("r1", "succeeded", 1, { machine: "laptop", threadId: "t-1" })] });
    expect(lastResult(started, NOW)).toEqual({ text: "Started on laptop 1 h ago", tone: "ok" });
  });

  test("a non-quiet run verdict is parsed and shown as the last result", () => {
    const parsed = parseLoops({
      daemonRunning: true,
      loops: [
        {
          id: "loop-ci",
          name: "CI watch",
          runs: [
            {
              id: "r1",
              state: "succeeded",
              scheduledAt: new Date(NOW - 5 * 60_000).toISOString(),
              finishedAt: new Date(NOW - 5 * 60_000).toISOString(),
              threadId: "t-1",
              verdict: "CI broke on main: test_x fails",
            },
          ],
        },
      ],
    });
    expect(parsed.loops[0]?.runs[0]?.verdict).toBe("CI broke on main: test_x fails");
    expect(lastResult(parsed.loops[0]!, NOW)).toEqual({
      text: "CI broke on main: test_x fails · 5 min ago",
      tone: "ok",
    });
  });

  test("run duration, timestamp, and timezone helpers format cleanly", () => {
    expect(runDurationText(1000, 1400)).toBe("<1s");
    expect(runDurationText(1000, 43_000)).toBe("42s");
    expect(runDurationText(0, 135_000)).toBe("2m 15s");
    expect(runDurationText(0, 3_600_000)).toBe("1h");
    expect(runDurationText(0, 3_840_000)).toBe("1h 4m");
    expect(runDurationText(undefined, 1000)).toBeNull();
    expect(runDurationText(2000, 1000)).toBeNull();

    const today = new Date(2026, 10, 17, 14, 0).getTime();
    const sameDay = new Date(2026, 10, 17, 8, 5).getTime();
    const yesterday = new Date(2026, 10, 16, 9, 30).getTime();
    expect(runTimestampText(sameDay, today)).toBe("Today 08:05");
    expect(runTimestampText(yesterday, today)).toBe("Yesterday 09:30");

    const zones = timeZoneChoices("Europe/Oslo");
    expect(zones).toContain("Europe/Oslo");
    expect(zones).toContain("UTC");
  });
});
