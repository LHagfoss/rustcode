import { describe, expect, test } from "bun:test";
import type { SlashCommand } from "../acp/types";
import {
  COMMANDS_PER_HARNESS_MAX,
  HARNESS_COMMANDS_MAX,
  type HarnessCommands,
  menuCommands,
  parseHarnessCommands,
  rememberCommands,
  serializeHarnessCommands,
  withSkills,
} from "./commands";

const command = (name: string, action = "execute", description = ""): SlashCommand => ({
  name,
  description: description || `${name} description`,
  takesArguments: false,
  report: false,
  action,
});

const BASE = [
  command("/status"),
  command("/model", "model_picker"),
  command("/clear", "new_session"),
  command("/compact"),
];

const names = (list: SlashCommand[]) => list.map((c) => c.name);

describe("the slash-command menu of a thread without a session", () => {
  test("the engine's catalog is the built-in harness's menu before any session", () => {
    expect(names(menuCommands({ base: BASE, builtin: true }))).toEqual([
      "/status",
      "/model",
      "/clear",
      "/compact",
    ]);
  });

  test("a harness's last list sits over the catalog by name, and adds what it has", () => {
    const seen = [command("/compact", "execute", "Compact Claude's history"), command("/review")];
    const menu = menuCommands({ base: BASE, seen, builtin: true });
    expect(names(menu)).toEqual(["/status", "/model", "/clear", "/compact", "/review"]);
    expect(menu.find((c) => c.name === "/compact")?.description).toBe("Compact Claude's history");
  });

  test("another harness gets only the commands the app answers itself", () => {
    // `/status` and `/compact` are prompts to the harness there, which has advisory commands of
    // its own; `/model` and `/clear` never reach it.
    expect(names(menuCommands({ base: BASE, builtin: false }))).toEqual(["/model", "/clear"]);
    const seen = [command("/compact"), command("/review")];
    expect(names(menuCommands({ base: BASE, seen, builtin: false }))).toEqual([
      "/model",
      "/clear",
      "/compact",
      "/review",
    ]);
  });

  test("a harness's list moves to the most recent end and the map stays bounded", () => {
    let all: HarnessCommands = {};
    for (let i = 0; i < HARNESS_COMMANDS_MAX + 3; i++) all = rememberCommands(all, `h${i}`, [command("/a")]);
    const ids = Object.keys(all);
    expect(ids.length).toBe(HARNESS_COMMANDS_MAX);
    // The first three fell off, the newest is last.
    expect(ids[0]).toBe("h3");
    expect(ids[ids.length - 1]).toBe(`h${HARNESS_COMMANDS_MAX + 2}`);
    // Updating one that is already known moves it up instead of adding a second entry.
    all = rememberCommands(all, "h3", [command("/b")]);
    expect(Object.keys(all).at(-1)).toBe("h3");
    expect(names(all.h3 ?? [])).toEqual(["/b"]);
  });

  test("a list longer than the bound is trimmed", () => {
    const many = Array.from({ length: COMMANDS_PER_HARNESS_MAX + 5 }, (_, i) => command(`/c${i}`));
    const all = rememberCommands({}, "rustcode", many);
    expect(all.rustcode?.length).toBe(COMMANDS_PER_HARNESS_MAX);
  });

  test("the engine's skills join a harness's own list, and one it already has stays its own", () => {
    const own = [command("/compact"), command("/review", "execute", "Claude's review")];
    const base = [
      command("/status"),
      command("/release-automation", "skill", "Cut a release"),
      command("/review", "skill", "RustCode's review"),
    ];
    const merged = withSkills(own, base);
    expect(names(merged)).toEqual(["/compact", "/review", "/release-automation"]);
    // A command the engine answers itself is not a skill and does not cross over.
    expect(names(merged)).not.toContain("/status");
    expect(merged.find((c) => c.name === "/review")?.description).toBe("Claude's review");
    // Nothing to add returns the list itself, so a re-render keeps the reference.
    expect(withSkills(own, [])).toBe(own);
  });

  test("stored lists are read back, and malformed ones are dropped", () => {
    const stored = serializeHarnessCommands({ rustcode: [command("/status")] });
    expect(names(parseHarnessCommands(stored).rustcode ?? [])).toEqual(["/status"]);
    expect(parseHarnessCommands(null)).toEqual({});
    expect(parseHarnessCommands("not json")).toEqual({});
    expect(parseHarnessCommands('{"rustcode":[{"description":"no name"}]}')).toEqual({});
    expect(parseHarnessCommands('{"rustcode":[{"name":"status"}]}').rustcode).toEqual([
      {
        name: "/status",
        description: "",
        takesArguments: false,
        report: false,
        action: "execute",
      },
    ]);
  });
});
