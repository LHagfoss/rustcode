import { beforeEach, describe, expect, test } from "bun:test";
import type { SessionSummary } from "../acp/types";
import { PREF, readPref, writePref } from "../state/sidebarPrefs";
import { shelfKey } from "../state/sidebarRows";
import { useApp } from "../state/store";
import { matches, sidebarActions, threadHaystack } from "./paletteActions";

const thread = (over: Partial<SessionSummary> = {}): SessionSummary =>
  ({
    id: "t1",
    cwd: "/code/rustcode",
    title: "Fix the composer",
    updatedAt: 1,
    ...over,
  }) as SessionSummary;

const ids = () => sidebarActions().map((a) => a.id);

describe("finding a thread", () => {
  test("by its title and its project", () => {
    expect(matches(threadHaystack(thread(), undefined), "composer rustcode")).toBe(true);
  });

  test("by its branch", () => {
    const s = thread({ worktree: { path: "/wt/a", branch: "feat/slim-sidebar", repo: "/code/rustcode" } });
    expect(matches(threadHaystack(s, undefined), "slim-sidebar")).toBe(true);
    expect(matches(threadHaystack(thread(), undefined), "slim-sidebar")).toBe(false);
  });

  test("by its pull request's number and title", () => {
    const pull = { number: 3259, title: "Slim the sidebar" } as never;
    const hay = threadHaystack(thread(), pull);
    expect(matches(hay, "#3259")).toBe(true);
    expect(matches(hay, "slim sidebar")).toBe(true);
    expect(matches(hay, "#9999")).toBe(false);
  });

  test("every word has to match", () => {
    expect(matches(threadHaystack(thread(), undefined), "composer zzz")).toBe(false);
  });
});

describe("the actions the sidebar's buttons and filter left for ⌘K", () => {
  beforeEach(() => {
    useApp.setState({ selectedId: null, sessions: [], pinned: [], snoozed: {}, projectCwd: "" });
    writePref(PREF.groupByProject, false);
    writePref(PREF.workingSection, true);
    writePref(PREF.onlyProject, false);
  });

  test("add project, pull requests, usage, statistics and the view options are all there", () => {
    expect(ids()).toEqual(
      expect.arrayContaining([
        "add-project",
        "pulls",
        "usage",
        "statistics",
        "sidebar-group",
        "sidebar-working",
        "sidebar-only",
        "settled-open",
      ]),
    );
  });

  test("pin and snooze are offered for the open thread only", () => {
    expect(ids().some((id) => id === "pin" || id.startsWith("snooze"))).toBe(false);
    useApp.setState({ selectedId: "t1", sessions: [thread()] });
    const actions = sidebarActions();
    expect(actions.find((a) => a.id === "pin")?.label).toBe("Pin this thread");
    expect(actions.some((a) => a.id === "snooze-custom")).toBe(true);
    expect(actions.some((a) => a.id === "unsnooze")).toBe(false);
  });

  test("a pinned thread offers Unpin, a snoozed one offers to wake it", () => {
    useApp.setState({ selectedId: "t1", sessions: [thread()], pinned: ["t1"], snoozed: { t1: 9e12 } });
    const actions = sidebarActions();
    expect(actions.find((a) => a.id === "pin")?.label).toBe("Unpin this thread");
    expect(actions.some((a) => a.id === "unsnooze")).toBe(true);
  });

  test("group by project and the Working section toggle the sidebar's stored options", () => {
    sidebarActions()
      .find((a) => a.id === "sidebar-group")
      ?.run();
    expect(readPref(PREF.groupByProject, false)).toBe(true);
    // Folders already separate the work: the Working option is off while grouped.
    expect(sidebarActions().find((a) => a.id === "sidebar-working")?.disabled).toBeDefined();
    sidebarActions()
      .find((a) => a.id === "sidebar-group")
      ?.run();
    sidebarActions()
      .find((a) => a.id === "sidebar-working")
      ?.run();
    expect(readPref(PREF.workingSection, true)).toBe(false);
  });

  test("filtering to one project needs a project", () => {
    expect(sidebarActions().find((a) => a.id === "sidebar-only")?.disabled).toBeDefined();
    useApp.setState({ projectCwd: "/code/rustcode" });
    const only = sidebarActions().find((a) => a.id === "sidebar-only");
    expect(only?.disabled).toBeUndefined();
    expect(only?.label).toBe("Sidebar: Only rustcode");
    only?.run();
    expect(readPref(PREF.onlyProject, false)).toBe(true);
  });

  test("open settled threads is disabled with nothing settled, and opens the shelf otherwise", () => {
    expect(sidebarActions().find((a) => a.id === "settled-open")?.disabled).toBe("No settled threads");
    useApp.setState({ sessions: [thread({ status: "done" })] });
    const open = sidebarActions().find((a) => a.id === "settled-open");
    expect(open?.disabled).toBeUndefined();
    open?.run();
    expect(readPref(shelfKey("settled"), false)).toBe(true);
  });
});
