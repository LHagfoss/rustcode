import { describe, expect, test } from "bun:test";
import {
  cleanPreset,
  parsePresets,
  presetError,
  presetSummary,
  presetsJson,
  removePreset,
  savePreset,
} from "./delegatePresets";

describe("delegate presets", () => {
  test("parsing skips nameless and malformed entries and keeps known fields", () => {
    const presets = parsePresets([
      { name: " Pi flash ", harness: "pi", access: "edits", worktree: true, instructions: "Read AGENTS.md." },
      { name: "", harness: "codex" },
      { name: "bad access", access: "root" },
      "nope",
      null,
    ]);
    expect(presets).toEqual([
      { name: "Pi flash", harness: "pi", access: "edits", worktree: true, instructions: "Read AGENTS.md." },
      { name: "bad access" },
    ]);
    expect(parsePresets(undefined)).toEqual([]);
  });

  test("a name must be set and unique, ignoring case, except for the preset being edited", () => {
    const presets = [{ name: "Codex" }];
    expect(presetError(presets, { name: "  " })).toBe("A preset needs a name.");
    expect(presetError(presets, { name: "codex" })).toContain("already a preset");
    expect(presetError(presets, { name: "codex" }, "Codex")).toBeNull();
    expect(presetError(presets, { name: "pi" })).toBeNull();
  });

  test("saving replaces in place, adds at the end, and drops empty fields", () => {
    const presets = [{ name: "a" }, { name: "b" }];
    expect(savePreset(presets, { name: "c", model: " " })).toEqual([
      { name: "a" },
      { name: "b" },
      { name: "c" },
    ]);
    expect(savePreset(presets, { name: "a2", effort: "high" }, "a")).toEqual([
      { name: "a2", effort: "high" },
      { name: "b" },
    ]);
    expect(removePreset(presets, "a")).toEqual([{ name: "b" }]);
    expect(cleanPreset({ name: " x ", harness: "", worktree: false })).toEqual({
      name: "x",
      worktree: false,
    });
    expect(presetsJson([{ name: "x", account: "work" }])).toEqual([{ name: "x", account: "work" }]);
  });

  test("the summary names what a preset sets", () => {
    expect(presetSummary({ name: "x" })).toBe("Nothing set");
    expect(
      presetSummary({
        name: "x",
        harness: "codex",
        effort: "high",
        access: "ask",
        worktree: false,
        instructions: "y",
      }),
    ).toBe("codex · high · at most Ask · no worktree · instructions");
  });
});
