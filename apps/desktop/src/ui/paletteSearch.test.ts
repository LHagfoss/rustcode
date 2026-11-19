import { beforeEach, describe, expect, test } from "bun:test";
import type { SessionSummary } from "../acp/types";
import { useApp } from "../state/store";
import { GROUP_LIMIT, runnableCommands, searchGroups } from "./paletteSearch";

const thread = (over: Partial<SessionSummary> = {}): SessionSummary =>
  ({ id: "t1", cwd: "/code/rustcode", title: "Fix the composer", updatedAt: 1, ...over }) as SessionSummary;

const inGroup = (query: string, group: string) => searchGroups(query).filter((a) => a.group === group);

beforeEach(() => {
  useApp.setState({ sessions: [], selectedId: null, settings: null, settingsLanding: null });
});

describe("⌘K's search groups", () => {
  test("nothing without a query", () => {
    expect(searchGroups("")).toEqual([]);
    expect(searchGroups("   ")).toEqual([]);
  });

  test("a Settings row is found by the words it is looked up by", () => {
    const rows = inGroup("report issue", "Settings");
    expect(rows.map((r) => r.label)).toContain("Debug report");
    expect(rows.find((r) => r.label === "Debug report")?.detail).toBe("Settings › Diagnostics");
  });

  test("picking a Settings row opens its page and lands on it", () => {
    inGroup("report issue", "Settings")
      .find((r) => r.label === "Debug report")
      ?.run();
    expect(useApp.getState().settings).toBe("diagnostics");
    expect(useApp.getState().settingsLanding).toEqual({ section: "diagnostics", title: "Debug report" });
  });

  test("each group is capped", () => {
    expect(inGroup("a", "Settings").length).toBeLessThanOrEqual(GROUP_LIMIT.settings);
    expect(inGroup("e", "Shortcuts").length).toBeLessThanOrEqual(GROUP_LIMIT.shortcuts);
  });

  test("a project of the threads offers a new thread in it", () => {
    useApp.setState({ sessions: [thread(), thread({ id: "t2", cwd: "/code/other" })] });
    const projects = inGroup("rustcode", "Projects");
    expect(projects.map((p) => p.label)).toEqual(["New thread in rustcode"]);
  });

  test("a shortcut shows its keys and is not one the palette lists as an action", () => {
    const zoom = inGroup("ui scale", "Shortcuts");
    expect(zoom.map((z) => z.label)).toContain("Increase UI scale");
    expect(zoom.every((z) => !!z.keys)).toBe(true);
    expect(searchGroups("toggle sidebar", ["Toggle sidebar"]).some((a) => a.group === "Shortcuts")).toBe(
      false,
    );
  });

  test("a runnable shortcut is not repeated as its Keyboard row", () => {
    expect(inGroup("increase ui scale", "Settings").some((r) => r.detail === "Settings › Keyboard")).toBe(
      false,
    );
  });

  test("the composer's keys and the approval digits are not runnable from ⌘K", () => {
    const ids = runnableCommands().map((c) => c.id);
    expect(ids).not.toContain("composer-send");
    expect(ids).not.toContain("answer-approval");
    expect(ids).not.toContain("command-palette");
    expect(ids).toContain("zoom-in");
  });
});
