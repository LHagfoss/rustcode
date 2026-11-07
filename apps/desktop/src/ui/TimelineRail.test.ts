import { describe, expect, test } from "bun:test";
import {
  cardBox,
  plainPreview,
  railHeight,
  railHitWidth,
  railIndexAt,
  railLabel,
  railScale,
  rewindOffer,
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

  test("keeps the card out of the strip's hit area", () => {
    for (const width of [800, 900, 1200, 1400]) {
      const hit = railHitWidth(width);
      const box = cardBox(hit);
      // The strip owns [0, hit); the card starts at or after its right edge, so pointing at the card
      // never picks a turn and the card cannot chase the pointer it is under.
      expect(box.left).toBeGreaterThanOrEqual(hit);
      // ... and past the 24px a strip draws, so a narrow gutter does not put the card over it.
      expect(box.left).toBeGreaterThanOrEqual(24);
      expect(box.width).toBe(320);
    }
    expect(cardBox(40).left).toBeGreaterThan(40);
  });

  test("offers the rewind only where the thread can go back", () => {
    const checkpoint = { turn: 3 } as Parameters<typeof rewindOffer>[0];
    expect(rewindOffer(checkpoint, false, { busy: false, keep: true })).toBeUndefined();
    expect(rewindOffer(undefined, true, { busy: false, keep: true })).toBeUndefined();
    const offer = rewindOffer(checkpoint, true, { busy: false, keep: true });
    expect(offer?.turn).toBe(3);
    expect(offer?.disabled).toBe(false);
    // It says what the click does, because a checkpointed engine goes back without a confirm.
    expect(offer?.note).toContain("earlier attempt");
    expect(rewindOffer(checkpoint, true, { busy: false, keep: false })?.note).toContain("no way back");
  });

  test("waits for the turn to stop before a rewind", () => {
    const checkpoint = { turn: 4 } as Parameters<typeof rewindOffer>[0];
    const offer = rewindOffer(checkpoint, true, { busy: true, keep: true });
    expect(offer?.disabled).toBe(true);
    expect(offer?.note).toBe("Stop the turn first");
  });
});
