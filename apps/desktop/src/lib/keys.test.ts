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
  listCommandFor,
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
    expect(bindingFromEvent(press("b", { ctrlKey: true }))).toBeNull();
    expect(bindingFromEvent(cmd("O", { shiftKey: true }))).toBe("mod+shift+o");
    expect(bindingFromEvent(cmd("."))).toBe("mod+.");
    expect(bindingFromEvent(cmd(",", { shiftKey: true }))).toBe("mod+,");
    expect(bindingFromEvent(press("F6"))).toBe("f6");
    expect(bindingFromEvent(press("Enter", { shiftKey: true }))).toBe("shift+enter");
  });

  test("uses each platform's modifier and labels", () => {
    for (const platform of ["windows", "linux"] as const) {
      expect(formatBinding("mod+shift+p", platform)).toBe("Ctrl+Shift+P");
      expect(formatBinding("mod+alt+b", platform)).toBe("Ctrl+Alt+B");
      expect(formatBinding("mod+enter", platform)).toBe("Ctrl+Enter");
      expect(bindingFromEvent(press("k", { ctrlKey: true }), platform)).toBe("mod+k");
      expect(bindingFromEvent(cmd("k"), platform)).toBeNull();
    }
    expect(formatBinding("mod+shift+p", "mac")).toBe("⌘⇧P");
    expect(bindingFromEvent(cmd("k"), "mac")).toBe("mod+k");
    expect(bindingFromEvent(press("k", { ctrlKey: true }), "mac")).toBeNull();
  });

  test("normalizes browser arrow names in shortcuts", () => {
    expect(bindingFromEvent(press("ArrowLeft", { ctrlKey: true }), "windows")).toBe("mod+left");
    expect(bindingFromEvent(press("ArrowUp", { ctrlKey: true, shiftKey: true }), "linux")).toBe(
      "mod+shift+up",
    );
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
      "pin-thread": "p",
      "settle-thread": "d",
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
    expect(commandFor({}, press("b", { ctrlKey: true }))).toBeNull();
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

  test("the list's marking keys are answered by the list, never by the window", () => {
    expect(commandFor({}, press("p"))).toBeNull();
    expect(commandFor({}, press("d"))).toBeNull();
    expect(listCommandFor({}, press("p"))).toBe("pin-thread");
    expect(listCommandFor({}, press("d"))).toBe("settle-thread");
    // A shifted or modified key is not the bare one, and the app's own keys are not the list's.
    expect(listCommandFor({}, press("P", { shiftKey: true }))).toBeNull();
    expect(listCommandFor({}, cmd("b"))).toBeNull();
  });

  test("a list key moves with its remap, and leaves the bare one free", () => {
    const overrides = { "pin-thread": "mod+shift+p" };
    expect(listCommandFor(overrides, cmd("P", { shiftKey: true }))).toBe("pin-thread");
    expect(listCommandFor(overrides, press("p"))).toBeNull();
    // Still the list's own key: the window does not pick it up either way.
    expect(commandFor(overrides, cmd("P", { shiftKey: true }))).toBeNull();
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

  test("a bare letter is usable for a list key, and for nothing else", () => {
    expect(bindingProblem({}, "pin-thread", "p")).toBeNull();
    expect(bindingProblem({}, "settle-thread", "d")).toBeNull();
    expect(bindingProblem({}, "new-thread", "p")).toContain("function key");
    expect(bindingProblem({}, "pin-thread", "mod+b")).toContain("Toggle sidebar");
  });

  test("a list key is one printable character, not a key a row already answers", () => {
    for (const key of [
      "tab",
      "backspace",
      "delete",
      "insert",
      "home",
      "end",
      "pageup",
      "pagedown",
      "space",
      "up",
      "down",
      "left",
      "right",
    ]) {
      expect(bindingProblem({}, "pin-thread", key)).toContain("function key");
      expect(bindingProblem({}, "settle-thread", key)).toContain("function key");
    }
    // The named keys that are not a row's own are still function keys or modified.
    expect(bindingProblem({}, "pin-thread", "f6")).toBeNull();
    expect(bindingProblem({}, "pin-thread", "mod+tab")).toContain("reserved by the system");
  });

  test("refuses the combinations the system owns", () => {
    for (const binding of RESERVED_BINDINGS) {
      expect(bindingProblem({}, "new-thread", binding)).toContain("reserved by the system");
    }
    expect(RESERVED_BINDINGS).toEqual(["mod+q", "mod+w", "mod+h", "mod+m", "mod+tab"]);
  });

  test("refuses the editing keys a field owns", () => {
    for (const binding of RESERVED_EDITING) {
      expect(bindingProblem({}, "new-thread", binding)).toContain("text editing");
    }
    expect(parseOverrides(JSON.stringify({ "new-thread": "mod+v" }))).toEqual({});
  });

  test("keeps word editing shortcuts available in fields", () => {
    for (const binding of [
      "mod+y",
      "mod+backspace",
      "mod+delete",
      "mod+left",
      "mod+right",
      "mod+up",
      "mod+down",
      "mod+shift+left",
      "mod+shift+right",
      "mod+shift+up",
      "mod+shift+down",
      "mod+home",
      "mod+end",
    ]) {
      expect(bindingProblem({}, "new-thread", binding)).not.toBeNull();
    }
    expect(parseOverrides(JSON.stringify({ "toggle-sidebar": "mod+left" }))).toEqual({});
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

  test("a list key keeps a bare override; no other command does", () => {
    expect(parseOverrides(JSON.stringify({ "pin-thread": "q" }))).toEqual({ "pin-thread": "q" });
    expect(parseOverrides(JSON.stringify({ "new-thread": "q" }))).toEqual({});
    // Its own default is not an override.
    expect(parseOverrides(JSON.stringify({ "settle-thread": "d" }))).toEqual({});
  });

  test("a stored list key that is not a printable character is dropped", () => {
    for (const key of ["tab", "space", "backspace", "delete", "home", "end", "pagedown", "up"]) {
      expect(parseOverrides(JSON.stringify({ "pin-thread": key }))).toEqual({});
      expect(parseOverrides(JSON.stringify({ "settle-thread": key }))).toEqual({});
    }
    // A function key or a modified one stays, as for every other command.
    expect(parseOverrides(JSON.stringify({ "pin-thread": "f6" }))).toEqual({ "pin-thread": "f6" });
  });

  test("a stored binding equal to the default is not kept as an override", () => {
    expect(parseOverrides(JSON.stringify({ "toggle-sidebar": "mod+b" }))).toEqual({});
  });

  test("two commands never share one key, whatever the file says", () => {
    // The first command in the table keeps the binding; the later one is dropped.
    const stored = JSON.stringify({ "toggle-sidebar": "mod+alt+y", "new-thread": "mod+alt+y" });
    expect(parseOverrides(stored)).toEqual({ "new-thread": "mod+alt+y" });
  });
});
