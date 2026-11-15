// design-lint: allow-file (colours sampled from a picture)
// The new-thread banner, after NewThreadBanner.swift and AsciiBanner.swift:
// a wide picture the user picks in Settings ▸ Appearance, shown over a new
// thread either as is or drawn in coloured text. The app keeps a copy in its
// data folder (src-tauri/src/banner.rs); the setting holds the copy's name.
// In the browser build the picture is kept, downscaled, in localStorage.

import { useSyncExternalStore } from "react";
import { isTauri } from "../acp/transport";
import { EffectCache, effectKey, fitWithin, type PixelEffect } from "./bannerEffects";
import type { EffectRequest, EffectResponse } from "./bannerEffects.worker";
import { shellCall } from "./shell";
import { readStorage, writeStorage } from "./storage";

/**
 * How the picture is drawn: in coloured text, as is, blurred and faded (haze), or re-printed by
 * the worker as a dither, a halftone or scanlines (lib/bannerEffects.ts).
 */
export type BannerStyle = "ascii" | "picture" | "haze" | PixelEffect;

export const BANNER_STYLES: readonly BannerStyle[] = [
  "ascii",
  "picture",
  "dither",
  "halftone",
  "scanlines",
  "haze",
];

/** A stored style; anything unknown is the default. */
export function parseBannerStyle(raw: string): BannerStyle {
  return (BANNER_STYLES as readonly string[]).includes(raw) ? (raw as BannerStyle) : BANNER_STYLE_DEFAULT;
}

const KEY = "rustcode.newThreadBanner";
const STYLE_KEY = "rustcode.newThreadBannerStyle";
const DARKEN_KEY = "rustcode.newThreadBannerDarken";
const BLUR_KEY = "rustcode.newThreadBannerBlur";
const IN_THREADS_KEY = "rustcode.newThreadBannerInThreads";
const VISIBILITY_KEY = "rustcode.newThreadBannerVisibility";
const DATA_KEY = "rustcode.newThreadBannerData";
const BROWSER_NAME = "browser";
/** The bundled sample picture, for the browser build and the mock (see {@link showSampleBanner}). */
const SAMPLE_NAME = "sample";

/** No picture until one is chosen, and then drawn in text (Settings › Appearance). */
export const BANNER_NAME_DEFAULT = "";
export const BANNER_STYLE_DEFAULT: BannerStyle = "ascii";
/** How much black lies over the picture, in percent: none until asked for (the pane's own wash stays). */
export const BANNER_DARKEN_DEFAULT = 0;
export const BANNER_DARKEN_MAX = 80;
/** How far the picture is blurred, in px. */
export const BANNER_BLUR_DEFAULT = 0;
export const BANNER_BLUR_MAX = 40;
/** The picture shows behind open threads too: off, so it stays a new thread's. */
export const BANNER_IN_THREADS_DEFAULT = false;
/**
 * How strongly the picture shows behind an open thread, in percent. 20% is the 80% wash of the
 * pane colour that keeps a thread's text readable over any picture (docs/desktop.md › New thread
 * banner), so the default is that and more is the user's call.
 */
export const BANNER_VISIBILITY_DEFAULT = 20;
export const BANNER_VISIBILITY_MAX = 60;

export interface BannerSetting {
  name: string;
  style: BannerStyle;
  /** Percent of black over the picture, 0 to {@link BANNER_DARKEN_MAX}. */
  darken: number;
  /** Blur radius in px, 0 to {@link BANNER_BLUR_MAX}. */
  blur: number;
  /** Whether the picture also shows behind open threads. */
  inThreads: boolean;
  /** Percent the picture shows at behind an open thread, 0 to {@link BANNER_VISIBILITY_MAX}. */
  visibility: number;
}

function read(key: string): string {
  return readStorage(key) ?? "";
}

function write(key: string, value: string) {
  // Not kept in a private window.
  writeStorage(key, value || null);
}

/** A stored whole number held to 0..max; anything unreadable is the default. */
export function boundedNumber(raw: string, max: number, fallback: number): number {
  if (raw.trim() === "") return fallback;
  const n = Number(raw);
  if (!Number.isFinite(n)) return fallback;
  return Math.min(max, Math.max(0, Math.round(n)));
}

function load(): BannerSetting {
  return {
    name: read(KEY),
    style: parseBannerStyle(read(STYLE_KEY)),
    darken: boundedNumber(read(DARKEN_KEY), BANNER_DARKEN_MAX, BANNER_DARKEN_DEFAULT),
    blur: boundedNumber(read(BLUR_KEY), BANNER_BLUR_MAX, BANNER_BLUR_DEFAULT),
    inThreads: read(IN_THREADS_KEY) === "1",
    visibility: boundedNumber(read(VISIBILITY_KEY), BANNER_VISIBILITY_MAX, BANNER_VISIBILITY_DEFAULT),
  };
}

const listeners = new Set<() => void>();
let snapshot = load();

function publish() {
  snapshot = load();
  for (const l of listeners) l();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** The banner setting now, outside React. */
export function bannerSetting(): BannerSetting {
  return snapshot;
}

/** The banner setting: the kept picture's name ("" for none), its style, darkening, blur and reach. */
export function useBanner(): BannerSetting {
  return useSyncExternalStore(subscribe, () => snapshot);
}

export function setBannerStyle(style: BannerStyle) {
  write(STYLE_KEY, style);
  publish();
}

/** Percent of black over the picture; the default is stored as nothing. */
export function setBannerDarken(percent: number) {
  const value = boundedNumber(String(percent), BANNER_DARKEN_MAX, BANNER_DARKEN_DEFAULT);
  write(DARKEN_KEY, value === BANNER_DARKEN_DEFAULT ? "" : String(value));
  publish();
}

/** The picture's blur in px; the default is stored as nothing. */
export function setBannerBlur(px: number) {
  const value = boundedNumber(String(px), BANNER_BLUR_MAX, BANNER_BLUR_DEFAULT);
  write(BLUR_KEY, value === BANNER_BLUR_DEFAULT ? "" : String(value));
  publish();
}

/** Whether open threads show the picture too. */
export function setBannerInThreads(on: boolean) {
  write(IN_THREADS_KEY, on ? "1" : "");
  publish();
}

/** How strongly the picture shows behind an open thread; the default is stored as nothing. */
export function setBannerVisibility(percent: number) {
  const value = boundedNumber(String(percent), BANNER_VISIBILITY_MAX, BANNER_VISIBILITY_DEFAULT);
  write(VISIBILITY_KEY, value === BANNER_VISIBILITY_DEFAULT ? "" : String(value));
  publish();
}

/**
 * Shows the bundled sample picture, for the mock and the browser build, where there is no file to
 * choose from a script: `(await import("/src/lib/banner.ts")).showSampleBanner("dither")`.
 */
export function showSampleBanner(style?: BannerStyle) {
  if (isTauri()) return;
  if (style) write(STYLE_KEY, style === BANNER_STYLE_DEFAULT ? "" : style);
  write(KEY, SAMPLE_NAME);
  publish();
}

/** The picture's black veil as a CSS colour, or null when there is none. */
export function darkenOverlay(darken: number): string | null {
  const percent = Math.min(BANNER_DARKEN_MAX, Math.max(0, Math.round(darken)));
  return percent > 0 ? `rgb(0 0 0 / ${percent}%)` : null;
}

/**
 * How far the picture grows so a blur's soft edge falls outside the pane: a blur fades about two
 * radii in from each side, so the layer must overshoot by that on both.
 */
export function blurScale(blur: number, width: number, height: number): number {
  const shortest = Math.min(width, height);
  if (blur <= 0 || shortest <= 0) return 1;
  return 1 + (4 * blur) / shortest;
}

/** Asks for a picture and keeps a copy. The new name, or null when none was chosen. */
export async function chooseBanner(): Promise<string | null> {
  const old = snapshot.name;
  let name: string | null;
  if (isTauri()) {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({
      multiple: false,
      directory: false,
      title: "Choose a wide picture for the top of new threads",
      filters: [{ name: "Pictures", extensions: ["png", "jpg", "jpeg", "gif", "webp"] }],
    });
    if (typeof picked !== "string") return null;
    name = await shellCall<string>("banner_save", { source: picked, replacing: old || null });
  } else {
    const data = await pickInBrowser();
    if (!data) return null;
    write(DATA_KEY, data);
    name = BROWSER_NAME;
  }
  images.delete(old);
  effects.keepOnly(name);
  write(KEY, name);
  publish();
  return name;
}

export async function removeBanner() {
  const name = snapshot.name;
  if (!name) return;
  images.delete(name);
  effects.keepOnly("");
  write(KEY, "");
  if (name === BROWSER_NAME) write(DATA_KEY, "");
  else if (isTauri()) await shellCall("banner_remove", { name }).catch(() => undefined);
  publish();
}

function pickInBrowser(): Promise<string | null> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = "image/png,image/jpeg,image/gif,image/webp";
    input.onchange = () => {
      const file = input.files?.[0];
      if (!file) return resolve(null);
      const url = URL.createObjectURL(file);
      const img = new Image();
      img.onload = () => {
        // Small enough for localStorage; a banner never needs more.
        const scale = Math.min(1, 1600 / img.naturalWidth);
        const canvas = document.createElement("canvas");
        canvas.width = Math.max(1, Math.round(img.naturalWidth * scale));
        canvas.height = Math.max(1, Math.round(img.naturalHeight * scale));
        canvas.getContext("2d")?.drawImage(img, 0, 0, canvas.width, canvas.height);
        URL.revokeObjectURL(url);
        resolve(canvas.toDataURL("image/jpeg", 0.85));
      };
      img.onerror = () => resolve(null);
      img.src = url;
    };
    input.click();
  });
}

// ---------------------------------------------------------------- loading

/** Decoded pictures, kept so a new thread shows its banner at once. */
const images = new Map<string, Promise<HTMLImageElement | null>>();

export function loadBanner(name: string): Promise<HTMLImageElement | null> {
  const cached = images.get(name);
  if (cached) return cached;
  const loading = (async () => {
    let src: string;
    if (name === BROWSER_NAME) src = read(DATA_KEY);
    else if (name === SAMPLE_NAME && !isTauri())
      src = (await import("../assets/banner-sample.png?url")).default;
    else if (isTauri()) {
      const file = await shellCall<{ mimeType: string; data: string }>("banner_read", { name });
      src = `data:${file.mimeType};base64,${file.data}`;
    } else return null;
    if (!src) return null;
    const img = new Image();
    img.decoding = "async";
    img.src = src;
    await img.decode();
    return img;
  })().catch(() => null);
  images.set(name, loading);
  return loading;
}

// ---------------------------------------------------------------- print effects

let worker: Worker | null | undefined;
let nextId = 0;
const waiting = new Map<number, (png: Blob | null) => void>();

/** The effects worker; null where there is none (then the picture shows as is). */
function effectsWorker(): Worker | null {
  if (worker !== undefined) return worker;
  if (typeof Worker === "undefined" || typeof OffscreenCanvas === "undefined") {
    worker = null;
    return worker;
  }
  try {
    worker = new Worker(new URL("./bannerEffects.worker.ts", import.meta.url), { type: "module" });
    worker.onmessage = (event: MessageEvent<EffectResponse>) => {
      waiting.get(event.data.id)?.(event.data.png);
      waiting.delete(event.data.id);
    };
    worker.onerror = () => {
      worker?.terminate();
      worker = null;
      for (const done of waiting.values()) done(null);
      waiting.clear();
    };
  } catch {
    worker = null;
  }
  return worker;
}

/** Rendered effects as blob URLs, by picture, effect and theme (lib/bannerEffects.ts). */
const effects = new EffectCache((url) => URL.revokeObjectURL(url));
const rendering = new Map<string, Promise<string | null>>();

/** The rendered effect's URL if it is ready, so a picture seen before shows at once. */
export function cachedEffect(name: string, effect: PixelEffect, dark: boolean): string | undefined {
  return effects.get(effectKey(name, effect, dark));
}

/**
 * The picture re-printed with `effect`, as a blob URL: rendered once in the worker, from a copy
 * at most 2048px on its long edge, then kept. Null when it cannot be rendered here.
 */
export function renderEffect(
  name: string,
  img: HTMLImageElement,
  effect: PixelEffect,
  dark: boolean,
): Promise<string | null> {
  const key = effectKey(name, effect, dark);
  const ready = effects.get(key);
  if (ready) return Promise.resolve(ready);
  const running = rendering.get(key);
  if (running) return running;
  const w = effectsWorker();
  if (!w || typeof createImageBitmap !== "function") return Promise.resolve(null);
  const job = (async () => {
    const size = fitWithin(img.naturalWidth, img.naturalHeight);
    if (!size.width) return null;
    const bitmap = await createImageBitmap(img, {
      resizeWidth: size.width,
      resizeHeight: size.height,
      resizeQuality: "high",
    });
    const id = ++nextId;
    const png = await new Promise<Blob | null>((resolve) => {
      waiting.set(id, resolve);
      w.postMessage({ id, bitmap, effect, dark } satisfies EffectRequest, [bitmap]);
    });
    // The picture changed while this ran: its render is not kept.
    if (!png || snapshot.name !== name) return null;
    const url = URL.createObjectURL(png);
    effects.set(key, url);
    return url;
  })()
    .catch(() => null)
    .finally(() => rendering.delete(key));
  rendering.set(key, job);
  return job;
}

// ---------------------------------------------------------------- text art

/** Lightest ink first. */
export const RAMP = " .:-=+*#%@";
/** Ink at or below this is paper: a blank cell. */
const BLANK_INK = 0.02;
/** Width over height the band is cropped to at least. */
export const MIN_ASPECT = 3;
/** A monospaced cell is about twice as tall as wide. */
export const CELL_ASPECT = 2;

export interface Cell {
  ch: string;
  r: number;
  g: number;
  b: number;
}

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** The centred crop of a picture at least `aspect` wide; with `fill`, exactly `aspect`. */
export function crop(width: number, height: number, aspect = MIN_ASPECT, fill = false): Rect {
  if (width <= 0 || height <= 0) return { x: 0, y: 0, width: 0, height: 0 };
  if (fill && aspect > 0) {
    const narrowest = Math.max(Math.floor(height * aspect), 1);
    if (width > narrowest) return { x: Math.floor((width - narrowest) / 2), y: 0, width: narrowest, height };
    const tallest = Math.max(Math.floor(width / aspect), 1);
    return { x: 0, y: Math.floor((height - tallest) / 2), width, height: tallest };
  }
  const tallest = Math.floor(width / Math.max(aspect, MIN_ASPECT));
  if (height <= tallest) return { x: 0, y: 0, width, height };
  return { x: 0, y: Math.floor((height - tallest) / 2), width, height: Math.max(tallest, 1) };
}

/**
 * The picture's RGBA pixels as rows of cells, `columns` wide: each cell's
 * character inks the brightness there, in the colour there. `inverted` maps
 * light to blank, for a light background.
 */
export function renderAscii(
  rgba: Uint8ClampedArray | Uint8Array,
  width: number,
  height: number,
  columns: number,
  { inverted = false, aspect = MIN_ASPECT, fill = false } = {},
): Cell[][] {
  if (width <= 0 || height <= 0 || columns <= 0 || rgba.length < width * height * 4) return [];
  const c = crop(width, height, aspect, fill);
  const rowCount = Math.max(1, Math.round(columns / (c.width / Math.max(c.height, 1)) / CELL_ASPECT));
  const cw = c.width / columns;
  const ch = c.height / rowCount;
  const rows: Cell[][] = [];
  for (let row = 0; row < rowCount; row++) {
    const y0 = Math.floor(c.y + row * ch);
    const y1 = Math.max(y0 + 1, Math.min(height, Math.floor(c.y + (row + 1) * ch)));
    const cells: Cell[] = [];
    for (let col = 0; col < columns; col++) {
      const x0 = Math.floor(c.x + col * cw);
      const x1 = Math.max(x0 + 1, Math.min(width, Math.floor(c.x + (col + 1) * cw)));
      let r = 0;
      let g = 0;
      let b = 0;
      let a = 0;
      let n = 0;
      for (let y = y0; y < y1; y++) {
        let i = (y * width + x0) * 4;
        for (let x = x0; x < x1; x++, i += 4) {
          const alpha = (rgba[i + 3] ?? 0) / 255;
          r += ((rgba[i] ?? 0) / 255) * alpha;
          g += ((rgba[i + 1] ?? 0) / 255) * alpha;
          b += ((rgba[i + 2] ?? 0) / 255) * alpha;
          a += alpha;
          n++;
        }
      }
      // Premultiplied sums: a see-through part reads as background.
      const coverage = a / n;
      const rr = a > 0 ? r / a : 0;
      const gg = a > 0 ? g / a : 0;
      const bb = a > 0 ? b / a : 0;
      const luminance = (0.2126 * rr + 0.7152 * gg + 0.0722 * bb) * coverage;
      const ink = inverted ? (1 - luminance) * coverage : luminance;
      // Lifted (square root) so mid-tones and shadows still draw a glyph: only paper itself, pure
      // black on dark or pure white on light, is a blank cell.
      const step =
        ink <= BLANK_INK
          ? 0
          : Math.max(1, Math.min(RAMP.length - 1, Math.floor(Math.sqrt(ink) * RAMP.length)));
      cells.push({ ch: RAMP[step] ?? " ", r: rr, g: gg, b: bb });
    }
    rows.push(cells);
  }
  return rows;
}

/**
 * The text's size: about 150 columns across a chat pane, small enough that the picture reads and
 * large enough that each cell is a glyph rather than a dash in a run.
 */
export const FONT_SIZE = 12;
export const LINE_HEIGHT = 14;
const FONT = `600 ${FONT_SIZE}px "SF Mono", ui-monospace, Menlo, monospace`;

/**
 * A cell's colour as ink on the theme's paper, channels 0 to 1: the source's hue and saturation,
 * its lightness lifted to a floor on dark paper (pressed below a ceiling on light), so a dark sky
 * or a shadow still reads as its colour instead of vanishing into the background.
 */
export function inkColor(r: number, g: number, b: number, dark: boolean): [number, number, number] {
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  const d = max - min;
  let h = 0;
  let sat = 0;
  if (d > 0) {
    sat = d / (1 - Math.abs(2 * l - 1));
    if (max === r) h = ((g - b) / d + 6) % 6;
    else if (max === g) h = (b - r) / d + 2;
    else h = (r - g) / d + 4;
  }
  sat = Math.min(1, sat * 1.15);
  const lift = dark ? 0.42 + 0.53 * l : 0.08 + 0.42 * l;
  const c = (1 - Math.abs(2 * lift - 1)) * sat;
  const x = c * (1 - Math.abs((h % 2) - 1));
  const m = lift - c / 2;
  const [r1, g1, b1] =
    h < 1
      ? [c, x, 0]
      : h < 2
        ? [x, c, 0]
        : h < 3
          ? [0, c, x]
          : h < 4
            ? [0, x, c]
            : h < 5
              ? [x, 0, c]
              : [c, 0, x];
  return [r1 + m, g1 + m, b1 + m];
}

let charWidthCache = 0;
export function charWidth(): number {
  if (charWidthCache) return charWidthCache;
  const ctx = document.createElement("canvas").getContext("2d");
  if (!ctx) return 7.2;
  ctx.font = FONT;
  charWidthCache = ctx.measureText("M").width || 7.2;
  return charWidthCache;
}

export interface Drawing {
  canvas: HTMLCanvasElement;
  width: number;
  height: number;
}

const drawings = new Map<string, Drawing>();

/**
 * The picture drawn as text on an offscreen canvas, once per size and
 * appearance: thousands of coloured runs are drawn a single time and then
 * shown as a bitmap that scales while the window resizes.
 */
export function asciiDrawing(
  name: string,
  img: HTMLImageElement,
  columns: number,
  rows: number,
  dark: boolean,
  scale: number,
): Drawing | null {
  const key = `${name} ${columns}x${rows} ${dark} ${scale}`;
  const cached = drawings.get(key);
  if (cached) return cached;
  const aspect = columns / (rows * CELL_ASPECT);
  // Drawn small first: a cell needs only a few pixels.
  const share = Math.min(1, (img.naturalHeight * aspect) / Math.max(img.naturalWidth, 1));
  const s = Math.min(1, (columns * 8) / (Math.max(img.naturalWidth, 1) * Math.max(share, 0.05)));
  const w = Math.max(1, Math.round(img.naturalWidth * s));
  const h = Math.max(1, Math.round(img.naturalHeight * s));
  const sample = document.createElement("canvas");
  sample.width = w;
  sample.height = h;
  const sctx = sample.getContext("2d", { willReadFrequently: true });
  if (!sctx) return null;
  sctx.drawImage(img, 0, 0, w, h);
  const cells = renderAscii(sctx.getImageData(0, 0, w, h).data, w, h, columns, {
    inverted: !dark,
    aspect,
    fill: true,
  });
  const cw = charWidth();
  const width = Math.ceil(columns * cw);
  const height = Math.ceil(cells.length * LINE_HEIGHT);
  if (!width || !height) return null;
  const canvas = document.createElement("canvas");
  canvas.width = Math.round(width * scale);
  canvas.height = Math.round(height * scale);
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  ctx.scale(scale, scale);
  ctx.font = FONT;
  ctx.textBaseline = "middle";
  cells.forEach((row, y) => {
    // Runs of like colour (16 steps a channel) are drawn as one string.
    let run = "";
    let runColor = "";
    let start = 0;
    const flush = (end: number) => {
      if (run.trim()) {
        ctx.fillStyle = runColor || "rgb(128 128 128)";
        ctx.fillText(run, start * cw, y * LINE_HEIGHT + LINE_HEIGHT / 2);
      }
      run = "";
      start = end;
    };
    row.forEach((cell, x) => {
      const [ir, ig, ib] = inkColor(cell.r, cell.g, cell.b, dark);
      const color = `rgb(${Math.round(ir * 15) * 17} ${Math.round(ig * 15) * 17} ${Math.round(ib * 15) * 17})`;
      if (cell.ch !== " " && color !== runColor) {
        flush(x);
        runColor = color;
      }
      run += cell.ch;
    });
    flush(row.length);
  });
  const drawing = { canvas, width, height };
  drawings.set(key, drawing);
  // A handful of sizes at most; a long resize session must not grow this forever.
  if (drawings.size > 12) drawings.delete(drawings.keys().next().value as string);
  return drawing;
}
