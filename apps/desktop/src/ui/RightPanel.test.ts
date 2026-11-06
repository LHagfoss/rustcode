// The right panel's card is docked beside the chat (ui/RightPanel.tsx): the width
// it draws keeps what it was given but never takes the room the chat column needs,
// nor more than 70% of the window (the edge it drags also answers on the
// keyboard), and a narrow enough window gives up on both columns and opens it as
// a sheet instead.

import { describe, expect, test } from "bun:test";
import {
  CHAT_MIN,
  DEFAULT_WIDTH,
  MAX,
  MIN,
  panelIsSheet,
  panelMax,
  panelWidth,
  resizedWidth,
  SHEET_AT,
} from "./RightPanel";

describe("the width the card may draw", () => {
  test("no wider than its own cap, 70% of the window, or the room the chat leaves", () => {
    // A 1512px window has room for the card's own cap.
    expect(panelMax(1512, 1512)).toBe(MAX);
    // The fraction is what binds in a window of 700px: 490, under the card's own cap.
    expect(panelMax(700, 2000)).toBe(Math.floor(700 * 0.7));
    // The row is what binds when the chat needs its 360px back.
    expect(panelMax(1512, 900)).toBe(900 - CHAT_MIN);
  });

  test("never under its own minimum, even in a row that cannot hold it", () => {
    expect(panelMax(1512, 500)).toBe(MIN);
    // 70% of a 400px window is 280, under the card's own floor.
    expect(panelMax(400, 1512)).toBe(MIN);
  });

  test("keeps the width it was given when the window has room", () => {
    expect(panelWidth(520, 1512, 1512)).toBe(520);
    expect(panelWidth(MIN, 1512, 1512)).toBe(MIN);
    expect(panelWidth(DEFAULT_WIDTH, 1512, 1512)).toBe(DEFAULT_WIDTH);
  });

  test("is fitted down to what the row and the window allow", () => {
    expect(panelWidth(MAX, 1512, 900)).toBe(900 - CHAT_MIN);
    expect(panelWidth(4000, 1512, 1512)).toBe(MAX);
    expect(panelWidth(120, 1512, 1512)).toBe(MIN);
    // 70% of a 900px window is 630.
    expect(panelWidth(MAX, 900, 2000)).toBe(630);
  });

  test("stands as stored before the row is measured", () => {
    expect(panelWidth(520, 1512, 0)).toBe(520);
    expect(panelWidth(4000, 1512, 0)).toBe(MAX);
  });

  test("a narrow window does not overwrite the width it was given", () => {
    expect(panelWidth(620, 1000, 900)).toBe(Math.min(620, 700, 900 - CHAT_MIN));
    expect(panelWidth(620, 1600, 1600)).toBe(620);
  });
});

describe("the sheet a narrow window opens", () => {
  test("the panel is a sheet at 980px and under, and docked above it", () => {
    expect(panelIsSheet(SHEET_AT)).toBe(true);
    expect(panelIsSheet(900)).toBe(true);
    expect(panelIsSheet(SHEET_AT + 1)).toBe(false);
    expect(panelIsSheet(1512)).toBe(false);
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
    // 620 asked for, 420 drawn in a 780px row.
    const shown = panelWidth(620, 1512, 780);
    expect(shown).toBe(780 - CHAT_MIN);
    const widest = panelMax(1512, 780);
    expect(widest).toBe(780 - CHAT_MIN);
    expect(resizedWidth(shown, "ArrowRight", false, widest)).toBe(396);
    expect(resizedWidth(shown, "ArrowLeft", false, widest)).toBe(420);
    expect(resizedWidth(shown, "End", false, widest)).toBe(420);
    // Never below the card's own minimum, even under a row that cannot hold it.
    expect(resizedWidth(MIN, "ArrowRight", true, MIN - 40)).toBe(MIN);
    expect(resizedWidth(MIN + 96, "Home", false, MIN - 40)).toBe(MIN);
  });
});
