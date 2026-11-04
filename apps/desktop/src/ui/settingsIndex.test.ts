import { describe, expect, test } from "bun:test";
import { SETTINGS_INDEX, SETTINGS_RESULTS, searchSettings } from "./settingsIndex";

describe("settings search", () => {
  test("finds a row by its title, in any case", () => {
    expect(searchSettings("TRACKPAD").map((r) => r.title)).toContain("Trackpad haptics");
    expect(searchSettings("rounded font")[0]?.title).toBe("Rounded font");
  });

  test("finds a row by its keywords and its section", () => {
    expect(searchSettings("haptic")[0]).toEqual({
      section: "general",
      title: "Trackpad haptics",
      keywords: "haptic tap feedback force touch",
    });
    // Every word must match, in any order: the row's keywords carry the words
    // its title does not.
    const tap = searchSettings("tap trackpad").map((r) => r.title);
    expect(tap).toEqual(["Trackpad haptics"]);
  });

  test("every word of the query has to match", () => {
    expect(searchSettings("trackpad sidebar")).toEqual([]);
  });

  test("nothing matches", () => {
    expect(searchSettings("zzzz")).toEqual([]);
  });

  test("an empty query returns nothing, so the nav shows the sections", () => {
    expect(searchSettings("")).toEqual([]);
    expect(searchSettings("   ")).toEqual([]);
  });

  test("shows at most 12 results", () => {
    // "shortcut" is in the keyboard rows' keywords, more than a dozen of them.
    const all = SETTINGS_INDEX.filter((e) => e.keywords?.includes("shortcut")).length;
    expect(all).toBeGreaterThan(SETTINGS_RESULTS);
    expect(searchSettings("shortcut").length).toBe(SETTINGS_RESULTS);
  });

  test("every entry names a section and a title", () => {
    for (const entry of SETTINGS_INDEX) {
      expect(entry.title.trim()).not.toBe("");
      expect([
        "general",
        "harnesses",
        "connections",
        "capabilities",
        "appearance",
        "usage",
        "engine",
      ]).toContain(entry.section);
    }
  });
});
