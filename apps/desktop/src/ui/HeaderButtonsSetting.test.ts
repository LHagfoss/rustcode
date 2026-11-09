import { describe, expect, test } from "bun:test";
import { reorderHeaderButtons } from "./HeaderButtonsSetting";

describe("header button drops", () => {
  test("reorders without mutating the saved order", () => {
    const shown = ["status", "plan", "pulls"] as const;
    expect(reorderHeaderButtons(shown, "status", { list: "shown", index: 2 })).toEqual([
      "plan",
      "pulls",
      "status",
    ]);
    expect(shown).toEqual(["status", "plan", "pulls"]);
  });

  test("moves between groups and clamps the destination", () => {
    expect(reorderHeaderButtons(["status", "plan"], "status", { list: "hidden", index: 0 })).toEqual([
      "plan",
    ]);
    expect(reorderHeaderButtons(["status"], "plan", { list: "shown", index: 99 })).toEqual([
      "status",
      "plan",
    ]);
    expect(reorderHeaderButtons(["status"], "plan", { list: "shown", index: -1 })).toEqual([
      "plan",
      "status",
    ]);
  });
});
