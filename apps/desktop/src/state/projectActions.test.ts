import { describe, expect, test } from "bun:test";
import {
  actionId,
  MAX_ACTIONS,
  onTerminalCommand,
  projectKey,
  queueTerminalCommand,
  sanitizeProjectActions,
  takeTerminalCommands,
  withProjectActions,
  worktreeHooks,
} from "./projectActions";

describe("project actions", () => {
  test("a worktree files its actions under its repository", () => {
    expect(projectKey("/repo/.worktrees/feat", "/repo")).toBe("/repo");
    expect(projectKey("/Users/me/app/")).toBe("/Users/me/app");
  });

  test("ids come from the name and stay unique", () => {
    expect(actionId("Dev server", [])).toBe("dev-server");
    expect(actionId("Dev server", [{ id: "dev-server", name: "x", command: "y" }])).toBe("dev-server-2");
    expect(actionId("  ✨  ", [])).toBe("action");
  });

  test("a stored value drops malformed entries and duplicate ids", () => {
    const raw = {
      "/app": [
        { id: "dev", name: "Dev", command: "bun run dev" },
        { id: "dev", name: "Again", command: "bun run dev" },
        { id: "blank", name: " ", command: "ls" },
        { name: "No id", command: "ls" },
      ],
      "/empty": [],
      "/broken": "nope",
    };
    expect(sanitizeProjectActions(raw)).toEqual({
      "/app": [{ id: "dev", name: "Dev", command: "bun run dev" }],
    });
    expect(sanitizeProjectActions(null)).toEqual({});
    expect(sanitizeProjectActions([1, 2])).toEqual({});
  });

  test("the editor's rows replace the list, keep ids and skip empty rows", () => {
    const before = { "/app": [{ id: "dev", name: "Dev", command: "bun run dev" }] };
    const after = withProjectActions(before, "/app", [
      { id: "dev", name: " Dev ", command: " bun run dev --open " },
      { name: "Check", command: "bun run check" },
      { name: "", command: "" },
    ]);
    expect(after["/app"]).toEqual([
      { id: "dev", name: "Dev", command: "bun run dev --open" },
      { id: "check", name: "Check", command: "bun run check" },
    ]);
    expect(withProjectActions(before, "/app", [])).toEqual({});
  });

  test("a list never grows past the cap", () => {
    const rows = Array.from({ length: MAX_ACTIONS + 5 }, (_, i) => ({ name: `A${i}`, command: "true" }));
    expect(withProjectActions({}, "/app", rows)["/app"]).toHaveLength(MAX_ACTIONS);
  });

  test("a command waits for its folder's terminal and is taken once", () => {
    const heard: string[] = [];
    const stop = onTerminalCommand((cwd) => heard.push(cwd));
    queueTerminalCommand("/app", "bun run dev");
    queueTerminalCommand("/app", "bun run check");
    queueTerminalCommand("/other", "ls");
    stop();
    expect(heard).toEqual(["/app", "/app", "/other"]);
    expect(takeTerminalCommands("/app")).toEqual(["bun run dev", "bun run check"]);
    expect(takeTerminalCommands("/app")).toEqual([]);
    expect(takeTerminalCommands("/other")).toEqual(["ls"]);
  });
});

describe("worktree hooks", () => {
  test("only the first action of each kind keeps its hook flag", () => {
    const map = withProjectActions({}, "/p", [
      { name: "Install", command: "bun install", onCreate: true, wait: true },
      { name: "Also", command: "npm i", onCreate: true, onSettle: true },
      { name: "Clean", command: "rm -rf dist", onSettle: true },
    ]);
    const list = map["/p"] ?? [];
    expect(list.map((a) => [a.onCreate, a.wait, a.onSettle])).toEqual([
      [true, true, undefined],
      [undefined, undefined, true],
      [undefined, undefined, undefined],
    ]);
    expect(sanitizeProjectActions(map)).toEqual(map);
  });

  test("wait needs run-on-create", () => {
    const map = withProjectActions({}, "/p", [{ name: "Dev", command: "bun run dev", wait: true }]);
    expect(map["/p"]?.[0]?.wait).toBeUndefined();
  });

  test("session/new carries the create and settle hooks", () => {
    const list = withProjectActions({}, "/p", [
      { name: "Install", command: "bun install", onCreate: true },
      { name: "Clean", command: "rm -rf dist", onSettle: true },
    ])["/p"];
    expect(worktreeHooks(list ?? [])).toEqual({
      create: { name: "Install", command: "bun install", wait: false },
      settle: { name: "Clean", command: "rm -rf dist" },
    });
    expect(worktreeHooks([{ id: "dev", name: "Dev", command: "bun run dev" }])).toBeUndefined();
  });
});
