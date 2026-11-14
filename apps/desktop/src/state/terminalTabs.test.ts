import { describe, expect, test } from "bun:test";
import {
  activeTab,
  MAX_TABS_PER_FOLDER,
  TERMINAL_CONTEXT_LINES,
  type TerminalTabs,
  tabsOf,
  terminalContext,
  terminalContextLabel,
  withActionTab,
  withFolder,
  withNewTab,
  withoutTab,
  withStep,
  withTitle,
} from "./terminalTabs";

const empty: TerminalTabs = { tabs: [], active: {} };

describe("terminal tabs", () => {
  test("a folder opens with one shell, and + adds the next free number", () => {
    let s = withFolder(empty, "/a");
    expect(tabsOf(s, "/a").map((t) => t.title)).toEqual(["Shell 1"]);
    expect(withFolder(s, "/a")).toBe(s);
    s = withNewTab(s, "/a", {}, "t2").state;
    expect(tabsOf(s, "/a").map((t) => t.title)).toEqual(["Shell 1", "Shell 2"]);
    expect(activeTab(s, "/a")?.id).toBe("t2");
  });

  test("folders keep their own tabs and active tab", () => {
    let s = withNewTab(empty, "/a", {}, "a1").state;
    s = withNewTab(s, "/b", {}, "b1").state;
    s = withNewTab(s, "/a", {}, "a2").state;
    expect(tabsOf(s, "/b").map((t) => t.id)).toEqual(["b1"]);
    expect(activeTab(s, "/a")?.id).toBe("a2");
    expect(activeTab(s, "/b")?.id).toBe("b1");
  });

  test("closing the shown tab shows the one after it, else the one before", () => {
    let s = withNewTab(empty, "/a", {}, "1").state;
    s = withNewTab(s, "/a", {}, "2").state;
    s = withNewTab(s, "/a", {}, "3").state;
    s = { ...s, active: { "/a": "2" } };
    s = withoutTab(s, "2");
    expect(activeTab(s, "/a")?.id).toBe("3");
    s = withoutTab(s, "3");
    expect(activeTab(s, "/a")?.id).toBe("1");
    s = withoutTab(s, "1");
    expect(tabsOf(s, "/a")).toEqual([]);
    expect(s.active["/a"]).toBeUndefined();
  });

  test("a project action reuses the tab it opened", () => {
    let s = withFolder(empty, "/a");
    const first = withActionTab(s, "/a", { id: "dev", name: "Dev server" }, "dev-tab");
    s = first.state;
    expect(first.tab.title).toBe("Dev server");
    s = withNewTab(s, "/a", {}, "other").state;
    const again = withActionTab(s, "/a", { id: "dev", name: "Dev server" }, "unused");
    expect(again.tab.id).toBe("dev-tab");
    expect(activeTab(again.state, "/a")?.id).toBe("dev-tab");
    expect(tabsOf(again.state, "/a")).toHaveLength(3);
  });

  test("the shell's title renames a shell tab, never an action's", () => {
    let s = withNewTab(empty, "/a", {}, "sh").state;
    s = withActionTab(s, "/a", { id: "dev", name: "Dev server" }, "act").state;
    s = withTitle(withTitle(s, "sh", "bun run dev"), "act", "zsh");
    expect(tabsOf(s, "/a").map((t) => t.title)).toEqual(["bun run dev", "Dev server"]);
  });

  test("stepping wraps round", () => {
    let s = withNewTab(empty, "/a", {}, "1").state;
    s = withNewTab(s, "/a", {}, "2").state;
    expect(activeTab(withStep(s, "/a", 1), "/a")?.id).toBe("1");
    expect(activeTab(withStep(s, "/a", -1), "/a")?.id).toBe("1");
  });

  test("a folder holds at most a few shells", () => {
    let s = empty;
    for (let i = 0; i < MAX_TABS_PER_FOLDER + 2; i++) s = withNewTab(s, "/a", {}, `t${i}`).state;
    expect(tabsOf(s, "/a")).toHaveLength(MAX_TABS_PER_FOLDER);
  });
});

describe("terminal output as message context", () => {
  test("a selection becomes a fenced block naming its tab, with a chip label", () => {
    const text = terminalContext("error: boom  \n  at main.ts:3\n\n", "bun run dev");
    expect(text).toBe("Terminal output (bun run dev), 2 lines:\n```\nerror: boom\n  at main.ts:3\n```");
    expect(terminalContextLabel(text)).toBe("Terminal · 2 lines");
    expect(terminalContextLabel("just a paste")).toBeUndefined();
  });

  test("a long selection keeps its last lines and says so", () => {
    const lines = Array.from({ length: TERMINAL_CONTEXT_LINES + 5 }, (_, i) => `line ${i}`);
    const text = terminalContext(lines.join("\n"), "zsh");
    expect(
      text.startsWith(`Terminal output (zsh), ${TERMINAL_CONTEXT_LINES} lines, the last of ${lines.length}:`),
    ).toBe(true);
    expect(text).not.toContain("line 4\n");
    expect(text).toContain(`line ${lines.length - 1}`);
  });

  test("output holding a fence gets a longer one", () => {
    expect(terminalContext("```js\nx\n```", "zsh")).toContain("````\n```js");
  });
});
