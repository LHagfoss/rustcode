import { describe, expect, test } from "bun:test";
import {
  addWidget,
  DEFAULT_LAYOUT,
  hiddenWidgets,
  moveWidget,
  nextSpan,
  parseLayout,
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

  test("drops unknown and repeated cards and repairs a bad span", () => {
    expect(
      stored([{ id: "pulls", span: 4 }, { id: "gone" }, { id: "pulls", span: 2 }, { id: "needs", span: 5 }]),
    ).toEqual([
      { id: "pulls", span: 4 },
      { id: "needs", span: 2 },
    ]);
  });

  test("folds the old Usage and This month cards into one Spend card", () => {
    expect(
      stored([
        { id: "month", span: 6 },
        { id: "activity", span: 3 },
        { id: "usage", span: 2 },
      ]),
    ).toEqual([
      { id: "spend", span: 2 },
      { id: "activity", span: 3 },
    ]);
  });

  test("keeps an empty overview empty", () => {
    expect(parseLayout("[]")).toEqual([]);
  });
});

describe("arranging", () => {
  test("moves a card to the target's place in either direction", () => {
    const order = ids([...DEFAULT_LAYOUT]);
    expect(ids(moveWidget(DEFAULT_LAYOUT, "pulls", 2))).toEqual([
      "activity",
      "threads",
      "pulls",
      "spend",
      "needs",
    ]);
    expect(ids(moveWidget(DEFAULT_LAYOUT, "activity", 1))).toEqual([
      "threads",
      "activity",
      "spend",
      "needs",
      "pulls",
    ]);
    expect(ids(moveWidget(DEFAULT_LAYOUT, "activity", -3))).toEqual(order);
    expect(ids(moveWidget(DEFAULT_LAYOUT, "pulls", 99))).toEqual(order);
  });

  test("removes, adds back at the end, and lists what is hidden", () => {
    const without = removeWidget(DEFAULT_LAYOUT, "spend");
    expect(hiddenWidgets(without)).toEqual(["spend", "running"]);
    const back = addWidget(without, "spend");
    expect(back.at(-1)).toEqual({ id: "spend", span: 2 });
    expect(addWidget(back, "spend")).toHaveLength(back.length);
  });

  test("cycles widths and wraps back to the narrowest", () => {
    expect([nextSpan(2), nextSpan(3), nextSpan(4), nextSpan(6)]).toEqual([3, 4, 6, 2]);
    expect(resizeWidget(DEFAULT_LAYOUT, "spend")[2]).toEqual({ id: "spend", span: 3 });
  });
});
