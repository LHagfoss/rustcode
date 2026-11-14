import { describe, expect, test } from "bun:test";
import {
  activeTab,
  foldersOf,
  MAX_FOLDERS,
  MAX_TABS_PER_FOLDER,
  TERMINAL_CONTEXT_LINES,
  type TerminalTabs,
  tabsOf,
  terminalContext,
  terminalContextLabel,
  withActionTab,
  withExited,
  withFolder,
  withNewTab,
  withOpenedFolder,
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

describe("terminal folders", () => {
  /** Opens each folder in turn, as visiting threads with the panel open does. */
  const visit = (s: TerminalTabs, ...dirs: string[]) => {
    const evicted: string[] = [];
    for (const dir of dirs) {
      const next = withOpenedFolder(s, dir);
      s = next.state;
      if (next.evicted) evicted.push(next.evicted.cwd);
    }
    return { s, evicted };
  };

  test("the cap is four folders", () => {
    expect(MAX_FOLDERS).toBe(4);
  });

  test("a fifth folder closes the least recently used one, once, and the state settles", () => {
    const { s, evicted } = visit(empty, "/a", "/b", "/c", "/d", "/a", "/e");
    expect(evicted).toEqual(["/b"]);
    expect(foldersOf(s).sort()).toEqual(["/a", "/c", "/d", "/e"]);
    expect(s.active["/b"]).toBeUndefined();
    // Showing the same folder again changes nothing: no update, so no effect runs again.
    expect(withOpenedFolder(s, "/e").state).toBe(s);
    expect(withOpenedFolder(s, "/e").evicted).toBeUndefined();
  });

  test("an evicted folder is gone from the state, so nothing mounts it again", () => {
    const { s } = visit(empty, "/a", "/b", "/c", "/d", "/e");
    expect(tabsOf(s, "/a")).toEqual([]);
    expect(foldersOf(s)).toHaveLength(MAX_FOLDERS);
    // Revisiting it starts a fresh shell and closes the next oldest.
    const again = withOpenedFolder(s, "/a");
    expect(again.evicted?.cwd).toBe("/b");
    expect(foldersOf(again.state)).toHaveLength(MAX_FOLDERS);
  });

  test("many folders never leave more than the cap", () => {
    const dirs = Array.from({ length: 20 }, (_, i) => `/w${i}`);
    const { s, evicted } = visit(empty, ...dirs);
    expect(foldersOf(s)).toEqual(dirs.slice(-MAX_FOLDERS));
    expect(evicted).toEqual(dirs.slice(0, -MAX_FOLDERS));
  });

  test("a folder running a project action is kept over an idle one", () => {
    let s = visit(empty, "/a", "/b", "/c", "/d").s;
    s = withActionTab(s, "/a", { id: "dev", name: "Dev server" }, "dev").state;
    const next = withOpenedFolder(s, "/e");
    expect(next.evicted).toEqual({ cwd: "/b", busy: false });
    expect(tabsOf(next.state, "/a").map((t) => t.id)).toContain("dev");
  });

  test("an action tab whose shell exited does not keep its folder", () => {
    let s = visit(empty, "/a", "/b", "/c", "/d").s;
    s = withActionTab(s, "/a", { id: "dev", name: "Dev server" }, "dev").state;
    s = withExited(s, "dev", true);
    expect(withOpenedFolder(s, "/e").evicted).toEqual({ cwd: "/a", busy: false });
  });

  test("when every other folder runs an action, the oldest closes and says so", () => {
    let s = visit(empty, "/a", "/b", "/c", "/d").s;
    for (const dir of ["/a", "/b", "/c", "/d"]) s = withActionTab(s, dir, { id: "dev", name: "Dev" }).state;
    expect(withOpenedFolder(s, "/e").evicted).toEqual({ cwd: "/a", busy: true });
  });

  test("an action run in a fifth folder with the panel hidden closes nothing", () => {
    let s = visit(empty, "/a", "/b", "/c", "/d").s;
    s = withActionTab(s, "/e", { id: "dev", name: "Dev" }, "dev").state;
    expect(foldersOf(s)).toHaveLength(5);
    // Opening the panel on that folder then evicts once, and never the action's own folder.
    const next = withOpenedFolder(s, "/e");
    expect(next.evicted?.cwd).toBe("/a");
    expect(tabsOf(next.state, "/e").map((t) => t.id)).toEqual(["dev"]);
    expect(withOpenedFolder(next.state, "/e").state).toBe(next.state);
  });
});
