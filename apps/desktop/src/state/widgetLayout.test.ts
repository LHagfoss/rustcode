import { describe, expect, test } from "bun:test";
import {
  addWidget,
  cardHeight,
  DEFAULT_LAYOUT,
  fittedRowHeight,
  hiddenWidgets,
  MAX_ROW_HEIGHT,
  MIN_ROW_HEIGHT,
  moveWidget,
  nextSize,
  packLayout,
  parseLayout,
  ROW_HEIGHT,
  removeWidget,
  resizeWidget,
  sameLayout,
} from "./widgetLayout";

const ids = (layout: { id: string }[]) => layout.map((slot) => slot.id);
const stored = (value: unknown) => parseLayout(JSON.stringify(value));

describe("parseLayout", () => {
  test("falls back to the default when nothing or garbage is stored", () => {
    expect(sameLayout(parseLayout(null), DEFAULT_LAYOUT)).toBe(true);
    expect(sameLayout(parseLayout("{nope"), DEFAULT_LAYOUT)).toBe(true);
    expect(sameLayout(stored({ id: "spend" }), DEFAULT_LAYOUT)).toBe(true);
  });

  test("drops unknown and repeated cards and gives a bad size the card's own", () => {
    expect(
      stored([
        { id: "pulls", w: 1, h: 2 },
        { id: "gone" },
        { id: "pulls", w: 2, h: 2 },
        { id: "needs", w: 3, h: 1 },
        { id: "threads", span: 3 },
      ]),
    ).toEqual([
      { id: "pulls", w: 1, h: 2 },
      { id: "needs", w: 1, h: 1 },
      { id: "threads", w: 1, h: 2 },
    ]);
  });

  test("folds the old Usage and This month cards into one Spend card", () => {
    expect(stored([{ id: "month", w: 1, h: 1 }, { id: "activity" }, { id: "usage" }])).toEqual([
      { id: "spend", w: 2, h: 1 },
      { id: "activity", w: 2, h: 2 },
    ]);
  });

  test("keeps an empty overview empty", () => {
    expect(parseLayout("[]")).toEqual([]);
  });
});

describe("arranging", () => {
  test("moves a card to the target's place in either direction", () => {
    const order = ids([...DEFAULT_LAYOUT]);
    expect(ids(moveWidget(DEFAULT_LAYOUT, "pulls", 1))).toEqual([
      "activity",
      "pulls",
      "threads",
      "needs",
      "running",
      "spend",
    ]);
    expect(ids(moveWidget(DEFAULT_LAYOUT, "activity", 2))).toEqual([
      "threads",
      "needs",
      "activity",
      "running",
      "spend",
      "pulls",
    ]);
    expect(ids(moveWidget(DEFAULT_LAYOUT, "activity", -3))).toEqual(order);
    expect(ids(moveWidget(DEFAULT_LAYOUT, "pulls", 99))).toEqual(order);
  });

  test("removes, adds back at the end in its own size, and lists what is hidden", () => {
    const without = removeWidget(DEFAULT_LAYOUT, "spend");
    expect(hiddenWidgets(without)).toEqual(["spend"]);
    const back = addWidget(without, "spend");
    expect(back.at(-1)).toEqual({ id: "spend", w: 2, h: 1 });
    expect(addWidget(back, "spend")).toHaveLength(back.length);
  });

  test("steps through every size, wide before tall, and wraps", () => {
    let slot = nextSize({ id: "needs", w: 1, h: 1 });
    const sizes = [slot];
    for (let i = 0; i < 3; i++) {
      slot = nextSize(slot);
      sizes.push(slot);
    }
    expect(sizes.map((slot) => `${slot.w}x${slot.h}`)).toEqual(["2x1", "1x2", "2x2", "1x1"]);
    expect(resizeWidget(DEFAULT_LAYOUT, "needs")[2]).toEqual({ id: "needs", w: 2, h: 1 });
  });
});

describe("packing", () => {
  test("the default fills three rows of four with no hole", () => {
    expect(packLayout(DEFAULT_LAYOUT)).toEqual({
      rows: 3,
      places: [
        { id: "activity", row: 0, column: 0 },
        { id: "threads", row: 0, column: 2 },
        { id: "needs", row: 0, column: 3 },
        { id: "running", row: 1, column: 3 },
        { id: "spend", row: 2, column: 0 },
        { id: "pulls", row: 2, column: 2 },
      ],
    });
  });

  test("a small card later in the order fills a hole a wide one left", () => {
    const { places } = packLayout([
      { id: "spend", w: 1, h: 1 },
      { id: "activity", w: 2, h: 1 },
      { id: "pulls", w: 2, h: 1 },
      { id: "needs", w: 1, h: 1 },
    ]);
    expect(places.find((place) => place.id === "needs")).toEqual({ id: "needs", row: 0, column: 3 });
    expect(packLayout([]).rows).toBe(0);
  });

  test("rows share the room between the least and the most height, and a tall card spans the gap", () => {
    expect(fittedRowHeight(444, 3)).toBe(142);
    expect(fittedRowHeight(2000, 2)).toBe(MAX_ROW_HEIGHT);
    expect(fittedRowHeight(200, 3)).toBe(MIN_ROW_HEIGHT);
    expect(fittedRowHeight(500, 0)).toBe(ROW_HEIGHT);
    expect(cardHeight(142, 2)).toBe(292);
  });
});
