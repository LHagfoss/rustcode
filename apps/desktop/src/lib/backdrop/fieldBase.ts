// design-lint: allow-file (the backdrop's palettes are light, not interface colour)
// Ported from Sable (https://github.com/Leonxlnx/sable, src/art/field.ts), MIT License,
// Copyright (c) 2026 Leonxlnx. Types, palettes, noise tables and the constants the field reads.

import type { Field } from "./field";

/**
 * The light field behind Sable's surfaces: evaluated on the CPU from two precomputed noise
 * tables (no WebGL), drawn in one of two textures:
 *
 * - `dither`: one pixel per cell, ordered-dithered to a few levels and scaled up without
 *   smoothing. Large stages get coarser cells so a frame stays under 24k cells.
 * - `ascii`: the same field on a glyph grid, one Geist Mono character per cell picked from
 *   a brightness ramp and tinted with the cell's colour. Glyphs are blitted from a small white
 *   atlas into a persistent mask, and only cells whose glyph changed are redrawn; colour is
 *   applied in one composite pass.
 * - `halftone`: the glyph machinery with a ramp of round dots (drawn here) on a square grid.
 * - `soft`: no texture: the dither renderer without quantization, scaled up smoothly. With an
 *   image it is the image itself, cover-fitted and dimmed (`softImageRenderer`).
 *
 * An image (`FieldEngine.setImage`) replaces the pattern: its brightness drives every texture,
 * coloured by the palette or by its own colours. `Look.spot` confines the light around a point
 * with a smooth falloff applied before quantization, so its edge is dithered like the rest.
 *
 * This module has no DOM dependencies, so it runs both in `field.worker.ts` on an
 * OffscreenCanvas (the normal path) and on the main thread as a fallback. `FieldEngine` owns
 * the clock and frame loop; whoever hosts it decides when it runs.
 */

export type DitherVariant = "streaks" | "aurora" | "streak" | "bloom" | "tubes" | "tiles";
export type DitherPalette = "ember" | "iris" | "forest" | "mint" | "amber" | "spectrum" | "mono" | "custom";
export type DitherTexture = "dither" | "ascii" | "halftone" | "soft";

export interface Look {
  /**
   * Confines the light around a point: [x, y] 0..1 across the stage, and reach 0..1 (1 = even,
   * no falloff). Absent means no falloff beyond the variant's own.
   */
  spot?: [number, number, number] | null;
  /** Direction of streaks and of the sweep / tubes' wave, in degrees. Absent: the built-in drift. */
  angle?: number;
  /** The 'custom' palette: hue 0..360 and saturation 0..1. */
  hue?: number;
  sat?: number;
  /** With an image: its own colours instead of the palette. */
  imageColour?: boolean;
  /** With an image: the variant's moving pattern played as light over it, 0..1 (0 or absent: the image alone). */
  overlay?: number;
  variant: DitherVariant;
  palette: DitherPalette;
  focus: [number, number];
  gain: number;
  speed: number;
  seed: number;
  /** A full-bleed backdrop: even light edge to edge, darkened softly behind the centre column, capped well below white. */
  full: boolean;
  /** Where the pointer is, -0.5..0.5 across the field; the vanishing point eases a few percent toward it. */
  pointer?: [number, number] | null;
  /** How strongly beats (send, done, fail) show on this field: 0 none, 1 full; above 1 lifts a dim field's beats. */
  react?: number;
  /**
   * The pane colour the field melts into (RustCode: the theme's `--rc-bg`). On a light ground the
   * ramp runs the other way: the ground, a pale tint, the light and the deep body, so the light
   * reads as colour on paper instead of glowing. Absent: Sable's near-black.
   */
  ground?: RGB;
}

/**
 * A one-shot disturbance that travels through the cells:
 * - `send`: a pressure wave leaves the point: a thin bright crest that bends the pattern outward,
 *   a darker trough behind it, a weaker echo, and a short flash where it started.
 * - `pulse`: the same wave, softer (onboarding steps, sign-in, the ultra edge).
 * - `done`: the reverse: a soft ring closes in on the point and settles there.
 * - `fail`: the light dips, spreading out from the point, and recovers.
 */
export type FieldBeat = "send" | "pulse" | "done" | "fail";

/** What the host measured: CSS size, device pixel ratio, and the requested cell size (dither). */
export interface Metrics {
  width: number;
  height: number;
  dpr: number;
  cell: number;
}

export type RGB = [number, number, number];

export const hex = (h: string): RGB => [
  parseInt(h.slice(1, 3), 16),
  parseInt(h.slice(3, 5), 16),
  parseInt(h.slice(5, 7), 16),
];

/** shadow → body → light → hot */
export const PALETTES: Record<DitherPalette, [RGB, RGB, RGB, RGB]> = {
  // The shadow stop is the stage colour (--sheet), so the field melts into the surface.
  ember: [hex("#0c0c0e"), hex("#7a2608"), hex("#f2702e"), hex("#ffe2bc")],
  iris: [hex("#0c0c0e"), hex("#262c84"), hex("#6c88ff"), hex("#e2ebff")],
  // Sunlit foliage: deep green through yellow-green to pale cream.
  forest: [hex("#0c0c0e"), hex("#174a1e"), hex("#9cc23e"), hex("#f3efc8")],
  mint: [hex("#0c0c0e"), hex("#0b6444"), hex("#45dc98"), hex("#e4ffec")],
  // --amber: waiting on you.
  amber: [hex("#0c0c0e"), hex("#7a5206"), hex("#ecc064"), hex("#fff3d6")],
  spectrum: [hex("#0c0c0e"), hex("#3a1a66"), hex("#e0649e"), hex("#ffe6c8")],
  mono: [hex("#0c0c0e"), hex("#2c2c30"), hex("#909096"), hex("#f2f2f0")],
  // Replaced by customPalette() from the look's hue and saturation.
  custom: [hex("#0c0c0e"), hex("#2c2c30"), hex("#909096"), hex("#f2f2f0")],
};

export function hsl(h: number, s: number, l: number): RGB {
  const k = (n: number) => (n + h / 30) % 12;
  const a = s * Math.min(l, 1 - l);
  const f = (n: number) => Math.round((l - a * Math.max(-1, Math.min(k(n) - 3, 9 - k(n), 1))) * 255);
  return [f(0), f(8), f(4)];
}

export let customKey = "";
export let customStops: [RGB, RGB, RGB, RGB] = PALETTES.custom;
/** A palette from one hue: the same shadow, a deep body, a clear light and a pale, less saturated hot end. */
export function customPalette(hue = 28, sat = 0.7): [RGB, RGB, RGB, RGB] {
  const key = `${hue}/${sat}`;
  if (key !== customKey) {
    customKey = key;
    customStops = [PALETTES.mono[0], hsl(hue, sat * 0.9, 0.2), hsl(hue, sat, 0.56), hsl(hue, sat * 0.5, 0.9)];
  }
  return customStops;
}

const basePalette = (s: Look) => (s.palette === "custom" ? customPalette(s.hue, s.sat) : PALETTES[s.palette]);

const mixRGB = (a: RGB, b: RGB, t: number): RGB => [
  a[0] + (b[0] - a[0]) * t,
  a[1] + (b[1] - a[1]) * t,
  a[2] + (b[2] - a[2]) * t,
];

let groundKey = "";
let groundStops: [RGB, RGB, RGB, RGB] = PALETTES.mono;
/** The palette over the look's ground (see `Look.ground`), cached for the last palette and ground. */
export const paletteOf = (s: Look): [RGB, RGB, RGB, RGB] => {
  const p = basePalette(s);
  const g = s.ground;
  if (!g) return p;
  const key = `${s.palette}/${s.hue}/${s.sat}/${g.join(",")}`;
  if (key === groundKey) return groundStops;
  groundKey = key;
  const light = (g[0] * 0.2126 + g[1] * 0.7152 + g[2] * 0.0722) / 255 > 0.5;
  groundStops = light ? [g, mixRGB(g, p[2], 0.35), p[2], p[1]] : [g, p[1], p[2], p[3]];
  return groundStops;
};

/** An image as the field samples it: cover-fitted to the stage, brightness and colour per pixel (0..1). */
export interface ImagePixels {
  w: number;
  h: number;
  aspect: number;
  lum: Float32Array;
  rgb: Float32Array;
}

/** Rows an image is sampled at; finer than any texture's rows, so cells never see its pixels. */
export const IMAGE_ROWS = 240;

export function rasterize(
  src: CanvasImageSource & { width: number; height: number },
  aspect: number,
): ImagePixels | null {
  const h = IMAGE_ROWS;
  const w = Math.max(2, Math.min(720, Math.round(h * aspect)));
  const c = scratch(w, h);
  const ctx = context(c, { willReadFrequently: true });
  if (!ctx || !src.width || !src.height) return null;
  // Cover: the image fills the stage, cropped evenly on the long side.
  const k = Math.max(w / src.width, h / src.height);
  const dw = src.width * k;
  const dh = src.height * k;
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(src, (w - dw) / 2, (h - dh) / 2, dw, dh);
  const d = ctx.getImageData(0, 0, w, h).data;
  const lum = new Float32Array(w * h);
  const rgb = new Float32Array(w * h * 3);
  for (let i = 0, o = 0; i < w * h; i++, o += 4) {
    const r = d[o]! / 255;
    const g = d[o + 1]! / 255;
    const b = d[o + 2]! / 255;
    rgb[i * 3] = r;
    rgb[i * 3 + 1] = g;
    rgb[i * 3 + 2] = b;
    lum[i] = r * 0.2126 + g * 0.7152 + b * 0.0722;
  }
  return { w, h, aspect, lum, rgb };
}

// ── noise tables (built once per thread, shared by every field) ──

export const N = 256;
export const MASK = N - 1;
export let tables: [Float32Array, Float32Array] | null = null;

export function buildNoise(seed: number): Float32Array {
  // Tileable value-noise fbm: each octave is a wrapped lattice, so the table repeats seamlessly.
  let s = seed >>> 0;
  const rand = () => {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    return s / 4294967296;
  };
  const out = new Float32Array(N * N);
  let amp = 0.5;
  let total = 0;
  for (let period = 4; period <= 64; period *= 2) {
    const lattice = new Float32Array(period * period);
    for (let i = 0; i < lattice.length; i++) lattice[i] = rand();
    const scale = period / N;
    for (let y = 0; y < N; y++) {
      const fy = y * scale;
      const y0 = Math.floor(fy);
      const ty = fy - y0;
      const sy = ty * ty * (3 - 2 * ty);
      const ya = (y0 % period) * period;
      const yb = ((y0 + 1) % period) * period;
      for (let x = 0; x < N; x++) {
        const fx = x * scale;
        const x0 = Math.floor(fx);
        const tx = fx - x0;
        const sx = tx * tx * (3 - 2 * tx);
        const xa = x0 % period;
        const xb = (x0 + 1) % period;
        const top = lattice[ya + xa]! + (lattice[ya + xb]! - lattice[ya + xa]!) * sx;
        const bot = lattice[yb + xa]! + (lattice[yb + xb]! - lattice[yb + xa]!) * sx;
        out[y * N + x]! += (top + (bot - top) * sy) * amp;
      }
    }
    total += amp;
    amp *= 0.5;
  }
  for (let i = 0; i < out.length; i++) out[i]! /= total;
  return out;
}

export function getTables() {
  if (!tables) tables = [buildNoise(7), buildNoise(1913)];
  return tables;
}

/** Bilinear sample; one unit of (x, y) spans the whole table. */
export function sample(tex: Float32Array, x: number, y: number): number {
  const fx = x * N;
  const fy = y * N;
  const x0 = Math.floor(fx);
  const y0 = Math.floor(fy);
  const tx = fx - x0;
  const ty = fy - y0;
  const xa = x0 & MASK;
  const xb = (x0 + 1) & MASK;
  const ya = (y0 & MASK) * N;
  const yb = ((y0 + 1) & MASK) * N;
  const top = tex[ya + xa]! + (tex[ya + xb]! - tex[ya + xa]!) * tx;
  const bot = tex[yb + xa]! + (tex[yb + xb]! - tex[yb + xa]!) * tx;
  return top + (bot - top) * ty;
}

export const BAYER = (() => {
  const m = new Float32Array(64);
  const b2 = [0, 2, 3, 1];
  for (let y = 0; y < 8; y++)
    for (let x = 0; x < 8; x++) {
      let v = 0;
      for (let bit = 0; bit < 3; bit++) {
        const xx = (x >> bit) & 1;
        const yy = (y >> bit) & 1;
        // The finest spatial bit carries the most weight: that is what makes the pattern fine-grained.
        v += b2[yy * 2 + xx]! / 4 ** (bit + 1);
      }
      m[y * 8 + x] = v - 0.5;
    }
  return m;
})();

export const sstep = (a: number, b: number, x: number) => {
  const t = x <= a ? 0 : x >= b ? 1 : (x - a) / (b - a);
  return t * t * (3 - 2 * t);
};

export const TAU = 6.28318;

/** How long each beat lasts, ms. */
export const BEAT_MS: Record<FieldBeat, number> = { send: 1800, pulse: 1400, done: 1250, fail: 1100 };
export const MAX_BEATS = 3;
/** Fronts drawn per frame: a beat draws up to three (its crest, an echo, a flash or settle at its origin). */
export const MAX_FRONTS = 9;
/** Floats per front: cx, cy, radius, lead, tail, amp, push, disk, multiply, trough. */
export const FS = 10;
/** The pointer's light: reach (field heights), strength, and the spring it follows the pointer on. */
export const LIGHT_R = 0.4;
export const LIGHT_K = 0.34;
export const LIGHT_W = 5.2;

/**
 * The spectrum palette's hues: Sable's own spectrum (the working mark's blue, violet, pink,
 * ember and gold) as a closed loop, held a little below full saturation so a whole stage of
 * it stays a wash of light rather than a rainbow.
 */
export const HUE_N = 512;
export const HUES = (() => {
  const stops = ["#5b8cf0", "#9a78f2", "#e46aa4", "#f07a48", "#eaa850", "#e46aa4", "#9a78f2"].map(hex);
  const t = new Float32Array(HUE_N * 3);
  for (let i = 0; i < HUE_N; i++) {
    const f = (i / HUE_N) * stops.length;
    const k = Math.floor(f);
    const u = sstep(0, 1, f - k);
    const a = stops[k % stops.length]!;
    const b = stops[(k + 1) % stops.length]!;
    const rgb = [0, 1, 2].map((j) => (a[j]! + (b[j]! - a[j]!) * u) / 255);
    const grey = rgb[0]! * 0.3 + rgb[1]! * 0.55 + rgb[2]! * 0.15;
    for (let j = 0; j < 3; j++) t[i * 3 + j] = grey + (rgb[j]! - grey) * 0.82;
  }
  return t;
})();
/** cos(x) by table, for per-cell waves: 1024 steps around the circle are far finer than a cell. */
export const COS_N = 1024;
export const COS = (() => {
  const t = new Float32Array(COS_N);
  for (let i = 0; i < COS_N; i++) t[i] = Math.cos((i / COS_N) * Math.PI * 2);
  return t;
})();
export const COS_K = COS_N / (Math.PI * 2);
export const fcos = (x: number) => COS[((x * COS_K) | 0) & (COS_N - 1)]!;
/** The session's horizon, in band heights: where its crest is (from the top), how far it sinks to the
    stage's edges, how far its glow rises, how thick the bright edge is, and the light left below it. */
export const HORIZON_APEX = 0.47;
export const HORIZON_SAG = 0.6;
export const HORIZON_GLOW = 0.36;
export const HORIZON_RIM = 0.08;
export const HORIZON_GROUND = 0.24;
export const STREAK_COS = Math.cos(-0.6);
export const STREAK_SIN = Math.sin(-0.6);
/** Streaks run along a 35° diagonal, rising to the right on screen (y grows downward). */
export const STREAKS_COS = Math.cos((35 * Math.PI) / 180);
export const STREAKS_SIN = Math.sin((35 * Math.PI) / 180);

/** Tubes: grid pitch (in field heights), and how far the light reaches from the focus. */
export const TUBE_G = 0.2;
/** A full-bleed field of tubes is a finer, regular grid. */
export const TUBE_G_FULL = 0.1;
export const TUBE_REACH = 0.92;
/** Past this distance the brightest rim, after the falloff, stays under the black floor: never evaluated. */
export const TUBE_SPAN = 0.8;
/** The travelling wave: radians per tube along its direction, and its angular speed (a 4.2 s period). */
export const WAVE_K = 0.9;
export const WAVE_W = TAU / 4.2;
/** Full bleed: one long, slow sweep crossing the stage (about 20 tubes per wavelength, an 11 s period). */
export const WAVE_K_FULL = 0.32;
export const WAVE_W_FULL = TAU / 11;
/** Tiles: a plane seen from above at a slight angle. Horizon (field heights above the top), scales across and along. */
export const TILE_H = -2.2;
export const TILE_SU = 25;
export const TILE_SV = 92;
export const TILE_REACH = 0.88;
export const TILE_SPAN = 0.84;
/** Tile body: half size and corner radius in cell units, and the width of the lit edge. */
export const TILE_HALF = 0.4;
export const TILE_R = 0.16;
export const TILE_RIM = 0.075;
/** Full bleed: the soft darkening behind the centre column (field heights) and how deep it goes. */
export const HOLLOW_Y = 0.45;
export const HOLLOW_RX = 0.62;
export const HOLLOW_RY = 0.4;
export const HOLLOW = 0.64;
/** Full bleed: light above this knee rolls off toward KNEE + ROLL, so the brightest cells stay well below white. */
export const KNEE = 0.4;
export const ROLL = 0.24;
/** Full bleed: how strongly each variant shows, so they sit at the same calm level. */
export const FULL_GAIN: Partial<Record<DitherVariant, number>> = {
  streaks: 2.1,
  aurora: 1.3,
  tubes: 0.85,
  tiles: 0.85,
};

// ── renderers ──

export type Surface = HTMLCanvasElement | OffscreenCanvas;
export type Ctx = CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D;
export type Source = CanvasImageSource;

export function context(el: Surface, opts?: CanvasRenderingContext2DSettings): Ctx | null {
  return (el as HTMLCanvasElement).getContext("2d", opts) as Ctx | null;
}

export function scratch(w: number, h: number): Surface {
  if (typeof OffscreenCanvas !== "undefined") return new OffscreenCanvas(w, h);
  const c = document.createElement("canvas");
  c.width = w;
  c.height = h;
  return c;
}

export interface Renderer {
  resize(m: Metrics): void;
  /** False when nothing could be drawn yet (glyphs before their atlas arrives). */
  draw(field: Field, look: Look, now: number): boolean;
  setAtlas?(atlas: Source): void;
  setImage?(image: ImageSource | null): void;
}

export type ImageSource = CanvasImageSource & { width: number; height: number };
