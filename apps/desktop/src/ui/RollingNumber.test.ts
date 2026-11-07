import { describe, expect, test } from "bun:test";
import { digitColumns, rollDigits } from "./RollingNumber";

describe("rolling digits", () => {
  test("digits are keyed from the right, so the units column keeps its key", () => {
    expect(digitColumns(0)).toEqual([{ key: "d0", digit: "0" }]);
    expect(digitColumns(9)).toEqual([{ key: "d0", digit: "9" }]);
    expect(digitColumns(10)).toEqual([
      { key: "d1", digit: "1" },
      { key: "d0", digit: "0" },
    ]);
    expect(digitColumns(1204).map((c) => c.key)).toEqual(["d3", "d2", "d1", "d0"]);
  });

  test("a growing count rolls its units and adds a leading digit", () => {
    expect(rollDigits(9, 10)).toEqual([
      { key: "d1", digit: "1", direction: "up" },
      { key: "d0", digit: "0", from: "9", direction: "up" },
    ]);
  });

  test("a shrinking count rolls down", () => {
    expect(rollDigits(10, 9)).toEqual([{ key: "d0", digit: "9", from: "0", direction: "down" }]);
  });

  test("only digits that changed carry a from", () => {
    const rolls = rollDigits(120, 121);
    expect(rolls.find((c) => c.key === "d1")?.from).toBeUndefined();
    expect(rolls.find((c) => c.key === "d2")?.from).toBeUndefined();
    expect(rolls.find((c) => c.key === "d0")).toMatchObject({ digit: "1", from: "0", direction: "up" });
  });

  test("an unchanged count has no rolls", () => {
    expect(rollDigits(42, 42).some((c) => c.from !== undefined)).toBe(false);
  });

  test("a negative count reads as its magnitude", () => {
    expect(digitColumns(-7)).toEqual([{ key: "d0", digit: "7" }]);
  });
});
