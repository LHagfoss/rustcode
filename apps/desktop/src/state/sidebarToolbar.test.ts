import { describe, expect, test } from "bun:test";
import { settleAfterDays, settleAfterMs, sidebarSort } from "./sidebarPrefs";
import { type OrderKey, sortSection, threadMatches } from "./sidebarRows";
import { idleSettled } from "./thread";

const DAY = 86_400_000;

describe("the sidebar's sort", () => {
  const rows: { id: string; key: OrderKey }[] = [
    { id: "old-running", key: { backAt: 1, running: true, title: "zeta" } },
    { id: "new", key: { backAt: 30, title: "Alpha" } },
    { id: "mid-unseen", key: { backAt: 20, unseen: true, title: "beta" } },
  ];
  const order = (sort: "activity" | "recent" | "title", section: "threads" | "pinned" = "threads") =>
    sortSection(section, rows, (r) => r.key, sort).map((r) => r.id);

  test("activity leads with what runs, then what came back unseen", () => {
    expect(order("activity")).toEqual(["old-running", "mid-unseen", "new"]);
  });

  test("last updated ignores what runs: only when each thread last changed", () => {
    expect(order("recent")).toEqual(["new", "mid-unseen", "old-running"]);
  });

  test("title is A to Z, ignoring case", () => {
    expect(order("title")).toEqual(["new", "mid-unseen", "old-running"]);
  });

  test("a stored sort is read leniently", () => {
    expect(sidebarSort("title")).toBe("title");
    expect(sidebarSort("nonsense")).toBe("activity");
    expect(sidebarSort(undefined)).toBe("activity");
  });
});

describe("the sidebar's search", () => {
  const thread = {
    title: "Fix the flaky CI job",
    cwd: "/home/me/code/rustcode",
    worktree: { branch: "fix/ci-flake", repo: "/home/me/code/rustcode" },
  };

  test("every word must be in the title, the project or the branch", () => {
    expect(threadMatches(thread, "")).toBe(true);
    expect(threadMatches(thread, "flaky")).toBe(true);
    expect(threadMatches(thread, "RUSTCODE ci")).toBe(true);
    expect(threadMatches(thread, "ci-flake")).toBe(true);
    expect(threadMatches(thread, "flaky website")).toBe(false);
  });

  test("only the folder's name counts, not its parents", () => {
    expect(threadMatches(thread, "home")).toBe(false);
  });
});

describe("settling idle threads", () => {
  test("after the chosen days, and never at 0", () => {
    const now = 100 * DAY;
    expect(idleSettled(now - 2 * DAY, now)).toBe(false);
    expect(idleSettled(now - 3 * DAY, now)).toBe(true);
    expect(idleSettled(now - 2 * DAY, now, settleAfterMs(1))).toBe(true);
    expect(idleSettled(now - 90 * DAY, now, settleAfterMs(0))).toBe(false);
  });

  test("a stored day count is one of the choices, else the default", () => {
    expect(settleAfterDays(7)).toBe(7);
    expect(settleAfterDays(0)).toBe(0);
    expect(settleAfterDays(5)).toBe(3);
    expect(settleAfterDays("7")).toBe(3);
  });
});
