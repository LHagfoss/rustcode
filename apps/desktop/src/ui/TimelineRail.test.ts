import { describe, expect, test } from "bun:test";
import {
  plainPreview,
  railHeight,
  railHitWidth,
  railIndexAt,
  railLabel,
  railScale,
  turnTitle,
} from "./TimelineRail";

describe("the timeline minimap", () => {
  test("spaces strips naturally, then squeezes them into the view", () => {
    expect(railHeight(5, 800)).toBe(32);
    expect(railHeight(500, 800)).toBe(720);
  });

  test("maps a pointer to the nearest turn, clamped to the strip", () => {
    expect(railIndexAt(0, 80, 11)).toBe(0);
    expect(railIndexAt(43, 80, 11)).toBe(5);
    expect(railIndexAt(-20, 80, 11)).toBe(0);
    expect(railIndexAt(200, 80, 11)).toBe(10);
  });

  test("widens strips by distance from the hovered one", () => {
    expect(railScale(0)).toBe(1);
    expect(railScale(1)).toBeGreaterThan(railScale(2));
    expect(railScale(null)).toBe(railScale(7));
  });

  test("has no hit area without a gutter", () => {
    expect(railHitWidth(768)).toBe(0);
    expect(railHitWidth(800)).toBe(4);
    expect(railHitWidth(1400)).toBe(40);
  });

  test("names a turn's checkpoint in the rail's own label", () => {
    expect(railLabel("Fix the rail", undefined, false)).toBe("Jump to message: Fix the rail");
    expect(railLabel("Fix the rail", 3, false)).toBe("Jump to message: Fix the rail, checkpoint turn 3");
    expect(railLabel("Fix the rail", 4, true)).toBe(
      "Jump to message: Fix the rail, checkpoint turn 4, current",
    );
  });

  test("previews read as words", () => {
    expect(plainPreview("## Title\n\n- **bold** `code` [link](http://x)\n```ts\nlet a;\n```\nend")).toBe(
      "Title bold code link end",
    );
    expect(turnTitle("   ")).toBe("Message");
    expect(turnTitle("a b c")).toBe("a b c");
  });
});
