// design-lint: allow-file (colours sampled from a picture)
// The banner's print effects (Settings › Appearance › New thread banner): the chosen picture
// re-drawn as an ordered dither, a halftone or scanlines. Pure pixel functions, so they run in
// the worker (bannerEffects.worker.ts) and in tests alike; the picture is rendered once per
// effect and theme and kept as a PNG, never per frame or per size (lib/banner.ts).

export type PixelEffect = "dither" | "halftone" | "scanlines";

export const PIXEL_EFFECTS: readonly PixelEffect[] = ["dither", "halftone", "scanlines"];

export function isPixelEffect(value: string): value is PixelEffect {
  return (PIXEL_EFFECTS as readonly string[]).includes(value);
}

/** The longest edge a picture is processed at: a banner never needs more. */
export const MAX_EDGE = 2048;

/** The size a picture is drawn at before its effect: at most {@link MAX_EDGE} on its long edge. */
export function fitWithin(width: number, height: number, max = MAX_EDGE): { width: number; height: number } {
  if (width <= 0 || height <= 0) return { width: 0, height: 0 };
  const scale = Math.min(1, max / Math.max(width, height));
  return { width: Math.max(1, Math.round(width * scale)), height: Math.max(1, Math.round(height * scale)) };
}

/** Relative luminance of an sRGB pixel, 0 to 255. */
export function luma(r: number, g: number, b: number): number {
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** The 4×4 Bayer matrix, thresholds 0 to 15. */
const BAYER = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
/** Levels each channel is dithered to: few enough to read as print, enough to keep the colour. */
const DITHER_LEVELS = 4;

/** An ordered (4×4 Bayer) dither, each channel to {@link DITHER_LEVELS} levels. */
export function dither(src: Uint8ClampedArray, width: number, height: number): Uint8ClampedArray {
  const out = new Uint8ClampedArray(src.length);
  const step = 255 / (DITHER_LEVELS - 1);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const i = (y * width + x) * 4;
      // Centred on zero, so a flat mid-grey dithers to an even mix of its two neighbours.
      const bias = ((BAYER[(y % 4) * 4 + (x % 4)] ?? 0) + 0.5) / 16 - 0.5;
      for (let c = 0; c < 3; c++) {
        const v = (src[i + c] ?? 0) / step + bias;
        out[i + c] = Math.round(Math.min(DITHER_LEVELS - 1, Math.max(0, Math.round(v))) * step);
      }
      out[i + 3] = src[i + 3] ?? 255;
    }
  }
  return out;
}

/** The halftone grid's pitch, px. */
export const HALFTONE_CELL = 4;
/** How much of the plain picture shows through the print, so it still reads as the picture. */
const HALFTONE_SOURCE = 0.3;

/**
 * Dots on a {@link HALFTONE_CELL}px grid, each in its cell's colour and sized by its brightness
 * (by its darkness on a light theme's white paper), on black or white paper.
 */
export function halftone(
  src: Uint8ClampedArray,
  width: number,
  height: number,
  dark: boolean,
): Uint8ClampedArray {
  const out = new Uint8ClampedArray(src.length);
  const paper = dark ? 0 : 255;
  const n = HALFTONE_CELL;
  const centre = (n - 1) / 2;
  for (let cy = 0; cy < height; cy += n) {
    for (let cx = 0; cx < width; cx += n) {
      // The cell's average colour.
      let r = 0;
      let g = 0;
      let b = 0;
      let count = 0;
      const h = Math.min(n, height - cy);
      const w = Math.min(n, width - cx);
      for (let dy = 0; dy < h; dy++) {
        for (let dx = 0; dx < w; dx++) {
          const i = ((cy + dy) * width + cx + dx) * 4;
          r += src[i] ?? 0;
          g += src[i + 1] ?? 0;
          b += src[i + 2] ?? 0;
          count++;
        }
      }
      r /= count;
      g /= count;
      b /= count;
      const l = luma(r, g, b) / 255;
      const ink = dark ? l : 1 - l;
      // Area tracks ink, so the radius is its square root; a full cell's dot just touches the next.
      const radius = Math.sqrt(ink) * (n / 2) * Math.SQRT2;
      for (let dy = 0; dy < h; dy++) {
        for (let dx = 0; dx < w; dx++) {
          const i = ((cy + dy) * width + cx + dx) * 4;
          const distance = Math.hypot(dx - centre, dy - centre);
          // A one-pixel soft edge.
          const cover = Math.min(1, Math.max(0, radius + 0.5 - distance));
          const dot = [r, g, b];
          for (let c = 0; c < 3; c++) {
            const printed = (dot[c] ?? 0) * cover + paper * (1 - cover);
            out[i + c] = (src[i + c] ?? 0) * HALFTONE_SOURCE + printed * (1 - HALFTONE_SOURCE);
          }
          out[i + 3] = src[i + 3] ?? 255;
        }
      }
    }
  }
  return out;
}

/** How far every third row is pulled to the paper colour. */
const SCANLINE_DEPTH = 0.5;

/** Every third row darkened (lightened on a light theme), like a CRT's lines. */
export function scanlines(
  src: Uint8ClampedArray,
  width: number,
  height: number,
  dark: boolean,
): Uint8ClampedArray {
  const out = new Uint8ClampedArray(src);
  const paper = dark ? 0 : 255;
  for (let y = 0; y < height; y += 3) {
    for (let x = 0; x < width; x++) {
      const i = (y * width + x) * 4;
      for (let c = 0; c < 3; c++) {
        const v = src[i + c] ?? 0;
        out[i + c] = v + (paper - v) * SCANLINE_DEPTH;
      }
    }
  }
  return out;
}

/** The effect applied to RGBA pixels; a new array, the source untouched. */
export function applyEffect(
  effect: PixelEffect,
  src: Uint8ClampedArray,
  width: number,
  height: number,
  dark: boolean,
): Uint8ClampedArray {
  if (width <= 0 || height <= 0 || src.length < width * height * 4) return new Uint8ClampedArray(src);
  switch (effect) {
    case "dither":
      return dither(src, width, height);
    case "halftone":
      return halftone(src, width, height, dark);
    case "scanlines":
      return scanlines(src, width, height, dark);
  }
}

/**
 * A print effect (or none), then a baked blur (or none): what the worker, or the page where there
 * is no worker, does to a picture's pixels. A new array; the source untouched.
 */
export function processPixels(
  src: Uint8ClampedArray,
  width: number,
  height: number,
  effect: PixelEffect | null,
  dark: boolean,
  blur: number,
): Uint8ClampedArray {
  const printed = effect ? applyEffect(effect, src, width, height, dark) : new Uint8ClampedArray(src);
  return blur > 0 ? boxBlur(printed, width, height, blur) : printed;
}

// ---------------------------------------------------------------- blur

/**
 * The pixels blurred by about `radius` px (the CSS `blur()` standard deviation): three box blurs
 * each way, which add up to a near-Gaussian. Windows and Linux paint no live blur
 * (docs/design.md › Platforms), so there the banner's blur is baked into its picture once with
 * this instead. A new array; the source untouched.
 */
export function boxBlur(
  src: Uint8ClampedArray,
  width: number,
  height: number,
  radius: number,
): Uint8ClampedArray {
  const r = Math.round(radius);
  if (r <= 0 || width <= 0 || height <= 0 || src.length < width * height * 4)
    return new Uint8ClampedArray(src);
  let a = new Uint8ClampedArray(src);
  let b = new Uint8ClampedArray(src.length);
  for (let pass = 0; pass < 3; pass++) {
    boxPass(a, b, r, 4, width * 4, width, height);
    [a, b] = [b, a];
    boxPass(a, b, r, width * 4, 4, height, width);
    [a, b] = [b, a];
  }
  return a;
}

/**
 * One box blur along lines `length` pixels long, `step` bytes apart, the lines `stride` bytes
 * apart; edges repeat their last pixel.
 */
function boxPass(
  src: Uint8ClampedArray,
  dst: Uint8ClampedArray,
  r: number,
  step: number,
  stride: number,
  length: number,
  lines: number,
) {
  const span = 2 * r + 1;
  const last = length - 1;
  for (let line = 0; line < lines; line++) {
    const base = line * stride;
    for (let c = 0; c < 4; c++) {
      const at = (i: number) => src[base + Math.min(last, Math.max(0, i)) * step + c] ?? 0;
      let sum = 0;
      for (let i = -r; i <= r; i++) sum += at(i);
      for (let i = 0; i < length; i++) {
        dst[base + i * step + c] = sum / span;
        sum += at(i + r + 1) - at(i - r);
      }
    }
  }
}

// ---------------------------------------------------------------- cache

/**
 * One rendered picture per picture, look (a print effect, or how a plain or text picture is
 * shown), theme and baked blur radius.
 */
export function effectKey(name: string, look: string, dark: boolean, blur = 0): string {
  return `${name}\n${look}\n${dark ? "dark" : "light"}${blur > 0 ? `\n${blur}` : ""}`;
}

/** Rendered pictures kept: a theme switch, an effect or two back and a blur or two, no more. */
export const EFFECT_CACHE_LIMIT = 6;

/**
 * Rendered effects by {@link effectKey}, oldest dropped first. A dropped entry's URL is released
 * through `release` (URL.revokeObjectURL in the app), and a new picture drops every entry of the
 * old one, so a changed banner never shows, or keeps, the last picture's renders.
 */
export class EffectCache {
  private readonly entries = new Map<string, string>();

  constructor(
    private readonly release: (url: string) => void,
    private readonly limit = EFFECT_CACHE_LIMIT,
  ) {}

  get(key: string): string | undefined {
    const url = this.entries.get(key);
    if (url !== undefined) {
      // Most recently used last.
      this.entries.delete(key);
      this.entries.set(key, url);
    }
    return url;
  }

  set(key: string, url: string) {
    const old = this.entries.get(key);
    if (old !== undefined && old !== url) this.release(old);
    this.entries.delete(key);
    this.entries.set(key, url);
    while (this.entries.size > this.limit) {
      const [oldest, oldUrl] = this.entries.entries().next().value as [string, string];
      this.entries.delete(oldest);
      this.release(oldUrl);
    }
  }

  /** Drops every render that is not of `name`. */
  keepOnly(name: string) {
    for (const [key, url] of this.entries) {
      if (key.split("\n")[0] !== name) {
        this.entries.delete(key);
        this.release(url);
      }
    }
  }

  get size(): number {
    return this.entries.size;
  }
}
