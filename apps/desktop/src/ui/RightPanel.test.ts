// The right panel's card width is fitted, not fixed (ui/RightPanel.tsx): it
// keeps the width it was given, but never draws past the column's edges, and the
// edge it drags or arrows also answer on the keyboard.

import { describe, expect, test } from "bun:test";
import { DEFAULT_WIDTH, MAX, MIN, panelWidth, resizedWidth } from "./RightPanel";

/** The card's gap from the window's edges, on both sides (see RightPanel.tsx). */
const GAPS = 24;

describe("panel width", () => {
  test("keeps the width it was given when the column has room", () => {
    expect(panelWidth(520, 1512)).toBe(520);
    expect(panelWidth(MIN, 1512)).toBe(MIN);
    expect(panelWidth(DEFAULT_WIDTH, 1512)).toBe(DEFAULT_WIDTH);
  });

  test("never draws past the column's edges", () => {
    expect(panelWidth(MAX, 700)).toBe(700 - GAPS);
    expect(panelWidth(460, 400)).toBe(400 - GAPS);
    // A column too narrow for even the smallest card still gets one: it is
    // drawn at its minimum and its content scrolls.
    expect(panelWidth(MAX, 300)).toBe(MIN);
  });

  test("stays between its own bounds", () => {
    expect(panelWidth(120, 1512)).toBe(MIN);
    expect(panelWidth(4000, 3000)).toBe(MAX);
  });

  test("stands as stored before the first measurement", () => {
    expect(panelWidth(520, 0)).toBe(520);
    expect(panelWidth(4000, 0)).toBe(MAX);
  });

  test("a narrow window does not overwrite the width it was given", () => {
    expect(panelWidth(620, 600)).toBe(600 - GAPS);
    expect(panelWidth(620, 1600)).toBe(620);
  });
});

describe("keyboard resizing", () => {
  test("arrows move the edge the way the pointer does", () => {
    expect(resizedWidth(460, "ArrowLeft", false)).toBe(484);
    expect(resizedWidth(460, "ArrowRight", false)).toBe(436);
    expect(resizedWidth(460, "ArrowLeft", true)).toBe(556);
  });

  test("stops at its bounds", () => {
    expect(resizedWidth(MIN, "ArrowRight", true)).toBe(MIN);
    expect(resizedWidth(MAX, "ArrowLeft", false)).toBe(MAX);
    expect(resizedWidth(MIN - 40, "ArrowRight", false)).toBe(MIN);
  });

  test("Home and End go to the ends, other keys do not resize", () => {
    expect(resizedWidth(500, "Home", false)).toBe(MIN);
    expect(resizedWidth(500, "End", true)).toBe(MAX);
    expect(resizedWidth(500, "ArrowUp", false)).toBeNull();
    expect(resizedWidth(500, "a", false)).toBeNull();
    expect(resizedWidth(500, "Escape", false)).toBeNull();
  });

  test("a key starts from the width on screen and stops at what it may draw", () => {
    // 620 asked for, 400 drawn in a 424px column.
    const shown = panelWidth(620, 424);
    expect(shown).toBe(400);
    expect(resizedWidth(shown, "ArrowRight", false, panelWidth(MAX, 424))).toBe(376);
    expect(resizedWidth(shown, "ArrowLeft", false, panelWidth(MAX, 424))).toBe(400);
    expect(resizedWidth(shown, "End", false, panelWidth(MAX, 424))).toBe(400);
    // Never below the pane's own minimum, even under a column that cannot hold it.
    expect(resizedWidth(MIN, "ArrowRight", true, MIN - 40)).toBe(MIN);
    expect(resizedWidth(MIN + 96, "Home", false, MIN - 40)).toBe(MIN);
  });
});
