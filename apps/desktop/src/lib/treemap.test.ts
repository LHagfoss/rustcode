import { describe, expect, test } from "bun:test";
import { layoutTiles, type TreemapRect } from "./treemap";

const area = (rect: TreemapRect) => rect.width * rect.height;
const EPSILON = 1e-6;

function overlaps(a: TreemapRect, b: TreemapRect): boolean {
  return (
    a.x < b.x + b.width - EPSILON &&
    b.x < a.x + a.width - EPSILON &&
    a.y < b.y + b.height - EPSILON &&
    b.y < a.y + a.height - EPSILON
  );
}

describe("the context tile map", () => {
  test("a single category fills the whole box", () => {
    expect(layoutTiles([{ key: "tools", value: 10 }], 120, 80)).toEqual([
      { key: "tools", x: 0, y: 0, width: 120, height: 80 },
    ]);
  });

  test("tiles cover the box exactly, proportional to their tokens", () => {
    const items = [
      { key: "a", value: 50 },
      { key: "b", value: 30 },
      { key: "c", value: 20 },
    ];
    const rects = layoutTiles(items, 200, 100);
    expect(rects).toHaveLength(3);
    expect(rects.reduce((total, rect) => total + area(rect), 0)).toBeCloseTo(200 * 100, 6);
    for (const item of items) {
      const rect = rects.find((candidate) => candidate.key === item.key);
      expect(rect).toBeDefined();
      expect(area(rect!)).toBeCloseTo((item.value / 100) * 200 * 100, 6);
    }
  });

  test("no two tiles overlap and none leaves the box", () => {
    const items = [42, 17, 12, 9, 6, 4, 3, 2, 1].map((value, index) => ({
      key: `c${index}`,
      value,
    }));
    const rects = layoutTiles(items, 320, 160);
    expect(rects).toHaveLength(items.length);
    for (const rect of rects) {
      expect(rect.x).toBeGreaterThanOrEqual(-EPSILON);
      expect(rect.y).toBeGreaterThanOrEqual(-EPSILON);
      expect(rect.x + rect.width).toBeLessThanOrEqual(320 + EPSILON);
      expect(rect.y + rect.height).toBeLessThanOrEqual(160 + EPSILON);
    }
    for (let i = 0; i < rects.length; i += 1) {
      for (let j = i + 1; j < rects.length; j += 1) {
        expect(overlaps(rects[i]!, rects[j]!)).toBe(false);
      }
    }
  });

  test("a gap separates neighbours without pushing a tile out", () => {
    const rects = layoutTiles(
      [
        { key: "a", value: 2 },
        { key: "b", value: 1 },
      ],
      100,
      100,
      4,
    );
    for (const rect of rects) {
      expect(rect.x).toBeGreaterThanOrEqual(2);
      expect(rect.y).toBeGreaterThanOrEqual(2);
      expect(rect.x + rect.width).toBeLessThanOrEqual(98);
      expect(rect.y + rect.height).toBeLessThanOrEqual(98);
    }
  });

  test("empty, zero and invalid input take no room", () => {
    expect(layoutTiles([], 100, 100)).toEqual([]);
    expect(layoutTiles([{ key: "zero", value: 0 }], 100, 100)).toEqual([]);
    expect(
      layoutTiles(
        [
          { key: "nan", value: Number.NaN },
          { key: "negative", value: -4 },
        ],
        100,
        100,
      ),
    ).toEqual([]);
    expect(layoutTiles([{ key: "a", value: 1 }], 0, 100)).toEqual([]);
  });

  test("equal values keep the order they were given", () => {
    const rects = layoutTiles(
      [
        { key: "first", value: 1 },
        { key: "second", value: 1 },
      ],
      100,
      100,
    );
    expect(rects.map((rect) => rect.key)).toEqual(["first", "second"]);
  });
});
