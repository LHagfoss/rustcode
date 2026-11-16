import { describe, expect, test } from "bun:test";
import {
  applyEffect,
  boxBlur,
  dither,
  EffectCache,
  effectKey,
  fitWithin,
  HALFTONE_CELL,
  halftone,
  isPixelEffect,
  scanlines,
} from "./bannerEffects";

function solid(width: number, height: number, rgb: [number, number, number]) {
  const px = new Uint8ClampedArray(width * height * 4);
  for (let i = 0; i < px.length; i += 4) px.set([...rgb, 255], i);
  return px;
}

const at = (px: Uint8ClampedArray, width: number, x: number, y: number) =>
  Array.from(px.subarray((y * width + x) * 4, (y * width + x) * 4 + 4));

describe("fitWithin", () => {
  test("downscales the long edge to the limit, keeping the shape", () => {
    expect(fitWithin(4096, 2048)).toEqual({ width: 2048, height: 1024 });
    expect(fitWithin(1000, 3000, 300)).toEqual({ width: 100, height: 300 });
  });
  test("never upscales, and an empty picture stays empty", () => {
    expect(fitWithin(640, 480)).toEqual({ width: 640, height: 480 });
    expect(fitWithin(0, 10)).toEqual({ width: 0, height: 0 });
  });
});

describe("dither", () => {
  test("black and white stay as they are", () => {
    expect(at(dither(solid(4, 4, [0, 0, 0]), 4, 4), 4, 1, 2)).toEqual([0, 0, 0, 255]);
    expect(at(dither(solid(4, 4, [255, 255, 255]), 4, 4), 4, 3, 3)).toEqual([255, 255, 255, 255]);
  });
  test("a grey between two levels becomes a pattern of both, averaging near the grey", () => {
    const out = dither(solid(4, 4, [128, 128, 128]), 4, 4);
    const values = new Set<number>();
    let sum = 0;
    for (let i = 0; i < out.length; i += 4) {
      values.add(out[i] ?? 0);
      sum += out[i] ?? 0;
    }
    expect(values.size).toBe(2);
    expect(Math.abs(sum / 16 - 128)).toBeLessThan(16);
  });
});

describe("halftone", () => {
  test("a bright cell prints a big dot on black, a dark cell almost none", () => {
    const n = HALFTONE_CELL;
    const bright = halftone(solid(n, n, [255, 255, 255]), n, n, true);
    const dim = halftone(solid(n, n, [10, 10, 10]), n, n, true);
    // The centre is inked, the corner of a dark cell is the black paper (plus the source's share).
    expect(at(bright, n, 1, 1)[0]).toBe(255);
    expect(at(dim, n, 0, 0)[0]).toBeLessThan(10);
  });
  test("on white paper a dark cell is the big dot", () => {
    const n = HALFTONE_CELL;
    const out = halftone(solid(n, n, [0, 0, 0]), n, n, false);
    expect(at(out, n, 1, 1)[0]).toBe(0);
  });
  test("keeps alpha and handles a size that is not a whole number of cells", () => {
    const out = halftone(solid(5, 3, [200, 100, 50]), 5, 3, true);
    expect(out.length).toBe(5 * 3 * 4);
    expect(at(out, 5, 4, 2)[3]).toBe(255);
  });
});

describe("scanlines", () => {
  test("every third row is darkened on dark, lightened on light, the rest untouched", () => {
    const src = solid(2, 6, [100, 100, 100]);
    const dark = scanlines(src, 2, 6, true);
    expect(at(dark, 2, 0, 0)[0]).toBe(50);
    expect(at(dark, 2, 0, 1)[0]).toBe(100);
    expect(at(dark, 2, 0, 3)[0]).toBe(50);
    const light = scanlines(src, 2, 6, false);
    expect(at(light, 2, 1, 0)[0]).toBe(178);
    expect(at(light, 2, 1, 2)[0]).toBe(100);
  });
});

test("applyEffect returns a new array and leaves the source alone", () => {
  const src = solid(3, 3, [128, 64, 32]);
  const copy = new Uint8ClampedArray(src);
  for (const effect of ["dither", "halftone", "scanlines"] as const) {
    const out = applyEffect(effect, src, 3, 3, true);
    expect(out).not.toBe(src);
    expect(out.length).toBe(src.length);
  }
  expect(src).toEqual(copy);
  // Too few pixels for the size: a plain copy rather than a read past the end.
  expect(applyEffect("dither", src, 10, 10, true)).toEqual(copy);
});

test("only the worker's effects are pixel effects", () => {
  expect(isPixelEffect("halftone")).toBe(true);
  expect(isPixelEffect("ascii")).toBe(false);
  expect(isPixelEffect("haze")).toBe(false);
});

describe("EffectCache", () => {
  test("keys by picture, effect and theme", () => {
    expect(effectKey("a", "dither", true)).not.toBe(effectKey("a", "dither", false));
    expect(effectKey("a", "dither", true)).not.toBe(effectKey("b", "dither", true));
    expect(effectKey("a", "dither", true)).not.toBe(effectKey("a", "halftone", true));
  });

  test("drops the least recently used past its limit and releases its URL", () => {
    const released: string[] = [];
    const cache = new EffectCache((url) => released.push(url), 2);
    cache.set("a", "blob:a");
    cache.set("b", "blob:b");
    expect(cache.get("a")).toBe("blob:a");
    cache.set("c", "blob:c");
    expect(released).toEqual(["blob:b"]);
    expect(cache.get("b")).toBeUndefined();
    expect(cache.size).toBe(2);
  });

  test("replacing an entry releases the old URL", () => {
    const released: string[] = [];
    const cache = new EffectCache((url) => released.push(url));
    cache.set("a", "blob:1");
    cache.set("a", "blob:2");
    expect(released).toEqual(["blob:1"]);
    expect(cache.get("a")).toBe("blob:2");
  });

  test("a new picture drops every render of the old one", () => {
    const released: string[] = [];
    const cache = new EffectCache((url) => released.push(url));
    cache.set(effectKey("old", "dither", true), "blob:1");
    cache.set(effectKey("old", "halftone", false), "blob:2");
    cache.set(effectKey("new", "dither", true), "blob:3");
    cache.keepOnly("new");
    expect(released.sort()).toEqual(["blob:1", "blob:2"]);
    expect(cache.size).toBe(1);
    expect(cache.get(effectKey("new", "dither", true))).toBe("blob:3");
  });
});

describe("boxBlur", () => {
  const stripe = () => {
    // 5×1: one white pixel between black ones.
    const px = new Uint8ClampedArray(5 * 4);
    for (let i = 0; i < 5; i++) px.set(i === 2 ? [255, 255, 255, 255] : [0, 0, 0, 255], i * 4);
    return px;
  };

  test("leaves the pixels alone at radius 0", () => {
    const src = stripe();
    expect(Array.from(boxBlur(src, 5, 1, 0))).toEqual(Array.from(src));
  });

  test("spreads a bright pixel to its neighbours and keeps the source", () => {
    const src = stripe();
    const out = boxBlur(src, 5, 1, 1);
    expect(out[2 * 4]).toBeLessThan(255);
    expect(out[1 * 4]).toBeGreaterThan(0);
    expect(out[3]).toBe(255);
    expect(src[2 * 4]).toBe(255);
  });

  test("keeps a flat colour flat, edges included", () => {
    const src = new Uint8ClampedArray(4 * 3 * 4).fill(120);
    expect(Array.from(boxBlur(src, 4, 3, 2)).every((v) => v === 120)).toBe(true);
  });

  test("keys a baked blur apart from none", () => {
    expect(effectKey("a", "picture", true, 3)).not.toBe(effectKey("a", "picture", true));
    expect(effectKey("a", "dither", true, 0)).toBe(effectKey("a", "dither", true));
  });
});
