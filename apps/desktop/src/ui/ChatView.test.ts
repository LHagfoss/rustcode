// The chat lane beside the right panel (ui/ChatView.tsx `laneLayout`): centred
// in the room an open card leaves, narrowed only when that room is below the
// lane's own cap, and never drawn under the card.

import { describe, expect, test } from "bun:test";
import { laneLayout } from "./ChatView";
import { DETAILS_CARD } from "./RightPanel";

const LANE_MAX = 52 * 16;

/** The lane's edges once it is placed, from the left of the column. */
function edges(pane: number, card: number, laneMax = LANE_MAX) {
  const { shift, cap } = laneLayout(pane, card, laneMax);
  const width = Math.min(cap ?? laneMax, pane);
  const left = (pane - width) / 2 - shift;
  return { left, right: left + width, width, shift, cap };
}

describe("the lane beside a card", () => {
  test("is the pane's own lane when no card is open", () => {
    expect(laneLayout(1250, 0)).toEqual({ shift: 0, cap: null });
    expect(laneLayout(0, 460)).toEqual({ shift: 0, cap: null });
  });

  test("centres in the room the card leaves", () => {
    // The details card's 280px and its two gaps leave 946px, over the 832px lane.
    const { left, right, shift, cap } = edges(1250, DETAILS_CARD.width);
    expect(cap).toBeNull();
    expect(shift).toBe(152);
    expect(left).toBe((1250 - LANE_MAX) / 2 - 152);
    expect(right).toBe(left + LANE_MAX);
    // Centred in the 946px room, not in the 1250px column.
    expect(left + right).toBe(2 * (946 / 2));
  });

  test("narrows to the room when the room is below its own cap", () => {
    const { width, right, cap } = edges(1250, 460);
    expect(cap).toBe(1250 - 460 - 24);
    expect(width).toBe(766);
    // Centred in the room: 12px or more clear of the card's left edge.
    expect(right).toBeLessThanOrEqual(1250 - 460 - DETAILS_CARD.gap);
  });

  test("never runs under the card at any width that keeps a lane", () => {
    for (let pane = 640; pane <= 1600; pane += 40) {
      for (const card of [320, 460, 760]) {
        const { right } = edges(pane, card);
        if (pane - card - 24 >= 320) expect(right).toBeLessThanOrEqual(pane - card - DETAILS_CARD.gap);
      }
    }
  });

  test("a column too narrow for both has the card float over the chat", () => {
    // 644px leaves 160px beside a 460px card: no lane that reads, so it stays whole.
    const { shift, cap, width } = edges(644, 460);
    expect(cap).toBeNull();
    expect(shift).toBe(0);
    expect(width).toBe(644);
  });

  test("a full-width lane is the room beside the card", () => {
    // "Full" hands the lane the pane: cap the width it is measured against.
    const { width, right, cap } = edges(1000, 460, 1000);
    expect(cap).toBe(516);
    expect(width).toBe(516);
    expect(right).toBeLessThanOrEqual(1000 - 460 - DETAILS_CARD.gap);
  });
});
