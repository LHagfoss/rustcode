import { describe, expect, test } from "bun:test";
import { zoomAt } from "./Lightbox";

describe("lightbox zoom", () => {
  test("keeps the point under the pointer still", () => {
    const v = zoomAt({ zoom: 1, x: 0, y: 0 }, 2, 100, 50);
    expect(v.x + (100 / 1) * 0 + ((100 - v.x) / v.zoom) * v.zoom).toBe(100);
    expect(v).toEqual({ zoom: 2, x: -100, y: -50 });
  });
  test("clamps and recentres at 1×", () => {
    expect(zoomAt({ zoom: 2, x: -100, y: -50 }, 0.5, 0, 0)).toEqual({ zoom: 1, x: 0, y: 0 });
    expect(zoomAt({ zoom: 4, x: 0, y: 0 }, 100, 0, 0).zoom).toBe(8);
  });
});
