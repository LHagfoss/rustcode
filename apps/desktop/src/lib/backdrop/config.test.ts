import { describe, expect, test } from "bun:test";
import { hueOf, parseHex } from "../../ui/LiveBackdrop";
import {
  LIVE_BACKDROP_DEFAULT,
  LIVE_PRESETS,
  liveCell,
  normalizeLiveBackdrop,
  sameLiveBackdrop,
} from "./config";

describe("live background setting", () => {
  test("anything unreadable is the default", () => {
    expect(normalizeLiveBackdrop(null)).toEqual(LIVE_BACKDROP_DEFAULT);
    expect(normalizeLiveBackdrop({ pattern: "nope", amount: "x", colour: 3 })).toEqual(LIVE_BACKDROP_DEFAULT);
  });

  test("numbers are clamped and rounded", () => {
    const c = normalizeLiveBackdrop({ amount: 140.6, threadAmount: -4, focus: { x: 2, y: 0.12345 } });
    expect(c.amount).toBe(100);
    expect(c.threadAmount).toBe(0);
    expect(c.focus).toEqual({ x: 1, y: 0.123 });
  });

  test("every preset is a valid, distinct setting", () => {
    for (const p of LIVE_PRESETS) expect(normalizeLiveBackdrop(p.config)).toEqual(p.config);
    const distinct = LIVE_PRESETS.filter((p, i) =>
      LIVE_PRESETS.some((q, j) => j !== i && sameLiveBackdrop(p.config, q.config)),
    );
    expect(distinct).toEqual([]);
  });

  test("detail maps to cell size", () => {
    expect(liveCell(100)).toBe(3);
    expect(liveCell(0)).toBe(14);
  });
});

describe("theme colours", () => {
  test("hex colours parse in every length", () => {
    expect(parseHex("#fff")).toEqual([255, 255, 255]);
    expect(parseHex("#0b0b0c")).toEqual([11, 11, 12]);
    expect(parseHex("#93a3e84d")).toEqual([147, 163, 232]);
    expect(parseHex("rgb(1 2 3)")).toBeNull();
  });

  test("the accent's hue", () => {
    expect(hueOf([255, 0, 0]).hue).toBe(0);
    expect(hueOf([0, 0, 255]).hue).toBe(240);
    expect(hueOf([10, 10, 10]).sat).toBe(0);
  });
});
