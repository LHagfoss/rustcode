import { describe, expect, test } from "bun:test";
import { clampZoom, ZOOM_MAX, ZOOM_MIN } from "./zoom";

describe("text zoom", () => {
  test("clamps to 80–150% in tenths", () => {
    expect(clampZoom(1.04)).toBe(1);
    expect(clampZoom(1.16)).toBe(1.2);
    expect(clampZoom(3)).toBe(ZOOM_MAX);
    expect(clampZoom(0.1)).toBe(ZOOM_MIN);
    expect(clampZoom(Number.NaN)).toBe(1);
  });
});
