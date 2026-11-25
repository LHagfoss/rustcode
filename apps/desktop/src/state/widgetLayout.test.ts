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

describe("parseLayout", () => {
  test("falls back to the default when nothing or garbage is stored", () => {
    expect(sameLayout(parseLayout(null), DEFAULT_LAYOUT)).toBe(true);
    expect(sameLayout(parseLayout("{nope"), DEFAULT_LAYOUT)).toBe(true);
    expect(sameLayout(parseLayout('{"id":"usage"}'), DEFAULT_LAYOUT)).toBe(true);
  });

  test("drops unknown and repeated cards and repairs a bad span", () => {
    const layout = parseLayout(
      JSON.stringify([
        { id: "pulls", span: 4 },
        { id: "gone" },
        { id: "pulls", span: 2 },
        { id: "needs", span: 5 },
      ]),
    );
    expect(layout).toEqual([
      { id: "pulls", span: 4 },
      { id: "needs", span: 3 },
    ]);
  });

  test("keeps an empty overview empty", () => {
    expect(parseLayout("[]")).toEqual([]);
  });
});

describe("arranging", () => {
  test("moves a card to the target's place in either direction", () => {
    expect(ids(moveWidget(DEFAULT_LAYOUT, "month", 2))).toEqual([
      "activity",
      "threads",
      "month",
      "usage",
      "pulls",
    ]);
    expect(ids(moveWidget(DEFAULT_LAYOUT, "activity", 1))).toEqual([
      "threads",
      "activity",
      "usage",
      "pulls",
      "month",
    ]);
    expect(ids(moveWidget(DEFAULT_LAYOUT, "activity", -3))).toEqual(ids([...DEFAULT_LAYOUT]));
    expect(ids(moveWidget(DEFAULT_LAYOUT, "month", 99))).toEqual(ids([...DEFAULT_LAYOUT]));
  });

  test("removes, adds back at the end, and lists what is hidden", () => {
    const without = removeWidget(DEFAULT_LAYOUT, "usage");
    expect(hiddenWidgets(without)).toEqual(["needs", "usage"]);
    const back = addWidget(without, "usage");
    expect(back.at(-1)).toEqual({ id: "usage", span: 2 });
    expect(addWidget(back, "usage")).toHaveLength(back.length);
  });

  test("cycles widths and wraps back to the narrowest", () => {
    expect([nextSpan(2), nextSpan(3), nextSpan(4), nextSpan(6)]).toEqual([3, 4, 6, 2]);
    expect(resizeWidget(DEFAULT_LAYOUT, "usage")[2]).toEqual({ id: "usage", span: 3 });
  });
});
