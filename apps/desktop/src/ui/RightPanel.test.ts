// The right panel's width is fitted, not fixed (ui/RightPanel.tsx): the pane
// keeps the width it was given, but never takes the room the chat column keeps,
// and the edge it drags or arrows also answer on the keyboard.

import { describe, expect, test } from "bun:test";
import {
  CHAT_FLOOR,
  chatFloor,
  fittedWidth,
  MAX,
  MIN,
  NARROW_CHAT_FLOOR,
  OVERLAY_BELOW,
  overlays,
  overlayWidth,
  paneMax,
  resizedWidth,
} from "./RightPanel";

describe("panel width", () => {
  test("keeps the width it was given when the window has room", () => {
    expect(fittedWidth(520, 1512)).toBe(520);
    expect(fittedWidth(MIN, 1512)).toBe(MIN);
  });

  test("never takes the room the chat column keeps", () => {
    expect(paneMax(1512)).toBe(MAX);
    expect(paneMax(1000)).toBe(1000 - CHAT_FLOOR);
    expect(fittedWidth(MAX, 1000)).toBe(1000 - CHAT_FLOOR);
  });

  test("the chat keeps less on a window narrower than NARROW", () => {
    expect(chatFloor(899)).toBe(NARROW_CHAT_FLOOR);
    expect(chatFloor(900)).toBe(CHAT_FLOOR);
    expect(fittedWidth(MAX, 800)).toBe(800 - NARROW_CHAT_FLOOR);
  });

  test("stays between its own bounds", () => {
    expect(fittedWidth(120, 1512)).toBe(MIN);
    expect(fittedWidth(4000, 3000)).toBe(MAX);
    // A window too narrow for both still leaves the pane its minimum, and
    // that is where it floats over the chat instead (see below).
    expect(fittedWidth(MAX, 500)).toBe(MIN);
  });

  test("stands as stored before the first measurement", () => {
    expect(fittedWidth(520, 0)).toBe(520);
    expect(fittedWidth(4000, 0)).toBe(MAX);
  });

  test("a narrow window does not overwrite the width it was given", () => {
    expect(fittedWidth(620, 900)).toBe(900 - CHAT_FLOOR);
    expect(fittedWidth(620, 1600)).toBe(620);
  });
});

describe("a column with no room for both", () => {
  test("docks where the pane and the chat both fit, and floats below that", () => {
    expect(overlays(0)).toBe(false);
    expect(overlays(OVERLAY_BELOW)).toBe(false);
    expect(overlays(OVERLAY_BELOW - 1)).toBe(true);
  });

  test("the floating pane keeps the chat's width and takes its own, up to the window", () => {
    expect(overlayWidth(460, 700)).toBe(460);
    expect(overlayWidth(460, 384)).toBe(384);
    expect(overlayWidth(460, 200)).toBe(MIN);
    expect(overlayWidth(140, 700)).toBe(MIN);
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
    // 620 asked for, 384 fitted to a 744px column beside the chat's 360px.
    const shown = fittedWidth(620, 744);
    expect(shown).toBe(384);
    expect(resizedWidth(shown, "ArrowRight", false, paneMax(744))).toBe(360);
    expect(resizedWidth(shown, "ArrowLeft", false, paneMax(744))).toBe(384);
    expect(resizedWidth(shown, "End", false, paneMax(744))).toBe(384);
    // Never below the pane's own minimum, even under a column that cannot hold it.
    expect(resizedWidth(MIN, "ArrowRight", true, MIN - 40)).toBe(MIN);
    expect(resizedWidth(MIN + 96, "Home", false, MIN - 40)).toBe(MIN);
  });
});
