// design-lint: allow-file (colours sampled from a picture)
// The new-thread banner, after NewThreadBanner.swift and AsciiBanner.swift:
// a wide picture the user picks in Settings ▸ Appearance, shown over a new
// thread either as is or drawn in coloured text. The app keeps a copy in its
// data folder (src-tauri/src/banner.rs); the setting holds the copy's name.
// In the browser build the picture is kept, downscaled, in localStorage.

import { useSyncExternalStore } from "react";
import { isTauri } from "../acp/transport";
import { shellCall } from "./shell";

export type BannerStyle = "ascii" | "picture";

const KEY = "rustcode.newThreadBanner";
const STYLE_KEY = "rustcode.newThreadBannerStyle";
const DATA_KEY = "rustcode.newThreadBannerData";
const BROWSER_NAME = "browser";

function read(key: string): string {
  try {
    return localStorage.getItem(key) ?? "";
  } catch {
    return "";
  }
}

function write(key: string, value: string) {
  try {
    if (value) localStorage.setItem(key, value);
    else localStorage.removeItem(key);
  } catch {
    // Not kept in a private window.
  }
}

const listeners = new Set<() => void>();
let snapshot = {
  name: read(KEY),
  style: (read(STYLE_KEY) === "picture" ? "picture" : "ascii") as BannerStyle,
};

function publish() {
  snapshot = { name: read(KEY), style: read(STYLE_KEY) === "picture" ? "picture" : "ascii" };
  for (const l of listeners) l();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** The banner setting: the kept picture's name ("" for none) and its style. */
export function useBanner(): { name: string; style: BannerStyle } {
  return useSyncExternalStore(subscribe, () => snapshot);
}

export function setBannerStyle(style: BannerStyle) {
  write(STYLE_KEY, style);
  publish();
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
  write(KEY, name);
  publish();
  return name;
}

export async function removeBanner() {
  const name = snapshot.name;
  if (!name) return;
  images.delete(name);
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

// ---------------------------------------------------------------- text art

/** Lightest ink first. */
export const RAMP = " .:-=+*#%@";
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
      const step = Math.min(RAMP.length - 1, Math.floor(ink * RAMP.length));
      cells.push({ ch: RAMP[step] ?? " ", r: rr, g: gg, b: bb });
    }
    rows.push(cells);
  }
  return rows;
}

/** The text's size: small enough that a band reads as a picture. */
export const FONT_SIZE = 7;
export const LINE_HEIGHT = 9;
const FONT = `${FONT_SIZE}px "SF Mono", ui-monospace, Menlo, monospace`;

let charWidthCache = 0;
export function charWidth(): number {
  if (charWidthCache) return charWidthCache;
  const ctx = document.createElement("canvas").getContext("2d");
  if (!ctx) return 4.2;
  ctx.font = FONT;
  charWidthCache = ctx.measureText("M").width || 4.2;
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
      const color = `rgb(${Math.round(cell.r * 15) * 17} ${Math.round(cell.g * 15) * 17} ${Math.round(cell.b * 15) * 17})`;
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
