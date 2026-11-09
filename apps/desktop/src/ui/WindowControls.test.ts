// The window's own controls: which platforms draw them, the room a top row leaves for them,
// and that a page with no shell behind it (the mock window, `dev:web`) asks for nothing.

import { describe, expect, test } from "bun:test";
import {
  drawsWindowControls,
  WINDOW_CONTROLS_WIDTH,
  windowAction,
  windowControlsInset,
} from "./WindowControls";

describe("the window's own controls", () => {
  test("only Windows draws them, because only Windows has no native decorations", () => {
    expect(drawsWindowControls("windows")).toBe(true);
    expect(drawsWindowControls("mac")).toBe(false);
    expect(drawsWindowControls("linux")).toBe(false);
  });

  test("a row that reaches the window's edge leaves the controls their room on Windows only", () => {
    expect(windowControlsInset("windows")).toBe(WINDOW_CONTROLS_WIDTH);
    expect(windowControlsInset("mac")).toBe(0);
    expect(windowControlsInset("linux")).toBe(0);
  });

  test("three 46 px buttons are as wide as the room they are given", () => {
    expect(WINDOW_CONTROLS_WIDTH).toBe(3 * 46);
  });

  test("outside the shell a window action does nothing rather than throw", async () => {
    await windowAction("minimize");
    await windowAction("toggle-maximize");
    await windowAction("close");
  });
});
