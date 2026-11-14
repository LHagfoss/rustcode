import { describe, expect, test } from "bun:test";
import { restoreChoice, SETTINGS_AREAS, stepArea, WINDOW_AREAS } from "./focusAreas";

const all = new Set<string>(WINDOW_AREAS);

describe("the F6 area cycle", () => {
  test("goes threads, conversation, message input, panel, terminal and wraps", () => {
    const seen: string[] = [];
    let at: string | null = "threads";
    for (let i = 0; i < 5; i += 1) {
      at = stepArea(WINDOW_AREAS, all, at, 1);
      seen.push(at ?? "none");
    }
    expect(seen).toEqual(["main", "composer", "panel", "terminal", "threads"]);
  });

  test("Shift+F6 goes back and wraps", () => {
    expect(stepArea(WINDOW_AREAS, all, "threads", -1)).toBe("terminal");
    expect(stepArea(WINDOW_AREAS, all, "composer", -1)).toBe("main");
  });

  test("skips hidden areas", () => {
    const shown = new Set(["main", "composer"]);
    expect(stepArea(WINDOW_AREAS, shown, "composer", 1)).toBe("main");
    expect(stepArea(WINDOW_AREAS, shown, "main", -1)).toBe("composer");
    const noPanel = new Set(["threads", "main", "composer", "terminal"]);
    expect(stepArea(WINDOW_AREAS, noPanel, "composer", 1)).toBe("terminal");
    expect(stepArea(WINDOW_AREAS, noPanel, "terminal", -1)).toBe("composer");
  });

  test("from outside every area starts at the first shown one, or the last going back", () => {
    const shown = new Set(["main", "composer", "panel"]);
    expect(stepArea(WINDOW_AREAS, shown, null, 1)).toBe("main");
    expect(stepArea(WINDOW_AREAS, shown, null, -1)).toBe("panel");
    expect(stepArea(WINDOW_AREAS, shown, "unknown", 1)).toBe("main");
  });

  test("goes nowhere with only the current area shown, or none", () => {
    expect(stepArea(WINDOW_AREAS, new Set(["composer"]), "composer", 1)).toBeNull();
    expect(stepArea(WINDOW_AREAS, new Set(), null, 1)).toBeNull();
    expect(stepArea(WINDOW_AREAS, new Set(["composer"]), null, 1)).toBe("composer");
  });

  test("Settings moves between the section list and the open section", () => {
    const shown = new Set<string>(SETTINGS_AREAS);
    expect(stepArea(SETTINGS_AREAS, shown, "sections", 1)).toBe("section");
    expect(stepArea(SETTINGS_AREAS, shown, "section", 1)).toBe("sections");
    expect(stepArea(SETTINGS_AREAS, shown, "section", -1)).toBe("sections");
  });
});

describe("restoring focus in an area", () => {
  const usable = (el: string) => !el.startsWith("gone");

  test("returns to the control that last had focus", () => {
    expect(restoreChoice("send", usable, ["field", "first", "area"])).toBe("send");
  });

  test("falls back when that control is gone or unusable", () => {
    expect(restoreChoice("gone:send", usable, ["field", "first", "area"])).toBe("field");
    expect(restoreChoice(undefined, usable, [null, "first", "area"])).toBe("first");
    expect(restoreChoice(null, usable, [undefined, "gone:first", "area"])).toBe("area");
  });

  test("is null when nothing can take focus", () => {
    expect(restoreChoice("gone", usable, ["gone:a", null])).toBeNull();
  });
});
