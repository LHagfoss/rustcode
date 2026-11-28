// design-lint: allow-file (the backdrop's palettes are light, not interface colour)
// Ported from Sable (https://github.com/Leonxlnx/sable, src/art/field.ts), MIT License,
// Copyright (c) 2026 Leonxlnx. The engine: the clock and frame loop, wherever it runs.

import { Field } from "./field";
import type {
  DitherTexture,
  FieldBeat,
  ImageSource,
  Look,
  Metrics,
  Renderer,
  Source,
  Surface,
} from "./fieldBase";
import { asciiRenderer, ditherRenderer, softImageRenderer } from "./fieldRender";

// ── engine: the clock and frame loop, wherever it runs ──

/** Frame interval in ms: ~16 fps for dither cells; glyphs cost a blit per change and a full-size composite, so ~9 fps. */
const FRAME_MS: Record<DitherTexture, number> = { dither: 60, ascii: 110, halftone: 90, soft: 60 };
/** The soft image redraws a stage-sized picture each frame; it moves so slowly that ~12 fps is plenty. */
const SOFT_IMAGE_MS = 84;

function makeRenderer(surface: Surface, texture: DitherTexture, image: boolean): Renderer | null {
  if (texture === "ascii" || texture === "halftone") return asciiRenderer(surface, texture);
  if (texture === "soft" && image) return softImageRenderer(surface);
  return ditherRenderer(surface, texture === "soft");
}

/** Frame interval while a beat is travelling, per quality level (a beat at full quality draws at ~36 fps). */
const BEAT_FRAME_MS = [28, 44, Infinity, Infinity, Infinity];

/**
 * Adaptive quality. A busy machine is the normal case, so the field watches whether it keeps up
 * and steps down on its own: 0 full, 1 the effects shed (beats lose their bending and echo, no
 * pointer light, beats draw slower), 2 a lower frame rate, 3 also coarser cells (the host re-sizes
 * the stage with QUALITY_CELL), 4 a still frame. It steps back up after a quiet spell.
 *
 * Signals, judged once a second: the field's own frame cost against its interval, how late its
 * timer fires (a late timer means the CPU is contended, wherever the engine runs), and the main
 * thread's load as reported by the host (ms of long animation frames per second, setPressure).
 * Three stressed seconds in a row step down; `quietNeed` quiet ms step up. Bouncing straight back
 * down after a step up doubles `quietNeed` (up to a minute), so a busy machine does not flicker
 * between levels. At level 4 nothing is drawn; a cheap 1 s probe timer keeps judging.
 *
 * Rest: once nothing moves (the drift has eased to a standstill under `setRest`, or a waiting
 * field's speed is 0, and no beat or light is easing) the loop stops entirely: no timer, no frame.
 * Any change (a look, the pointer, a beat, waking) starts it again.
 */
export const QUALITY_INTERVAL = [1, 1, 1.6, 2.4, 2.4];
export const QUALITY_CELL = [1, 1, 1, 1.5, 1.5];
export const MAX_QUALITY = 4;
const STRESS_LATE_MS = 40;
const STRESS_COST = 0.4;
const STRESS_MAIN_MS = 200;
const QUIET_MS = 8000;
/** Stressed seconds in a row before a step down (Windows timers alone are ~16 ms coarse). */
const STRESS_WINDOWS = 3;
const QUIET_MAX_MS = 64_000;
const BOUNCE_MS = 15_000;
const PROBE_MS = 1000;

export interface EngineEvents {
  /** After a frame was drawn: `first` once, and a smoothed frame cost every ~20 frames. */
  frame(first: boolean, ms: number | null): void;
  /** The quality level changed (0 full .. MAX_QUALITY still). */
  quality?(level: number): void;
}

export class FieldEngine {
  private readonly field: Field;
  private renderer: Renderer;
  private texture: DitherTexture;
  private image: ImageSource | null = null;
  private metrics: Metrics | null = null;
  private atlas: Source | null = null;
  private look: Look;
  private running = false;
  private animate = false;
  /** The loop stopped because nothing moved (see `wake`). */
  private halted = false;
  /** A frozen clock (development frame sheets), or null. */
  private fixed: number | null = null;
  private timer: ReturnType<typeof setTimeout> | 0 = 0;
  private settle: ReturnType<typeof setInterval> | 0 = 0;
  private drawn = false;
  /** The last 20 frame costs; their median is reported, so one stall on a busy machine does not skew it. */
  private readonly costs = new Float32Array(20);
  private count = 0;
  private interval: number;
  // ── governor ──
  private quality = 0;
  /** When the pending timer was due, to measure how late it fired. */
  private due = 0;
  private winStart = 0;
  private winN = 0;
  private winLate = 0;
  private winCost = 0;
  private stressed = 0;
  private quietSince = 0;
  private quietNeed = QUIET_MS;
  private lastUp = -Infinity;
  /** Main-thread load from the host: ms of long frames in the last second. */
  private mainLoad = 0;

  static create(
    surface: Surface,
    texture: DitherTexture,
    look: Look,
    events: EngineEvents,
  ): FieldEngine | null {
    const r = makeRenderer(surface, texture, false);
    return r ? new FieldEngine(surface, r, texture, look, events) : null;
  }

  private constructor(
    private readonly surface: Surface,
    renderer: Renderer,
    texture: DitherTexture,
    look: Look,
    private readonly events: EngineEvents,
  ) {
    this.renderer = renderer;
    this.texture = texture;
    this.look = look;
    this.field = new Field(look);
    this.interval = FRAME_MS[texture];
  }

  /** New size, and for glyphs the atlas drawn for it (sent whenever the glyph cell changes). */
  resize(m: Metrics, atlas?: Source) {
    this.metrics = m;
    if (atlas) this.atlas = atlas;
    this.renderer.resize(m);
    if (atlas) this.renderer.setAtlas?.(atlas);
    this.draw();
  }

  /**
   * Another texture on the same canvas (the host re-sizes the element and, for glyphs, sends a
   * new atlas). The clock, eased look and governor carry on.
   */
  setTexture(texture: DitherTexture) {
    if (texture === this.texture) return;
    this.texture = texture;
    this.rebuild();
  }

  /** An image in place of the pattern (null: back to the pattern). */
  setImage(image: ImageSource | null) {
    if (image === this.image) return;
    const was = this.softImage;
    this.image = image;
    this.applyImage();
    if (this.softImage !== was) this.rebuild();
    else {
      this.renderer.setImage?.(image);
      this.draw();
    }
  }

  private get softImage() {
    return this.texture === "soft" && this.image != null;
  }

  /** The soft image draws the picture itself, under a mask of even light; every other texture samples it. */
  private applyImage() {
    const soft = this.softImage;
    this.field.flat = soft;
    this.field.setImage(soft ? null : this.image);
  }

  private rebuild() {
    const r = makeRenderer(this.surface, this.texture, this.image != null);
    if (!r) return;
    this.renderer = r;
    this.interval = this.softImage ? SOFT_IMAGE_MS : FRAME_MS[this.texture];
    this.applyImage();
    r.setImage?.(this.image);
    if (this.metrics) r.resize(this.metrics);
    if (this.atlas) r.setAtlas?.(this.atlas);
    this.draw();
  }

  setLook(look: Look) {
    this.look = { ...look, pointer: this.look.pointer };
    if (this.animate && this.running && this.quality < MAX_QUALITY) return this.wake();
    // Still mode: run a short burst so the change eases in.
    if (this.settle) clearInterval(this.settle);
    let n = 0;
    this.settle = setInterval(() => {
      this.draw();
      if (++n > 14 && this.settle) {
        clearInterval(this.settle);
        this.settle = 0;
      }
    }, 70);
  }

  setPointer(p: [number, number] | null) {
    this.look.pointer = p;
    this.wake();
  }

  /** Main-thread load reported by the host: ms spent in long frames during the last second. */
  setPressure(ms: number) {
    this.mainLoad = ms;
  }

  /** A beat from (nx, ny), each 0..1 across the field. Ignored unless the field is moving. */
  beat(kind: FieldBeat, nx: number, ny: number, strength: number) {
    if (!this.animate || !this.running || this.quality >= MAX_QUALITY) return;
    this.field.beat(kind, nx, ny, strength, this.now());
    // Start on the next frame instead of waiting out the current interval.
    this.halted = false;
    this.schedule(0);
  }

  /** Rest (no input and no running turn for a while): the drift eases to a still frame and the loop stops. */
  setRest(rest: boolean) {
    this.field.setRest(rest);
    if (!rest) this.wake();
  }

  /** `animate`: the field moves at all. `running`: the host says it is visible, uncovered and in the foreground. */
  setState(animate: boolean, running: boolean) {
    this.animate = animate;
    const on = animate && running;
    if (on === this.running) return;
    this.running = on;
    this.halted = false;
    this.resetWindow();
    if (on) this.schedule(0);
    else this.stop();
  }

  /** Starts drawing again after the loop stopped because nothing moved. */
  private wake() {
    if (!this.halted || !this.running) return;
    this.halted = false;
    this.resetWindow();
    this.schedule(0);
  }

  /**
   * Development only (frame sheets): `advance` freezes the field's clock (at the current time the
   * first call) and moves it on by that many ms, drawing one frame; null lets it run again.
   */
  clock(advance: number | null) {
    if (advance == null) {
      if (this.fixed == null) return;
      this.fixed = null;
      this.halted = false;
      if (this.running) this.schedule(0);
      return;
    }
    if (this.fixed == null) this.fixed = performance.now();
    this.fixed += advance;
    this.stop();
    this.draw();
  }

  private now() {
    return this.fixed ?? performance.now();
  }

  dispose() {
    this.running = false;
    this.stop();
    if (this.settle) clearInterval(this.settle);
    this.settle = 0;
  }

  private stop() {
    if (this.timer) clearTimeout(this.timer);
    this.timer = 0;
  }

  private schedule(wait: number) {
    this.stop();
    if (this.fixed != null) return;
    this.due = performance.now() + wait;
    this.timer = setTimeout(this.quality >= MAX_QUALITY ? this.probe : this.tick, wait);
  }

  private tick = () => {
    this.timer = 0;
    if (!this.running) return;
    const t0 = performance.now();
    const late = t0 - this.due;
    this.draw();
    const cost = performance.now() - t0;
    // A slow drift needs fewer frames to look just as smooth (beats still draw at their own rate below).
    const slow = this.look.speed < 0.75 ? 1.35 : 1;
    // Light moving over a still image: the picture holds the eye, so fewer frames read as smooth
    // and the frame budget stays that of the pattern alone.
    const base = this.interval * QUALITY_INTERVAL[this.quality]! * slow * (this.field.overlaying ? 1.5 : 1);
    this.judge(late, cost, base);
    if (!this.running) return;
    // Nothing moves any more (rest, or a waiting field that has stilled): stop until something changes.
    if (this.field.still && this.quality < MAX_QUALITY) {
      this.halted = true;
      return;
    }
    // While a beat travels the field draws faster, so the front moves smoothly.
    const interval = this.field.pulsing ? Math.min(base, BEAT_FRAME_MS[this.quality]!) : base;
    this.schedule(this.quality >= MAX_QUALITY ? PROBE_MS : Math.max(4, interval - cost));
  };

  /** Level 3: nothing is drawn; a timer only checks whether the machine is quiet again. */
  private probe = () => {
    this.timer = 0;
    if (!this.running) return;
    this.judge(performance.now() - this.due, 0, Infinity);
    if (!this.running) return;
    this.schedule(this.quality >= MAX_QUALITY ? PROBE_MS : 0);
  };

  private resetWindow() {
    const now = performance.now();
    this.winStart = now;
    this.winN = this.winLate = this.winCost = 0;
    this.stressed = 0;
    this.quietSince = now;
  }

  private judge(late: number, cost: number, interval: number) {
    this.winN++;
    this.winLate += Math.max(late, 0);
    this.winCost += cost;
    const now = performance.now();
    if (now - this.winStart < 1000) return;
    const avgLate = this.winLate / this.winN;
    const avgCost = this.winCost / this.winN;
    this.winStart = now;
    this.winN = this.winLate = this.winCost = 0;
    const stressed =
      avgLate > STRESS_LATE_MS || avgCost > interval * STRESS_COST || this.mainLoad > STRESS_MAIN_MS;
    if (stressed) {
      this.quietSince = now;
      if (++this.stressed >= STRESS_WINDOWS && this.quality < MAX_QUALITY) {
        this.stressed = 0;
        // Straight back down after a step up: wait longer before the next try.
        if (now - this.lastUp < BOUNCE_MS) this.quietNeed = Math.min(this.quietNeed * 2, QUIET_MAX_MS);
        this.setQuality(this.quality + 1);
      }
      return;
    }
    this.stressed = 0;
    if (now - this.lastUp > 60_000) this.quietNeed = QUIET_MS;
    if (this.quality > 0 && now - this.quietSince >= this.quietNeed) {
      this.lastUp = now;
      this.quietSince = now;
      this.setQuality(this.quality - 1);
    }
  }

  private setQuality(q: number) {
    if (q === this.quality) return;
    this.quality = q;
    this.field.lite = q >= 1;
    this.events.quality?.(q);
  }

  private draw() {
    const t0 = performance.now();
    if (!this.renderer.draw(this.field, this.look, this.now())) return;
    const d = performance.now() - t0;
    this.costs[this.count % 20] = d;
    const first = !this.drawn;
    this.drawn = true;
    if (first) this.events.frame(true, null);
    else if (++this.count % 20 === 0)
      this.events.frame(false, [...this.costs].sort((a, b) => a - b)[10] ?? null);
  }
}
