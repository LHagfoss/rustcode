import { describe, expect, test } from "bun:test";
import { SETTINGS_SECTIONS, settingsSection } from "../state/store";
import { SETTINGS_INDEX, SETTINGS_RESULTS, searchSettings } from "./settingsIndex";

describe("settings search", () => {
  test("finds a row by its title, in any case", () => {
    expect(searchSettings("TRACKPAD").map((r) => r.title)).toContain("Trackpad haptics");
    expect(searchSettings("TEXT SIZE").map((r) => r.title)).toContain("Text size");
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

  test("an empty query returns nothing, so the nav shows the pages", () => {
    expect(searchSettings("")).toEqual([]);
    expect(searchSettings("   ")).toEqual([]);
  });

  test("shows at most 12 results", () => {
    // "shortcut" is in the keyboard rows' keywords, more than a dozen of them.
    const all = SETTINGS_INDEX.filter((e) => e.keywords?.includes("shortcut")).length;
    expect(all).toBeGreaterThan(SETTINGS_RESULTS);
    expect(searchSettings("shortcut").length).toBe(SETTINGS_RESULTS);
  });

  test("every entry names a page the panel shows, and a title", () => {
    for (const entry of SETTINGS_INDEX) {
      expect(entry.title.trim()).not.toBe("");
      expect(SETTINGS_SECTIONS).toContain(entry.section);
    }
  });

  test("every page has something to find", () => {
    for (const section of SETTINGS_SECTIONS) {
      expect(SETTINGS_INDEX.some((entry) => entry.section === section)).toBe(true);
    }
  });

  test("voice, computer use and orchestration land on their own pages, not Harnesses", () => {
    expect(searchSettings("dictation")[0]?.section).toBe("voice");
    expect(searchSettings("computer use")[0]?.section).toBe("voice");
    expect(searchSettings("workflows")[0]?.section).toBe("orchestration");
    expect(searchSettings("delegate presets")[0]?.section).toBe("orchestration");
    expect(searchSettings("merge delegate wakes")[0]?.section).toBe("orchestration");
    const harnesses = SETTINGS_INDEX.filter((entry) => entry.section === "agents").map((e) => e.title);
    expect(harnesses).not.toContain("Voice");
    expect(harnesses).not.toContain("Delegation");
  });

  test("a page id from an older build still opens a page", () => {
    // The Harnesses page kept its old `agents` id and Engine became Advanced;
    // nothing else moved.
    expect(settingsSection("harnesses")).toBe("agents");
    expect(settingsSection("engine")).toBe("advanced");
    expect(settingsSection("usage")).toBe("usage");
    expect(settingsSection("orchestration")).toBe("orchestration");
    expect(settingsSection("voice")).toBe("voice");
    expect(settingsSection("no-such-page")).toBe("general");
    expect(settingsSection(null)).toBeNull();
  });
});
