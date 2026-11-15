import { describe, expect, test } from "bun:test";
import { quickActionTarget } from "./menuNavigation";

describe("quick action keyboard navigation", () => {
  test("left and right traverse and wrap inside the quick row", () => {
    expect(quickActionTarget("ArrowRight", 0, 4, 7)).toBe(1);
    expect(quickActionTarget("ArrowRight", 3, 4, 7)).toBe(0);
    expect(quickActionTarget("ArrowLeft", 0, 4, 7)).toBe(3);
    expect(quickActionTarget("ArrowLeft", 2, 4, 7)).toBe(1);
  });

  test("down enters the list from every column; up returns to the quick row", () => {
    for (let column = 0; column < 4; column++) {
      expect(quickActionTarget("ArrowDown", column, 4, 7)).toBe(4);
      expect(quickActionTarget("ArrowUp", column, 4, 7)).toBe(6);
    }
    expect(quickActionTarget("ArrowUp", 4, 4, 7)).toBe(3);
  });

  test("menus consisting only of quick actions keep vertical focus in the row", () => {
    expect(quickActionTarget("ArrowDown", 2, 4, 4)).toBe(2);
    expect(quickActionTarget("ArrowUp", 2, 4, 4)).toBe(2);
  });

  test("ordinary rows, submenus, typeahead and activation keep their own keys", () => {
    for (const key of ["ArrowLeft", "ArrowRight", "ArrowDown", "Enter", " ", "Escape", "Home", "End", "p"]) {
      expect(quickActionTarget(key, 5, 4, 7)).toBeNull();
    }
    for (const key of ["Enter", " ", "Escape", "Home", "End", "p"]) {
      expect(quickActionTarget(key, 0, 4, 7)).toBeNull();
    }
    expect(quickActionTarget("ArrowRight", -1, 4, 7)).toBeNull();
    expect(quickActionTarget("ArrowRight", 0, 0, 7)).toBeNull();
  });
});
