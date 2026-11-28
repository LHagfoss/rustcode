// design-lint: allow-file (the backdrop's palettes are light, not interface colour)
// Ported from Sable (https://github.com/Leonxlnx/sable, src/art/field.ts), MIT License,
// Copyright (c) 2026 Leonxlnx. Layouts and the renderers that draw a field into a canvas.

import type { Field } from "./field";
import {
  BAYER,
  context,
  type DitherTexture,
  type ImageSource,
  type Look,
  type Renderer,
  type Source,
  type Surface,
  scratch,
} from "./fieldBase";

// ── layout (shared with the host, so it can size the canvas element to match) ──

/** Above this many cells a dither stage gets coarser cells: the frame cost stays flat on big screens. */
export const MAX_CELLS = 24000;

export function ditherLayout(width: number, height: number, cell: number) {
  let c = cell;
  let cols = Math.max(2, Math.ceil(width / c));
  let rows = Math.max(2, Math.ceil(height / c));
  while (cols * rows > MAX_CELLS && c < 24) {
    c++;
    cols = Math.max(2, Math.ceil(width / c));
    rows = Math.max(2, Math.ceil(height / c));
  }
  return { cell: c, cols, rows };
}

/** Glyph cell in CSS px (Geist Mono advances ~0.6em, so 12.5px type fills an 8px column). */
export const GLYPH_W = 8;
export const GLYPH_H = 14;
export const GLYPH_FONT = 12.5;
/** Above this stage area (CSS px²) glyphs grow by a quarter: big screens get fewer, larger characters. */
export const GLYPH_BIG_AREA = 1.5e6;
/** Dark to bright. Index 0 is never drawn. */
export const RAMP = " .·:-=+*xo#%@";

/** The dither cell the glyph size is judged against: at this cell, glyphs are their default size. */
export const GLYPH_REF_CELL = 5;

/** `cell` (the dither cell asked for) scales the glyphs: finer cells, smaller characters. */
export function asciiLayout(width: number, height: number, dpr: number, cell = GLYPH_REF_CELL) {
  const d = Math.min(3, Math.max(1, dpr || 1));
  // Whole device pixels per cell keep every glyph on the pixel grid: crisp at any ratio.
  const k = Math.min(2.4, Math.max(0.8, cell / GLYPH_REF_CELL));
  const s = (width * height > GLYPH_BIG_AREA ? 1.25 : 1) * k;
  const cw = GLYPH_W * s;
  const ch = GLYPH_H * s;
  return {
    dpr: d,
    font: GLYPH_FONT * s,
    cols: Math.max(2, Math.ceil(width / cw)),
    rows: Math.max(2, Math.ceil(height / ch)),
    gw: Math.round(cw * d),
    gh: Math.round(ch * d),
  };
}

/** At most this many dots: each is a blit when it changes, like a glyph. */
export const MAX_DOTS = 9000;

/** Halftone: square cells a little larger than the dither cell, sized in whole device pixels. */
export function halftoneLayout(width: number, height: number, dpr: number, cell: number) {
  const d = Math.min(2, Math.max(1, dpr || 1));
  let c = Math.max(5, cell * 1.6);
  let cols = Math.max(2, Math.ceil(width / c));
  let rows = Math.max(2, Math.ceil(height / c));
  while (cols * rows > MAX_DOTS) {
    c += 1;
    cols = Math.max(2, Math.ceil(width / c));
    rows = Math.max(2, Math.ceil(height / c));
  }
  const g = Math.round(c * d);
  return { dpr: d, cols, rows, gw: g, gh: g };
}

/** The soft image's canvas resolution: about one device pixel per CSS pixel, capped. */
export function softImageLayout(width: number, height: number, dpr: number) {
  const k = Math.min(1.25, Math.max(0.5, dpr || 1), 1600 / Math.max(1, width));
  return { w: Math.max(2, Math.round(width * k)), h: Math.max(2, Math.round(height * k)) };
}

/**
 * How big the host should make the canvas element (CSS px) for a texture, and for glyphs the
 * atlas it must draw and send.
 */
export function fieldLayout(
  texture: DitherTexture,
  image: boolean,
  width: number,
  height: number,
  dpr: number,
  cell: number,
): {
  cssW: number;
  cssH: number;
  cell: number;
  atlas?: { gw: number; gh: number; dpr: number; font: number };
} {
  if (texture === "soft" && image) return { cssW: width, cssH: height, cell };
  if (texture === "ascii") {
    const L = asciiLayout(width, height, dpr, cell);
    return {
      cssW: (L.cols * L.gw) / L.dpr,
      cssH: (L.rows * L.gh) / L.dpr,
      cell,
      atlas: { gw: L.gw, gh: L.gh, dpr: L.dpr, font: L.font },
    };
  }
  if (texture === "halftone") {
    const L = halftoneLayout(width, height, dpr, cell);
    return { cssW: (L.cols * L.gw) / L.dpr, cssH: (L.rows * L.gh) / L.dpr, cell: L.gw / L.dpr };
  }
  const L = ditherLayout(width, height, cell);
  return { cssW: L.cols * L.cell, cssH: L.rows * L.cell, cell: L.cell };
}

export const LEVELS = 4;
export const NONE: [number, number] = [1, 0];
export const FULL_ROW: [number, number] = [0, 1e9];

/** The first and last column of a `cols`-wide row that the light can reach (none when first > last). */
export function litColumns(
  field: Field,
  py: number,
  aspect: number,
  cols: number,
  sp: Float32Array,
): [number, number] {
  field.span(py, sp);
  if (sp[0]! > sp[1]!) return NONE;
  const a = Math.ceil((sp[0]! / aspect) * cols - 0.5);
  const b = Math.floor((sp[1]! / aspect) * cols - 0.5);
  return [Math.max(a, 0), Math.min(b, cols - 1)];
}

/** `soft`: the same field without quantization (a whisper of ordered noise keeps gradients from banding). */
export function ditherRenderer(el: Surface, soft = false): Renderer | null {
  // One canvas may move between renderers (a texture change), so every renderer asks for the same context.
  const ctx = context(el);
  if (!ctx) return null;
  let w = 0;
  let h = 0;
  let image: ImageData | null = null;
  const c = new Float32Array(4);
  const sp = new Float32Array(2);
  let coarse = new Float32Array(0);
  let coarseW = 0;
  return {
    resize(m) {
      const { cols, rows } = ditherLayout(m.width, m.height, m.cell);
      if (cols === w && rows === h) return;
      w = cols;
      h = rows;
      el.width = w;
      el.height = h;
      image = ctx.createImageData(w, h);
    },
    draw(field, look, now) {
      if (!image) return false;
      const aspect = w / h;
      field.step(look, now, aspect);
      const lut = field.lut;
      const r0 = lut[0]!;
      const g0 = lut[1]!;
      const b0 = lut[2]!;
      const data = image.data;
      const R0 = r0 * 255;
      const G0 = g0 * 255;
      const B0 = b0 * 255;
      // The broad, soft variants are evaluated on every other cell and interpolated between:
      // a quarter of the work, and nothing they draw is finer than two cells. Tubes and tiles
      // have crisp edges and are evaluated per cell (they cache per tube or tile).
      // An image is sampled per cell: its detail is the point.
      // With a moving effect over an image, the image is read like the soft variants too: the frame
      // then costs what the pattern alone does.
      const smooth =
        look.variant !== "tubes" && look.variant !== "tiles" && (!field.hasImage || field.overlaying);
      field.cellRows = h;
      // A full-bleed backdrop gets a couple more levels: its dim reaches read as soft light, not scattered dots.
      const levels = look.full ? LEVELS + 2 : LEVELS;
      if (smooth) {
        const cw = (w >> 1) + 2;
        const ch = (h >> 1) + 2;
        if (coarse.length < cw * ch * 3) coarse = new Float32Array(cw * ch * 3);
        let k = 0;
        for (let j = 0; j < ch; j++) {
          const py = (2 * j + 0.5) / h;
          for (let i = 0; i < cw; i++, k += 3) {
            field.shade(((2 * i + 0.5) / w) * aspect, py, c);
            coarse[k] = c[0]!;
            coarse[k + 1] = c[1]!;
            coarse[k + 2] = c[2]!;
          }
        }
        coarseW = cw;
      }
      let o = 0;
      for (let y = 0; y < h; y++) {
        const py = (y + 0.5) / h;
        const brow = (y & 7) * 8;
        const [xa, xb] = smooth ? FULL_ROW : litColumns(field, py, aspect, w, sp);
        const row0 = (y >> 1) * coarseW * 3;
        const row1 = row0 + coarseW * 3;
        const fy = (y & 1) * 0.5;
        for (let x = 0; x < w; x++, o += 4) {
          if (x < xa || x > xb) {
            data[o] = R0;
            data[o + 1] = G0;
            data[o + 2] = B0;
            data[o + 3] = 255;
            continue;
          }
          let r: number;
          let g: number;
          let b: number;
          if (smooth) {
            const a = row0 + (x >> 1) * 3;
            const d = row1 + (x >> 1) * 3;
            if (x & 1) {
              // Halfway between two coarse columns.
              const t0 = (coarse[a]! + coarse[a + 3]!) * 0.5;
              const t1 = (coarse[a + 1]! + coarse[a + 4]!) * 0.5;
              const t2 = (coarse[a + 2]! + coarse[a + 5]!) * 0.5;
              r = t0 + ((coarse[d]! + coarse[d + 3]!) * 0.5 - t0) * fy;
              g = t1 + ((coarse[d + 1]! + coarse[d + 4]!) * 0.5 - t1) * fy;
              b = t2 + ((coarse[d + 2]! + coarse[d + 5]!) * 0.5 - t2) * fy;
            } else {
              r = coarse[a]! + (coarse[d]! - coarse[a]!) * fy;
              g = coarse[a + 1]! + (coarse[d + 1]! - coarse[a + 1]!) * fy;
              b = coarse[a + 2]! + (coarse[d + 2]! - coarse[a + 2]!) * fy;
            }
          } else {
            field.shade(((x + 0.5) / w) * aspect, py, c);
            r = c[0]!;
            g = c[1]!;
            b = c[2]!;
          }
          // Dither the light's distance from the ground colour, so unlit cells are exactly the stage
          // colour. The cell's strength is quantized, not each channel on its own, so a cell keeps its
          // hue at every level instead of scattering off-colour specks through dark areas. On a dark
          // ground the light is above it; on a light one (RustCode's light theme) it is ink below it.
          const lr = r - r0;
          const lg = g - g0;
          const lb = b - b0;
          const ar = lr < 0 ? -lr : lr;
          const ag = lg < 0 ? -lg : lg;
          const ab = lb < 0 ? -lb : lb;
          const m = ar > ag ? Math.max(ar, ab) : Math.max(ag, ab);
          if (m < 0.002) {
            data[o] = R0;
            data[o + 1] = G0;
            data[o + 2] = B0;
            data[o + 3] = 255;
            continue;
          }
          if (soft) {
            // Unbroken light reads brighter than the same light dithered: held a little lower.
            const n = BAYER[brow + (x & 7)]! * 1.5;
            data[o] = R0 + lr * 204 + n;
            data[o + 1] = G0 + lg * 204 + n;
            data[o + 2] = B0 + lb * 204 + n;
            data[o + 3] = 255;
            continue;
          }
          // The strength is never negative, so truncation is the floor.
          const q = (((m * levels + BAYER[brow + (x & 7)]! + 0.5) | 0) / levels / m) * 255;
          data[o] = R0 + lr * q;
          data[o + 1] = G0 + lg * q;
          data[o + 2] = B0 + lb * q;
          data[o + 3] = 255;
        }
      }
      ctx.putImageData(image, 0, 0);
      return true;
    },
  };
}

/** Light level that maps to the last glyph of the ramp (broad fields; tubes peak at 1). */
export const GLYPH_PEAK = 1.3;
/** A full-bleed backdrop is capped near 0.6, so its glyph ramp tops out there. */
export const GLYPH_PEAK_FULL = 0.75;
/** Strength of the soft halo drawn under the characters. */
export const GLOW = 0.16;

/** A broad field on every other cell (colour and level), interpolated back: a quarter of the evaluations. */
export class Coarse {
  private buf = new Float32Array(0);
  private w = 0;

  fill(field: Field, cols: number, rows: number, aspect: number, c: Float32Array) {
    const cw = (cols >> 1) + 2;
    const ch = (rows >> 1) + 2;
    if (this.buf.length < cw * ch * 4) this.buf = new Float32Array(cw * ch * 4);
    const buf = this.buf;
    let k = 0;
    for (let j = 0; j < ch; j++) {
      const py = (2 * j + 0.5) / rows;
      for (let i = 0; i < cw; i++, k += 4) {
        field.shade(((2 * i + 0.5) / cols) * aspect, py, c);
        buf[k] = c[0]!;
        buf[k + 1] = c[1]!;
        buf[k + 2] = c[2]!;
        buf[k + 3] = c[3]!;
      }
    }
    this.w = cw;
  }

  at(x: number, y: number, out: Float32Array) {
    const buf = this.buf;
    const a = ((y >> 1) * this.w + (x >> 1)) * 4;
    const d = a + this.w * 4;
    const fy = (y & 1) * 0.5;
    const fx = (x & 1) * 0.5;
    for (let j = 0; j < 4; j++) {
      const top = buf[a + j]! + (buf[a + 4 + j]! - buf[a + j]!) * fx;
      const bot = buf[d + j]! + (buf[d + 4 + j]! - buf[d + j]!) * fx;
      out[j] = top + (bot - top) * fy;
    }
  }
}

/** A ramp of white dots, area proportional to the level, one `g`-px cell each, side by side (like the glyph atlas). */
export function dotAtlas(g: number): Source | null {
  const c = scratch(g * RAMP.length, g);
  const ctx = context(c);
  if (!ctx) return null;
  ctx.fillStyle = "#fff";
  const top = RAMP.length - 1;
  for (let i = 1; i <= top; i++) {
    const r = Math.max(0.6, g * 0.5 * 0.96 * Math.sqrt(i / top));
    ctx.beginPath();
    ctx.arc(i * g + g / 2, g / 2, r, 0, Math.PI * 2);
    ctx.fill();
  }
  return c;
}

/** Characters (`ascii`, atlas from the host) or dots (`halftone`, atlas drawn here). */
export function asciiRenderer(el: Surface, kind: "ascii" | "halftone" = "ascii"): Renderer | null {
  const ctx = context(el);
  const tint = scratch(2, 2);
  const tctx = context(tint);
  const mask = scratch(2, 2);
  const mctx = context(mask);
  if (!ctx || !tctx || !mctx) return null;
  const dots = kind === "halftone";
  // Dots carry brightness in their size; a fainter halo keeps them crisp.
  const glow = dots ? 0.07 : GLOW;
  let cols = 0;
  let rows = 0;
  let gw = 0;
  let gh = 0;
  let image: ImageData | null = null;
  let atlas: Source | null = null;
  /** The glyph each cell of the mask holds; 255 forces a redraw. */
  let held = new Uint8Array(0);
  const coarse = new Coarse();
  const c = new Float32Array(4);
  const sp = new Float32Array(2);

  return {
    setAtlas(next) {
      if (dots) return;
      atlas = next;
      held.fill(255);
    },
    resize(m) {
      const L = dots
        ? halftoneLayout(m.width, m.height, m.dpr, m.cell)
        : asciiLayout(m.width, m.height, m.dpr, m.cell);
      if (L.cols === cols && L.rows === rows && L.gw === gw && L.gh === gh) return;
      if (dots) {
        atlas = dotAtlas(L.gw);
        held.fill(255);
      }
      ({ cols, rows, gw, gh } = L);
      el.width = cols * gw;
      el.height = rows * gh;
      mask.width = cols * gw;
      mask.height = rows * gh;
      tint.width = cols;
      tint.height = rows;
      image = tctx.createImageData(cols, rows);
      held = new Uint8Array(cols * rows).fill(255);
    },
    draw(field, look, now) {
      if (!image || !atlas) return false;
      const aspect = dots ? cols / rows : (cols * GLYPH_W) / (rows * GLYPH_H);
      field.step(look, now, aspect);
      const lut = field.lut;
      const r0 = lut[0]!;
      const g0 = lut[1]!;
      const b0 = lut[2]!;
      /** A light ground: the field is drawn as ink below it rather than light above it. */
      const paper = r0 + g0 + b0 > 1.5;
      const data = image.data;
      const top = RAMP.length - 1;
      // Tubes and tiles are crisp lines and edges of light; they lean on the dense glyphs to read.
      const tubes = look.variant === "tubes" || look.variant === "tiles";
      const full = look.full;
      const peak = full ? GLYPH_PEAK_FULL : GLYPH_PEAK;
      const W = cols * gw;
      const H = rows * gh;

      // The broad variants are evaluated on every other glyph and interpolated (see the dither renderer).
      const smooth = !tubes && (!field.hasImage || field.overlaying);
      field.cellRows = rows;
      if (smooth) coarse.fill(field, cols, rows, aspect, c);

      // 1. Glyph masks: only cells whose glyph changed since the last frame are touched.
      let o = 0;
      let i = 0;
      for (let y = 0; y < rows; y++) {
        const py = (y + 0.5) / rows;
        const brow = (y & 7) * 8;
        const dy = y * gh;
        const [xa, xb] = litColumns(field, py, aspect, cols, sp);
        for (let x = 0; x < cols; x++, o += 4, i++) {
          let gi = 0;
          if (x >= xa && x <= xb) {
            if (smooth) coarse.at(x, y, c);
            else field.shade(((x + 0.5) / cols) * aspect, py, c);
            const v = Math.min(c[3]!, 1);
            if (v! > 0) {
              // Ordered dither between neighbouring glyphs, so the ramp shades smoothly. Tubes are thin
              // lines of light and lean on the dense glyphs to read; the broad fields keep the densest
              // glyphs for their headroom past 1, so bright areas stay a weave instead of a wall of @.
              let gf: number;
              if (tubes) gf = Math.sqrt(v!) * top + BAYER[brow + (x & 7)]! * 0.9 + 0.5;
              else {
                const level = c[3]! >= peak ? 1 : c[3]! / peak;
                gf = level * Math.sqrt(level) * top + BAYER[brow + (x & 7)]! * 0.9 + 0.5;
              }
              gi = Math.floor(gf);
              // Hysteresis: a cell keeps its glyph until the light has clearly moved on, so slow drift
              // and breathing do not flip glyphs back and forth (each flip is a blit).
              const h = held[i]!;
              if (h !== 255 && h !== gi && h > 0 && gf > h - 0.15 && gf < h + 1.15) gi = h!;
              if (gi > top) gi = top;
              // The light above the ground, keeping its hue and lifted so dim cells still read.
              // On a light ground it is ink: its distance below the ground, drawn from the ground down.
              const lr = paper ? r0 - c[0]! : c[0]! > r0 ? c[0]! - r0 : 0;
              const lg = paper ? g0 - c[1]! : c[1]! > g0 ? c[1]! - g0 : 0;
              const lb = paper ? b0 - c[2]! : c[2]! > b0 ? c[2]! - b0 : 0;
              const m = lr > lg ? Math.max(lr, lb) : Math.max(lg, lb);
              if (gi < 0 || m < 0.004) gi = 0;
              else {
                // A full-bleed backdrop is lifted less: characters stay a texture of light, not a wall of type.
                const lift = ((full ? 0.3 + 0.48 * Math.sqrt(v!) : 0.42 + 0.58 * Math.sqrt(v!)) * 255) / m;
                data[o] = paper ? r0 * 255 - lr * lift : lr * lift;
                data[o + 1] = paper ? g0 * 255 - lg * lift : lg * lift;
                data[o + 2] = paper ? b0 * 255 - lb * lift : lb * lift;
              }
            }
          }
          data[o + 3] = gi > 0 ? 255 : 0;
          if (held[i] !== gi) {
            held[i] = gi;
            mctx.clearRect(x * gw, dy, gw, gh);
            if (gi > 0) mctx.drawImage(atlas, gi * gw, 0, gw, gh, x * gw, dy, gw, gh);
          }
        }
      }
      tctx.putImageData(image, 0, 0);

      // 2. Colour the masks: one cell-sized pixel per glyph, scaled up hard.
      ctx.globalAlpha = 1;
      ctx.globalCompositeOperation = "copy";
      ctx.drawImage(mask, 0, 0);
      ctx.globalCompositeOperation = "source-in";
      ctx.imageSmoothingEnabled = false;
      ctx.drawImage(tint, 0, 0, W, H);
      // 3. A soft halo under the characters: the same colours scaled up smoothly.
      ctx.globalCompositeOperation = "destination-over";
      ctx.imageSmoothingEnabled = true;
      ctx.globalAlpha = glow;
      ctx.drawImage(tint, 0, 0, W, H);
      ctx.globalAlpha = 1;
      ctx.globalCompositeOperation = "source-over";
      // The ground is the host's background (every palette's shadow stop is --sheet).
      return true;
    },
  };
}

/** Cells of the soft image's light mask (CSS px); the mask is scaled up smoothly. */
export const SOFT_MASK_CELL = 12;

/**
 * The image itself, cover-fitted and dimmed: drawn from a cached copy (recoloured once for a
 * palette or mono), drifting very slightly and leaning with the pointer, under a light mask
 * from the field (gain, hollow, spot, pulses) that is ordered-dithered before it is scaled up.
 */
export function softImageRenderer(el: Surface): Renderer | null {
  const ctx = context(el);
  const mask = scratch(2, 2);
  const mctx = context(mask);
  if (!ctx || !mctx) return null;
  let w = 0;
  let h = 0;
  let mw = 0;
  let mh = 0;
  let src: ImageSource | null = null;
  let cache: Surface | null = null;
  let cacheKey = "";
  let mimg: ImageData | null = null;
  const c = new Float32Array(4);

  /** The image at stage size with a little overscan for drift, recoloured unless it keeps its own colours. */
  function prepare(look: Look, field: Field) {
    if (!src) return null;
    const own = !!look.imageColour;
    const key = `${w}x${h}/${own ? "own" : `${field.lut[600]},${field.lut[601]},${field.lut[602]}`}`;
    if (cache && key === cacheKey) return cache;
    const cw = Math.round(w * 1.06);
    const ch = Math.round(h * 1.06);
    const out = scratch(cw, ch);
    const o = context(out);
    if (!o) return null;
    const k = Math.max(cw / src.width, ch / src.height);
    o.imageSmoothingQuality = "high";
    o.drawImage(src, (cw - src.width * k) / 2, (ch - src.height * k) / 2, src.width * k, src.height * k);
    if (!own) {
      // Keep the image's light, take the palette's colour: the light stop's hue over its luminosity.
      const L = field.lut;
      o.globalCompositeOperation = "color";
      o.fillStyle = `rgb(${Math.round(L[600]! * 255)} ${Math.round(L[601]! * 255)} ${Math.round(L[602]! * 255)})`;
      o.fillRect(0, 0, cw, ch);
      o.globalCompositeOperation = "source-over";
    }
    cache = out;
    cacheKey = key;
    return out;
  }

  return {
    setImage(next) {
      src = next;
      cache = null;
      cacheKey = "";
    },
    resize(m) {
      const L = softImageLayout(m.width, m.height, m.dpr);
      const nmw = Math.max(2, Math.ceil(m.width / SOFT_MASK_CELL));
      const nmh = Math.max(2, Math.ceil(m.height / SOFT_MASK_CELL));
      if (L.w === w && L.h === h && nmw === mw && nmh === mh) return;
      w = L.w;
      h = L.h;
      el.width = w;
      el.height = h;
      mw = nmw;
      mh = nmh;
      mask.width = mw;
      mask.height = mh;
      mimg = mctx.createImageData(mw, mh);
      cache = null;
    },
    draw(field, look, now) {
      if (!src || !mimg || !w) return false;
      const aspect = w / h;
      field.step(look, now, aspect);
      const img = prepare(look, field);
      if (!img) return false;
      // The light mask: the field's level per cell (an even, bright image gives the strength the
      // gain, hollow and spot leave), dithered so its gradient does not band.
      const d = mimg.data;
      let o = 0;
      field.cellRows = mh;
      for (let y = 0; y < mh; y++) {
        const py = (y + 0.5) / mh;
        const brow = (y & 7) * 8;
        for (let x = 0; x < mw; x++, o += 4) {
          field.shade(((x + 0.5) / mw) * aspect, py, c);
          const a = Math.min(1, c[3]! * 1.25) * 255 + BAYER[brow + (x & 7)]! * 6;
          d[o] = d[o + 1] = d[o + 2] = 255;
          d[o + 3] = a < 0 ? 0 : Math.min(a, 255);
        }
      }
      mctx.putImageData(mimg, 0, 0);
      // The picture, drifting a hair and leaning with the pointer (the field's own parallax).
      const t = field.clockNow;
      const [lx, ly] = field.parallax;
      const ox = (img.width - w) / 2;
      const oy = (img.height - h) / 2;
      const dx = -ox + Math.sin(t * 0.021) * ox * 0.5 - lx! * ox * 6;
      const dy = -oy + Math.cos(t * 0.017) * oy * 0.5 - ly! * oy * 6;
      ctx.globalCompositeOperation = "copy";
      ctx.globalAlpha = 1;
      ctx.imageSmoothingEnabled = true;
      ctx.drawImage(img, dx, dy);
      ctx.globalCompositeOperation = "destination-in";
      ctx.drawImage(mask, 0, 0, w, h);
      ctx.globalCompositeOperation = "source-over";
      return true;
    },
  };
}
