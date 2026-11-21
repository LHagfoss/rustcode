import { describe, expect, test } from "bun:test";
import { reorderHeaderButtons } from "./HeaderButtonsSetting";

describe("header button drops", () => {
  test("reorders without mutating the saved order", () => {
    const shown = ["plan", "details"] as const;
    expect(reorderHeaderButtons(shown, "plan", { list: "shown", index: 1 })).toEqual(["details", "plan"]);
    expect(shown).toEqual(["plan", "details"]);
  });

  test("moves between groups and clamps the destination", () => {
    expect(reorderHeaderButtons(["plan", "details"], "plan", { list: "hidden", index: 0 })).toEqual([
      "details",
    ]);
    expect(reorderHeaderButtons(["details"], "plan", { list: "shown", index: 99 })).toEqual([
      "details",
      "plan",
    ]);
    expect(reorderHeaderButtons(["details"], "plan", { list: "shown", index: -1 })).toEqual([
      "plan",
      "details",
    ]);
  });
});
