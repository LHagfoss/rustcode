import { describe, expect, test } from "bun:test";
import {
  APP_COMMANDS,
  bindingFromEvent,
  bindingOf,
  bindingProblem,
  commandFor,
  commandHint,
  conflictingCommands,
  formatBinding,
  type KeyEventLike,
  parseBinding,
  parseOverrides,
  RESERVED_BINDINGS,
  RESERVED_EDITING,
} from "./keys";

const press = (key: string, mods: Partial<KeyEventLike> = {}): KeyEventLike => ({ key, ...mods });
const cmd = (key: string, mods: Partial<KeyEventLike> = {}) => press(key, { metaKey: true, ...mods });

describe("shortcut bindings", () => {
  test("reads a keydown as the table's canonical text", () => {
    expect(bindingFromEvent(cmd("b"))).toBe("mod+b");
    expect(bindingFromEvent(press("b", { ctrlKey: true }))).toBe("mod+b");
    expect(bindingFromEvent(cmd("O", { shiftKey: true }))).toBe("mod+shift+o");
    expect(bindingFromEvent(cmd("."))).toBe("mod+.");
    expect(bindingFromEvent(cmd(",", { shiftKey: true }))).toBe("mod+,");
    expect(bindingFromEvent(press("F6"))).toBe("f6");
    expect(bindingFromEvent(press("Enter", { shiftKey: true }))).toBe("shift+enter");
  });

  test("⌘+ and ⌘= are one binding, because + only exists shifted", () => {
    expect(bindingFromEvent(cmd("+", { shiftKey: true }))).toBe("mod+=");
    expect(bindingFromEvent(cmd("="))).toBe("mod+=");
  });

  test("a modifier on its own starts nothing", () => {
    expect(bindingFromEvent(press("Shift", { shiftKey: true }))).toBeNull();
    expect(bindingFromEvent(cmd("Meta"))).toBeNull();
    expect(bindingFromEvent(press("Dead", { altKey: true }))).toBeNull();
    expect(bindingFromEvent(press("Print"))).toBeNull();
  });

  test("parses and prints bindings", () => {
    expect(parseBinding("mod+shift+o")).toEqual({ mod: true, alt: false, shift: true, key: "o" });
    expect(formatBinding("mod+shift+o")).toBe("⌘⇧O");
    expect(formatBinding("mod+=")).toBe("⌘+");
    expect(formatBinding("mod+alt+b")).toBe("⌘⌥B");
    expect(formatBinding("escape")).toBe("Esc");
    expect(formatBinding("mod+enter")).toBe("⌘⏎");
    expect(parseBinding("⌘⇧O")).toEqual({ mod: true, alt: false, shift: true, key: "o" });
    // Tolerant: the same binding in the symbols Settings shows, and in words.
    expect(parseBinding("command+shift+o")).toEqual(parseBinding("mod+shift+o")!);
    expect(parseBinding("option+b")).toEqual({ mod: false, alt: true, shift: false, key: "b" });
    expect(parseBinding("mod")).toBeNull();
    expect(parseBinding("mod+nope")).toBeNull();
    expect(parseBinding("mod++")).toBeNull();
    expect(parseBinding("")).toBeNull();
  });

  test("the defaults name the keys the app has always answered", () => {
    const defaults = Object.fromEntries(APP_COMMANDS.map((c) => [c.id, c.binding]));
    expect(defaults).toMatchObject({
      "new-thread": "mod+n",
      "thread-folder": "mod+o",
      "default-folder": "mod+shift+o",
      "command-palette": "mod+k",
      settings: "mod+,",
      "toggle-sidebar": "mod+b",
      "find-in-thread": "mod+f",
      "toggle-terminal": "mod+j",
      "stop-turn": "mod+.",
      "zoom-in": "mod+=",
      "zoom-out": "mod+-",
      "zoom-actual": "mod+0",
    });
  });
});

describe("matching", () => {
  test("a keydown runs the command it is bound to", () => {
    expect(commandFor({}, cmd("b"))).toBe("toggle-sidebar");
    expect(commandFor({}, cmd("O", { shiftKey: true }))).toBe("default-folder");
    expect(commandFor({}, press("b", { ctrlKey: true }))).toBe("toggle-sidebar");
    expect(commandFor({}, cmd("Enter"))).toBeNull();
    expect(commandFor({}, press("Enter"))).toBeNull();
    expect(commandFor({}, press("b"))).toBeNull();
  });

  test("an override moves the command and frees the default", () => {
    const overrides = { "toggle-sidebar": "mod+alt+s" };
    expect(commandFor(overrides, cmd("s", { altKey: true }))).toBe("toggle-sidebar");
    expect(commandFor(overrides, cmd("b"))).toBeNull();
    expect(bindingOf(overrides, "toggle-sidebar")).toBe("mod+alt+s");
    expect(bindingOf(overrides, "new-thread")).toBe("mod+n");
  });

  test("composer keys never fire a changeable command", () => {
    expect(commandFor({}, press("Enter", { shiftKey: true }))).toBeNull();
    expect(commandFor({}, cmd("Enter"))).toBeNull();
    expect(commandFor({}, press("3"))).toBeNull();
  });

  test("a hint reads the binding, or the command's own display", () => {
    const send = APP_COMMANDS.find((c) => c.id === "answer-approval")!;
    expect(commandHint("", send)).toBe("1–9");
    expect(commandHint("mod+b")).toBe("⌘B");
  });
});

describe("what a binding may be", () => {
  test("needs a modifier unless it is a function key", () => {
    expect(bindingProblem({}, "new-thread", "b")).toContain("function key");
    expect(bindingProblem({}, "new-thread", "shift+b")).toContain("function key");
    expect(bindingProblem({}, "new-thread", "f6")).toBeNull();
    expect(bindingProblem({}, "new-thread", "mod+alt+n")).toBeNull();
  });

  test("refuses the combinations the system owns", () => {
    for (const binding of RESERVED_BINDINGS) {
      expect(bindingProblem({}, "new-thread", binding)).toContain("macOS");
    }
    expect(RESERVED_BINDINGS).toEqual(["mod+q", "mod+w", "mod+h", "mod+m", "mod+tab"]);
  });

  test("refuses the editing keys the menu answers first", () => {
    for (const binding of RESERVED_EDITING) {
      expect(bindingProblem({}, "new-thread", binding)).toContain("copy, paste and undo");
    }
    expect(parseOverrides(JSON.stringify({ "new-thread": "mod+v" }))).toEqual({});
  });

  test("names the command that already answers the key", () => {
    expect(bindingProblem({}, "new-thread", "mod+b")).toContain("Toggle sidebar");
    expect(conflictingCommands({}, "new-thread", "mod+b").map((c) => c.id)).toEqual(["toggle-sidebar"]);
    // Its own binding is not a conflict with itself.
    expect(conflictingCommands({}, "toggle-sidebar", "mod+b")).toEqual([]);
    expect(conflictingCommands({}, "new-thread", "mod+alt+n")).toEqual([]);
  });

  test("an override frees the key it left", () => {
    const overrides = { "toggle-sidebar": "mod+alt+s" };
    expect(bindingProblem(overrides, "new-thread", "mod+b")).toBeNull();
    expect(bindingProblem(overrides, "new-thread", "mod+alt+s")).toContain("Toggle sidebar");
  });

  test("a key this app cannot use says so", () => {
    expect(bindingProblem({}, "new-thread", "mod+nope")).toContain("not a key");
    expect(bindingProblem({}, "new-thread", "")).toContain("not a key");
  });
});

describe("stored overrides", () => {
  test("a missing or corrupt value leaves the defaults", () => {
    expect(parseOverrides(null)).toEqual({});
    expect(parseOverrides("")).toEqual({});
    expect(parseOverrides("{")).toEqual({});
    expect(parseOverrides("[]")).toEqual({});
    expect(parseOverrides("null")).toEqual({});
    expect(parseOverrides('"mod+b"')).toEqual({});
  });

  test("keeps the bindings it can use and drops the rest", () => {
    const stored = JSON.stringify({
      "toggle-sidebar": "mod+alt+s",
      "no-such-command": "mod+y",
      "new-thread": "n", // no modifier
      "default-folder": "mod+q", // reserved
      "stop-turn": "mod+nope", // not a key
      "toggle-terminal": 42, // not a string
    });
    expect(parseOverrides(stored)).toEqual({ "toggle-sidebar": "mod+alt+s" });
  });

  test("a stored binding equal to the default is not kept as an override", () => {
    expect(parseOverrides(JSON.stringify({ "toggle-sidebar": "mod+b" }))).toEqual({});
  });

  test("two commands never share one key, whatever the file says", () => {
    // The first command in the table keeps the binding; the later one is dropped.
    const stored = JSON.stringify({ "toggle-sidebar": "mod+y", "new-thread": "mod+y" });
    expect(parseOverrides(stored)).toEqual({ "new-thread": "mod+y" });
  });
});
