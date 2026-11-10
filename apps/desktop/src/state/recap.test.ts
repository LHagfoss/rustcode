import { describe, expect, test } from "bun:test";
import type { SessionSummary } from "../acp/types";
import type { InboxItem } from "./inbox";
import { awayDue, parseRecapHours, RECAP_HOURS_DEFAULT, recapEmpty, recapOf } from "./recap";

const NOW = 1_700_000_000_000;
const HOUR = 3_600_000;

function item(overrides: Partial<InboxItem> = {}): InboxItem {
  return {
    key: "one:request:1",
    kind: "approval",
    threadId: "one",
    title: "Fix the composer",
    project: "/code/app",
    detail: "Wants to bash",
    at: NOW - HOUR,
    pending: 1,
    number: 1,
    ...overrides,
  };
}

const sessions: SessionSummary[] = [
  { id: "one", title: "Fix the composer", cwd: "/code/app" },
  { id: "two", title: "Pricing page", cwd: "/code/website" },
];

describe("the away time", () => {
  test("a first run never earns a recap, and a long enough break does", () => {
    expect(awayDue(undefined, NOW, 4)).toBe(false);
    expect(awayDue(NOW - 4 * HOUR, NOW, 4)).toBe(true);
    expect(awayDue(NOW - 4 * HOUR + 1, NOW, 4)).toBe(false);
    expect(awayDue(NOW - HOUR, NOW, 1)).toBe(true);
  });

  test("the stored threshold stays one of the offered times", () => {
    expect(parseRecapHours(null)).toBe(RECAP_HOURS_DEFAULT);
    expect(parseRecapHours("8")).toBe(8);
    expect(parseRecapHours("3")).toBe(RECAP_HOURS_DEFAULT);
    expect(parseRecapHours("nonsense")).toBe(RECAP_HOURS_DEFAULT);
  });
});

describe("the recap", () => {
  const since = NOW - 2 * HOUR;

  test("needs you holds every waiting row, and failures and done hold what happened since", () => {
    const recap = recapOf({
      since,
      items: [
        item({ key: "old", at: NOW - 3 * HOUR }),
        item({ key: "failed", kind: "failed", at: NOW - HOUR, threadId: "two" }),
        item({ key: "stale-failure", kind: "failed", at: NOW - 3 * HOUR, threadId: "one" }),
      ],
      finished: { one: NOW - 30 * 60_000, two: NOW - 5 * HOUR },
      sessions,
      threads: {},
    });
    expect(recap.needs.map((row) => row.key)).toEqual(["old"]);
    expect(recap.failed.map((row) => row.threadId)).toEqual(["two"]);
    expect(recap.done.map((row) => [row.threadId, row.title])).toEqual([["one", "Fix the composer"]]);
    expect(recapEmpty(recap)).toBe(false);
    expect(recapEmpty({ since, needs: [], failed: [], done: [] })).toBe(true);
  });

  test("a loaded thread's fresher title wins over the list's", () => {
    const recap = recapOf({
      since,
      items: [item({ kind: "failed", at: NOW - HOUR, threadId: "two" })],
      finished: {},
      sessions,
      threads: { two: { title: "Pricing page, renamed" } },
    });
    expect(recap.failed[0]!.title).toBe("Pricing page, renamed");
  });

  test("a thread that slept through its finish is not in the card yet", () => {
    const recap = recapOf({
      since,
      items: [],
      finished: { one: NOW - 30 * 60_000, two: NOW - HOUR },
      sessions,
      threads: {},
      snoozed: { one: NOW + HOUR },
    });
    expect(recap.done.map((row) => row.threadId)).toEqual(["two"]);
  });
});
