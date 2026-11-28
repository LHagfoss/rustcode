// design-lint: allow-file (the backdrop's palettes are light, not interface colour)
// Ported from Sable (https://github.com/Leonxlnx/sable, src/art/field.ts), MIT License,
// Copyright (c) 2026 Leonxlnx. The field: eased look, clock, beats and the per-point shade.

import {
  BEAT_MS,
  type DitherVariant,
  type FieldBeat,
  FS,
  FULL_GAIN,
  fcos,
  getTables,
  HOLLOW,
  HOLLOW_RX,
  HOLLOW_RY,
  HOLLOW_Y,
  HORIZON_APEX,
  HORIZON_GLOW,
  HORIZON_GROUND,
  HORIZON_RIM,
  HORIZON_SAG,
  HUE_N,
  HUES,
  type ImagePixels,
  KNEE,
  LIGHT_K,
  LIGHT_R,
  LIGHT_W,
  type Look,
  MAX_BEATS,
  MAX_FRONTS,
  paletteOf,
  type RGB,
  ROLL,
  rasterize,
  STREAK_COS,
  STREAK_SIN,
  STREAKS_COS,
  STREAKS_SIN,
  sample,
  sstep,
  TILE_H,
  TILE_HALF,
  TILE_R,
  TILE_REACH,
  TILE_RIM,
  TILE_SPAN,
  TILE_SU,
  TILE_SV,
  TUBE_G,
  TUBE_G_FULL,
  TUBE_REACH,
  TUBE_SPAN,
  WAVE_K,
  WAVE_K_FULL,
  WAVE_W,
  WAVE_W_FULL,
} from "./fieldBase";

// ── the field (shared by both textures) ──

/**
 * The eased look and clock of one field, and the per-point evaluation. `step` once per
 * frame, then `shade` every cell inside `span`. Coordinates are in units of the field's
 * height: px in 0..aspect left to right, py in 0..1 top to bottom.
 */
export class Field {
  /** Palette ramp, 256 entries of rgb in 0..1. `lut[0..2]` is the ground colour. */
  readonly lut = new Float32Array(256 * 3);
  private readonly A: Float32Array;
  private readonly B: Float32Array;
  // Eased state, so palette / gain / focus changes cross-fade instead of snapping.
  private readonly colors: RGB[];
  private spectrum: number;
  private readonly focusNow: number[];
  private readonly par = [0, 0];
  private gainNow: number;
  private fullNow: number;
  private clock: number;
  private last = 0;
  private first = true;
  // Per-frame constants read by shade().
  private t = 0;
  private aspect = 1;
  private fx = 0;
  private fy = 0;
  /** The vanishing point: the focus, nudged toward the pointer. */
  private vx = 0;
  private vy = 0;
  private reach = Infinity;
  private full = false;
  private mode: DitherVariant = "bloom";
  private sd = 0;
  private wc = 1;
  private ws = 0;
  private l1x = 0;
  private l1y = 0;
  private l2x = 0;
  private l2y = 0;
  private breath = 1;
  private fullK = 1;
  private hollowK = 0;
  private tubeG = TUBE_G;
  private waveK = WAVE_K;
  private waveW = WAVE_W;
  // Per-cell cache: tubes and tiles are evaluated once per tube or tile, not once per pixel.
  private ci = 1e9;
  private cj = 1e9;
  private c0 = 0;
  private c1 = 0;
  private c2 = 0;
  private c3 = 0;
  private c4 = 0;
  private c5 = 0;
  private c6 = 0;
  // Per-row cache for tiles.
  private ry = -1;
  private rh = 0;
  private riz = 0;
  private riv = 0;
  private rlv = 0;
  /** The hue the variant chose for this point (spectrum palette). */
  private hue = 0;
  // Per-row cache for the hollow.
  private hy = -1;
  private hq = 0;
  // Beats: one-shot disturbances travelling through the cells (see FieldBeat). Each frame they are
  // turned into a few fronts (rings or soft disks); `fn` is how many are live, so a quiet field pays
  // one test per cell. Per point, `wave` leaves the light to add, the factor to multiply by and how
  // far the pattern is pushed.
  private readonly beats: { kind: FieldBeat; x: number; y: number; t0: number; s: number }[] = [];
  private fn = 0;
  private readonly pf = new Float32Array(MAX_FRONTS * FS);
  private lift = 0;
  private wadd = 0;
  private wmul = 1;
  private wdx = 0;
  private wdy = 0;
  /** Under load the governor sheds the costly parts first: no bending, no echo, no pointer light. */
  lite = false;
  // Eased speed: working drifts faster, waiting and rest ease to a standstill instead of snapping.
  private speedNow: number;
  private restK = 1;
  /** True once nothing moves: no drift, no beat, no light easing (the engine stops drawing). */
  still = false;
  // The pointer's light: a critically damped follower, and how present it is (fades in and out).
  private plx = 0;
  private ply = 0;
  private pvx = 0;
  private pvy = 0;
  private plI = 0;
  private plOn = false;
  /** Per-frame: the light's centre in field units and its strength (0 when off). */
  private lx = 0;
  private ly = 0;
  private lI = 0;

  /** How strongly beats show (Look.react). */
  private react = 1;

  /** Starts a beat at (nx, ny), each 0..1 across the field. */
  beat(kind: FieldBeat, nx: number, ny: number, strength: number, now: number) {
    if (this.beats.length >= MAX_BEATS) this.beats.shift();
    this.beats.push({ kind, x: nx, y: ny, t0: now, s: strength < 0 ? 0 : Math.min(strength, 1.6) });
    this.still = false;
  }

  /** True while a beat is travelling (the engine draws faster meanwhile). */
  get pulsing() {
    return this.beats.length > 0;
  }

  /** Rest: the drift eases to a standstill (and back when woken). */
  setRest(rest: boolean) {
    this.restK = rest ? 0 : 1;
    if (!rest) this.still = false;
  }

  // The spot (Look.spot), eased: centre in stage units (0..1 both ways) and reach.
  private readonly spotNow = [0.5, 0.5, 1];
  private spotOn = false;
  private spX = 0.5;
  private spY = 0.5;
  private spR = 1;
  private spIn = 0;
  private spEven = 0;
  /** Streak direction (cos, sin). */
  private sc = STREAKS_COS;
  private ss = STREAKS_SIN;
  // The image that replaces the pattern, and its pixels for the current aspect.
  private imageSrc: (CanvasImageSource & { width: number; height: number }) | null = null;
  private px: ImagePixels | null = null;
  private imgColour = false;
  /** Strength of the moving pattern over an image (Look.overlay). */
  private overlay = 0;
  /** The image colour at the point just shaded (image colours only). */
  private ir = 0;
  private ig = 0;
  private ib = 0;
  private ilum = 0;

  /** The image to draw instead of the pattern (null: the pattern). Rasterized on the next step. */
  setImage(src: (CanvasImageSource & { width: number; height: number }) | null) {
    this.imageSrc = src;
    this.px = null;
  }

  get hasImage() {
    return this.imageSrc != null;
  }

  /** Even light in place of the pattern (the soft image's light mask: the image itself is drawn as it is). */
  flat = false;

  /** The field's clock (s, scaled by speed) and the pointer's eased lean, for renderers that move on their own. */
  get clockNow() {
    return this.clock;
  }

  get parallax(): readonly number[] {
    return this.par;
  }

  constructor(look: Look) {
    [this.A, this.B] = getTables();
    this.colors = paletteOf(look).map((c) => [...c]) as RGB[];
    this.spectrum = look.palette === "spectrum" ? 1 : 0;
    this.focusNow = look.full ? [0.5, 0.5] : [...look.focus];
    this.gainNow = look.gain;
    this.fullNow = look.full ? 1 : 0;
    this.clock = look.seed * 10;
    this.speedNow = look.speed;
    if (look.spot) [this.spotNow[0], this.spotNow[1], this.spotNow[2]] = look.spot;
  }

  step(s: Look, now: number, aspect: number) {
    const dt = this.first ? 0 : Math.min(0.25, (now - this.last) / 1000);
    this.last = now;
    // Speed eases (about half a second): a turn starting quickens the drift, waiting and rest still it.
    const sp = s.speed * this.restK;
    const was = this.speedNow;
    this.speedNow += (sp - this.speedNow) * (this.first ? 1 : Math.min(1, dt * 2.2));
    if (Math.abs(sp - this.speedNow) < 0.004 * Math.max(0.05, s.speed)) this.speedNow = sp;
    this.clock += dt * (was + this.speedNow) * 0.5;
    const k = this.first ? 1 : Math.min(1, dt * 3);
    const kp = this.first ? 1 : Math.min(1, dt * 2.2);
    this.first = false;
    // How much the eased look still moves this frame (the engine stops drawing once nothing does).
    let moved = Math.abs(this.speedNow) + Math.abs(sp - this.speedNow);
    const target = paletteOf(s);
    const colors = this.colors;
    for (let i = 0; i < 4; i++)
      for (let j = 0; j < 3; j++) {
        const d = target[i]![j]! - colors[i]![j]!;
        colors[i]![j]! += d * k;
        moved += Math.abs(d) * 0.004;
      }
    this.spectrum += ((s.palette === "spectrum" ? 1 : 0) - this.spectrum) * k;
    const focus = s.full ? [0.5, 0.5] : s.focus;
    moved +=
      Math.abs(focus[0]! - this.focusNow[0]!) +
      Math.abs(focus[1]! - this.focusNow[1]!) +
      Math.abs(s.gain - this.gainNow);
    this.focusNow[0]! += (focus[0]! - this.focusNow[0]!) * k;
    this.focusNow[1]! += (focus[1]! - this.focusNow[1]!) * k;
    this.gainNow += (s.gain - this.gainNow) * k;
    this.fullNow += ((s.full ? 1 : 0) - this.fullNow) * k;
    const p = s.pointer;
    const parX = p ? p[0] * 0.07 : 0;
    const parY = p ? p[1] * 0.05 : 0;
    moved += Math.abs(parX - this.par[0]!) + Math.abs(parY - this.par[1]!);
    this.par[0]! += (parX - this.par[0]!) * kp;
    this.par[1]! += (parY - this.par[1]!) * kp;
    // The pointer's light (full stages only): follows on a critically damped spring, so it trails
    // the pointer by a fraction of a second and comes to rest without overshoot.
    const lightOn = !!p && s.full && !this.lite && (s.react ?? 1) > 0;
    if (lightOn && p) {
      const tx = (p[0] + 0.5) * aspect;
      const ty = p[1] + 0.5;
      if (!this.plOn) {
        this.plx = tx;
        this.ply = ty;
        this.pvx = this.pvy = 0;
      }
      const n = Math.max(1, Math.ceil(dt / 0.016));
      for (let i = 0; i < n; i++) {
        const h = dt / n;
        this.pvx += (LIGHT_W * LIGHT_W * (tx - this.plx) - 2 * LIGHT_W * this.pvx) * h;
        this.pvy += (LIGHT_W * LIGHT_W * (ty - this.ply) - 2 * LIGHT_W * this.pvy) * h;
        this.plx += this.pvx * h;
        this.ply += this.pvy * h;
      }
      moved += Math.abs(this.pvx) + Math.abs(this.pvy);
    }
    this.plOn = lightOn;
    const li = lightOn ? 1 : 0;
    moved += Math.abs(li - this.plI);
    this.plI += (li - this.plI) * Math.min(1, dt * 1.6);
    if (Math.abs(li - this.plI) < 0.01) this.plI = li;
    this.lx = this.plx;
    this.ly = this.ply;
    this.lI = this.plI * LIGHT_K;
    this.react = s.react ?? 1;
    // The spot eases like the focus, so dragging it in Settings glides.
    const spot = s.spot ?? null;
    const sn = this.spotNow;
    const ks = this.first ? 1 : Math.min(1, dt * 6);
    sn[0]! += ((spot ? spot[0] : 0.5) - sn[0]!) * ks;
    sn[1]! += ((spot ? spot[1] : 0.5) - sn[1]!) * ks;
    sn[2]! += ((spot ? spot[2] : 1) - sn[2]!) * ks;
    moved +=
      Math.abs((spot ? spot[0] : 0.5) - sn[0]!) +
      Math.abs((spot ? spot[1] : 0.5) - sn[1]!) +
      Math.abs((spot ? spot[2] : 1) - sn[2]!);
    if (Math.abs(sn[2]! - (spot ? spot[2] : 1)) < 0.002) sn[2] = spot ? spot[2] : 1;
    // Reach → falloff radius (in stage units, so the light spreads as an ellipse matching the
    // stage); near full reach the falloff blends into even light.
    const reach = sn[2]!;
    this.spotOn = reach < 0.999;
    this.spX = sn[0]!;
    this.spY = sn[1]!;
    this.spR = 0.1 + 1.8 * reach ** 1.3;
    this.spIn = 0;
    this.spEven = sstep(0.86, 1, reach!);
    const ang = s.angle == null ? null : (s.angle * Math.PI) / 180;
    this.sc = ang == null ? STREAKS_COS : Math.cos(ang);
    this.ss = ang == null ? STREAKS_SIN : Math.sin(ang);
    this.imgColour = !!s.imageColour;
    this.overlay = s.overlay && s.overlay > 0 ? Math.min(1, s.overlay) : 0;
    this.ogFrame = (this.ogFrame + 1) >>> 0 || 1;
    if (this.imageSrc && (!this.px || Math.abs(this.px.aspect - aspect) > 0.01))
      this.px = rasterize(this.imageSrc, aspect);

    const lut = this.lut;
    for (let i = 0; i < 256; i++) {
      const t = i / 255;
      const a = sstep(0, 0.4, t);
      const b = sstep(0.28, 0.74, t);
      const c = sstep(0.7, 1, t);
      for (let j = 0; j < 3; j++) {
        let v = colors[0]![j]! + (colors[1]![j]! - colors[0]![j]!) * a;
        v += (colors[2]![j]! - v) * b;
        v += (colors[3]![j]! - v) * c;
        lut[i * 3 + j] = v / 255;
      }
    }

    const t = this.clock;
    this.t = t;
    this.aspect = aspect;
    this.full = s.full;
    this.fx = this.focusNow[0]! * aspect;
    this.fy = this.focusNow[1]!;
    this.vx = (this.focusNow[0]! + this.par[0]!) * aspect;
    this.vy = this.focusNow[1]! + this.par[1]!;
    this.mode = s.variant;
    this.sd = s.seed * 0.137;
    this.reach = s.full
      ? Infinity
      : s.variant === "tubes"
        ? TUBE_SPAN
        : s.variant === "tiles"
          ? TILE_SPAN
          : Infinity;
    this.tubeG = s.full ? TUBE_G_FULL : TUBE_G;
    this.fullK = this.gainNow * (FULL_GAIN[s.variant] ?? 1);
    this.hollowK = HOLLOW * this.fullNow;
    this.waveK = s.full ? WAVE_K_FULL : WAVE_K;
    this.waveW = s.full ? WAVE_W_FULL : WAVE_W;
    // The wave's direction turns slowly: a full turn in about two minutes.
    // With an angle it holds that direction, swaying a little.
    const dir = ang == null ? t * 0.05 + this.sd * 7 : ang + 0.22 * Math.sin(t * 0.05);
    this.wc = Math.cos(dir);
    this.ws = Math.sin(dir);
    // A gentle breath, so a full stage of light is never quite still.
    this.breath = 0.92 + 0.08 * Math.sin(t * 0.42);
    // Two lights glide under the tiles on slow, unrelated loops; a full stage spreads them across its width.
    if (s.full) {
      this.l1x = aspect * (0.3 + 0.2 * Math.sin(t * 0.21 + this.sd * 9));
      this.l1y = 0.35 + 0.25 * Math.sin(t * 0.17 + 1.7);
      this.l2x = aspect * (0.7 + 0.2 * Math.sin(t * 0.15 + 3.1));
      this.l2y = 0.6 + 0.25 * Math.cos(t * 0.19);
    } else {
      this.l1x = this.fx - 0.3 + 0.24 * Math.sin(t * 0.45 + this.sd * 9);
      this.l1y = this.fy - 0.22 + 0.15 * Math.sin(t * 0.31 + 1.7);
      this.l2x = this.fx - 0.1 + 0.16 * Math.sin(t * 0.27 + 3.1);
      this.l2y = this.fy - 0.44 + 0.12 * Math.cos(t * 0.39);
    }
    this.ci = 1e9;
    this.cj = 1e9;
    this.ry = -1;
    this.hy = -1;

    this.planBeats(now, aspect);
    this.still = moved < 2e-4 && this.fn === 0;
  }

  /** Writes one front into the per-frame table (dropped when the table is full). */
  private front(
    cx: number,
    cy: number,
    r: number,
    lead: number,
    tail: number,
    amp: number,
    push: number,
    disk: boolean,
    mul: boolean,
    trough: number,
  ) {
    if (this.fn >= MAX_FRONTS) return;
    const o = this.fn++ * FS;
    const pf = this.pf;
    pf[o] = cx;
    pf[o + 1] = cy;
    pf[o + 2] = r;
    pf[o + 3] = lead;
    pf[o + 4] = tail;
    pf[o + 5] = amp;
    pf[o + 6] = this.lite ? 0 : push;
    pf[o + 7] = disk ? 1 : 0;
    pf[o + 8] = mul ? 1 : 0;
    pf[o + 9] = trough;
  }

  /**
   * The beats as this frame's fronts. Shapes are tuned in field heights, so a wave looks the same
   * on any stage; it always runs out past the farthest corner from where it started.
   */
  private planBeats(now: number, aspect: number) {
    this.fn = 0;
    let lift = 0;
    for (let i = this.beats.length - 1; i >= 0; i--) {
      const b = this.beats[i]!;
      const u = (now - b.t0) / BEAT_MS[b.kind];
      if (u >= 1 || u < -0.05) {
        this.beats.splice(i, 1);
        continue;
      }
      if (u < 0) continue;
      const cx = b.x * aspect;
      const cy = b.y;
      const fx = Math.max(cx, aspect - cx);
      const fy = Math.max(cy, 1 - cy);
      const far = Math.sqrt(fx * fx + fy * fy);
      const s = b.s;
      if (b.kind === "send" || b.kind === "pulse") {
        // The crest decelerates as it spreads (fast off the key, gliding out past the edges), thins
        // with distance like a ring on water, and widens behind as it goes.
        const e = 1 - (1 - u) ** 1.8;
        const r = 0.02 + e * far * 1.08;
        const env = (u < 0.04 ? u / 0.04 : 1) * (1 - sstep(0.35, 1, u));
        const amp = (s * env * (b.kind === "send" ? 1 : 0.62)) / (1 + 0.3 * r);
        const lead = 0.014 + 0.026 * e;
        const tail = 0.06 + 0.2 * e;
        // Bends the pattern by up to ~7% of the field's height at the crest.
        this.front(cx, cy, r, lead, tail, amp, 0.07, false, false, 0.34);
        lift += amp;
        // A weaker echo rides behind the crest.
        if (u > 0.07 && !this.lite)
          this.front(cx, cy, r * 0.62, lead * 1.4, tail * 0.8, amp * 0.36, 0.012, false, false, 0.2);
        // The key flashes as the wave leaves it.
        if (b.kind === "send" && u < 0.2) {
          const f = 1 - u / 0.2;
          this.front(cx, cy, 0.015 + 0.06 * (u / 0.2), 0.07, 0, s * 1.15 * f * f, 0, true, false, 0);
        }
      } else if (b.kind === "done") {
        // The turn settles: one wide, nearly flat crest of light comes down through the band from
        // above its top and lands on the point (the composer's top edge), across the whole width,
        // so it is seen beside and above the composer rather than behind it. It is the top of a
        // very large ring centred far below the point, so it bows gently, its ends a little ahead.
        // It decelerates as it lands, and the light there swells once and fades.
        const R = Math.max(3, aspect * 1.6);
        const travel = cy + 0.2;
        const e = 1 - (1 - u) ** 2.4;
        const r = R + travel * (1 - e);
        const env = (u < 0.08 ? u / 0.08 : 1) * (1 - sstep(0.62, 1, u));
        const amp = s * 1.25 * env;
        // The soft side trails above, the crisp side leads below.
        this.front(cx, cy + R, r, 0.26, 0.06, amp, -0.02, false, false, 0.22);
        lift += amp * 0.5;
        if (u > 0.55) {
          const q = Math.sin((Math.PI * (u - 0.55)) / 0.45);
          this.front(cx, cy, 0.08 + 0.1 * q, 0.22, 0, s * 0.32 * q, 0, true, false, 0);
        }
      } else {
        // fail: the light dips, the dimming spreading from the point within a third of the beat,
        // then recovers slowly. Multiplied, so dark stays dark.
        const g = Math.min(1, u / 0.3);
        const r = 0.04 + (1 - (1 - g) ** 3) * far * 1.05;
        const env = u < 0.07 ? u / 0.07 : (1 - (u - 0.07) / 0.93) ** 1.7;
        this.front(cx, cy, r, 0.4, 0, -0.52 * Math.min(1, s) * env, 0, true, true, 0);
      }
    }
    this.lift = Math.min(lift, 1);
  }

  /**
   * The beats at (px, py): light to add (`wadd`), a factor (`wmul`) and how far to push the
   * pattern (`wdx`, `wdy`, in field units). Only called while a beat is live.
   */
  private wave(px: number, py: number) {
    const pf = this.pf;
    let add = 0;
    let mul = 1;
    let dx = 0;
    let dy = 0;
    for (let o = 0, end = this.fn * FS; o < end; o += FS) {
      const ox = px - pf[o]!;
      const oy = py - pf[o + 1]!;
      const d2 = ox * ox + oy * oy;
      const r = pf[o + 2]!;
      const lead = pf[o + 3]!;
      const outer = r + lead;
      if (d2 > outer * outer) continue;
      if (pf[o + 7] !== 0) {
        // A soft disk: full inside, its edge falling away over `lead`.
        let f = 1;
        if (d2 > r * r) {
          const q = 1 - (Math.sqrt(d2) - r) / lead;
          f = q * q;
        }
        if (pf[o + 8] !== 0) mul *= 1 + pf[o + 5]! * f;
        else add += pf[o + 5]! * f;
        continue;
      }
      const tail = pf[o + 4]!;
      const trough = pf[o + 9]!;
      const inner = r - tail * (trough > 0 ? 1.75 : 1);
      if (inner > 0 && d2 < inner * inner) continue;
      const d = Math.sqrt(d2);
      const sd = d - r;
      let crest = 0;
      let low = 0;
      if (sd >= 0) {
        const q = 1 - sd / lead;
        crest = q * q;
      } else {
        const q = -sd / tail;
        if (q < 1) crest = (1 - q) * (1 - q) * (1 - q);
        // Behind the crest the light thins for a moment: the trough that makes the crest read.
        if (trough > 0 && q > 0.55 && q < 1.75) low = trough * fcos(((q - 1.15) / 1.2) * Math.PI);
      }
      const amp = pf[o + 5]!;
      add += amp * (crest - low);
      const push = pf[o + 6]! * amp * crest;
      if (push !== 0 && d > 1e-4) {
        dx += (ox / d) * push;
        dy += (oy / d) * push;
      }
    }
    this.wadd = add;
    this.wmul = mul;
    this.wdx = dx;
    this.wdy = dy;
  }

  /**
   * The part of row `py` that can be lit, as px from out[0] to out[1] (empty when out[0] > out[1]).
   * Outside it the light has fallen to nothing, so those cells are ground and are not evaluated.
   */
  span(py: number, out: Float32Array) {
    if (this.reach === Infinity) {
      out[0] = -1e9;
      out[1] = 1e9;
      // The spot leaves everything past its radius dark (unless it blends into even light).
      if (this.spotOn && this.spEven <= 0) {
        const dy = py - this.spY;
        const r2 = this.spR * this.spR - dy * dy;
        if (r2 <= 0) {
          out[0] = 1;
          out[1] = 0;
          return;
        }
        const r = Math.sqrt(r2);
        out[0] = (this.spX - r) * this.aspect;
        out[1] = (this.spX + r) * this.aspect;
      }
      return;
    }
    const dy = py - this.fy;
    const r2 = this.reach * this.reach - dy * dy;
    if (r2 <= 0) {
      out[0] = 1;
      out[1] = 0;
      return;
    }
    const r = Math.sqrt(r2);
    out[0] = this.fx - r;
    out[1] = this.fx + r;
  }

  /** Writes the colour (rgb in 0..1) into out[0..2] and the light level (0 = unlit) into out[3]. */
  shade(px: number, py: number, out: Float32Array) {
    // One small method per variant: each stays monomorphic and cheap for the JIT.
    const mode = this.mode;
    this.hue = 0;
    if (this.fn !== 0) {
      // A passing crest bends the pattern: it is read from a little closer to the beat's origin,
      // so the streaks and tiles visibly ripple outward with the light.
      this.wave(px, py);
      px -= this.wdx;
      py -= this.wdy;
    }
    let w = 1;
    if (this.spotOn) {
      w = this.spotAt(px, py);
      if (w <= 0) {
        // Past the spot: ground, without evaluating the pattern.
        out[0] = this.lut[0]!;
        out[1] = this.lut[1]!;
        out[2] = this.lut[2]!;
        out[3] = 0;
        return;
      }
    }
    const v =
      this.overlay > 0 && (this.flat || this.px)
        ? this.overlaid(this.flat ? 0.62 : this.image(px, py), px, py)
        : this.flat
          ? 0.62
          : this.px
            ? this.image(px, py)
            : mode === "streaks"
              ? this.streaks(px, py)
              : mode === "tubes"
                ? this.tubes(px, py)
                : mode === "tiles"
                  ? this.tiles(px, py)
                  : this.broad(px, py);
    this.finish(v, px, py, out, w);
  }

  /** How much of the light the spot lets through at (px, py): 1 at its centre, 0 past its reach. */
  private spotAt(px: number, py: number): number {
    const dx = px / this.aspect - this.spX;
    const dy = py - this.spY;
    const d = Math.sqrt(dx * dx + dy * dy);
    // A glow: brightest at the centre and thinning out all the way to the edge (no flat top, no rim).
    const u = d <= this.spIn ? 0 : d >= this.spR ? 1 : (d - this.spIn) / (this.spR - this.spIn);
    const q = 1 - u * u;
    const f = q * q;
    return f + (1 - f) * this.spEven;
  }

  /**
   * The moving pattern as light over an image's brightness `v`: the image steps back a little and
   * the pattern lifts it where it passes, brightest over the image's own light.
   */
  private overlaid(v: number, px: number, py: number): number {
    // The pattern is read from a grid at half the cell rows, each node worked out once per frame
    // and interpolated between (as the soft variants draw themselves), so the light over an image
    // costs about what the pattern alone does.
    const rows = this.cellRows > 0 ? Math.max(8, Math.ceil(this.cellRows / 2)) : 96;
    const cols = Math.ceil(this.aspect * rows) + 3;
    const n = (rows + 3) * cols;
    if (this.ogRows !== rows || this.ogCols !== cols) {
      this.ogRows = rows;
      this.ogCols = cols;
      if (this.og.length < n) {
        this.og = new Float32Array(n);
        this.ogStamp = new Uint32Array(n);
      } else this.ogStamp.fill(0);
    }
    let gx = px * rows;
    let gy = py * rows;
    gx = gx < 0 ? 0 : Math.min(gx, cols - 2);
    gy = gy < 0 ? 0 : Math.min(gy, rows + 1);
    const i = gx | 0;
    const j = gy | 0;
    const tx = gx - i;
    const ty = gy - j;
    const a = this.ogAt(i, j, rows, cols);
    const b = this.ogAt(i + 1, j, rows, cols);
    const c = this.ogAt(i, j + 1, rows, cols);
    const d = this.ogAt(i + 1, j + 1, rows, cols);
    const top = a + (b - a) * tx;
    const p = top + (c + (d - c) * tx - top) * ty;
    const k = this.overlay;
    return v * (1 - 0.35 * k) + k * Math.max(p, 0) * (0.3 + 0.7 * v);
  }

  /** The pattern at grid node (i, j) this frame (see `overlaid`). */
  private ogAt(i: number, j: number, rows: number, cols: number): number {
    const idx = j * cols + i;
    if (this.ogStamp[idx] === this.ogFrame) return this.og[idx]!;
    const x = i / rows;
    const y = j / rows;
    const mode = this.mode;
    const p =
      mode === "streaks"
        ? this.streaks(x, y)
        : mode === "tubes"
          ? this.tubes(x, y)
          : mode === "tiles"
            ? this.tiles(x, y)
            : this.broad(x, y);
    this.og[idx] = p;
    this.ogStamp[idx] = this.ogFrame;
    return p;
  }

  /** An image with a moving effect over it. */
  get overlaying() {
    return this.overlay > 0 && this.imageSrc != null;
  }

  /** Rows of cells the renderer draws this frame (sets the overlay grid's spacing; 0 unknown). */
  cellRows = 0;
  private og = new Float32Array(0);
  private ogStamp = new Uint32Array(0);
  private ogFrame = 1;
  private ogRows = 0;
  private ogCols = 0;

  /** An image's brightness, with a slow drift of the threshold so a moving field is never quite still. */
  private image(px: number, py: number): number {
    const P = this.px!;
    // Zoomed in a little, so the pointer can shift it without showing an edge.
    let u = 0.5 + (px / this.aspect - 0.5) * 0.95 - this.par[0]! * 0.6;
    let v = 0.5 + (py - 0.5) * 0.95 - this.par[1]! * 0.6;
    u = u < 0 ? 0 : Math.min(u, 1);
    v = v < 0 ? 0 : Math.min(v, 1);
    const fx = u * (P.w - 1);
    const fy = v * (P.h - 1);
    const x0 = fx | 0;
    const y0 = fy | 0;
    const x1 = x0 + 1 < P.w ? x0 + 1 : x0;
    const y1 = y0 + 1 < P.h ? y0 + 1 : y0;
    const tx = fx - x0;
    const ty = fy - y0;
    const a = y0 * P.w + x0;
    const b = y0 * P.w + x1;
    const c = y1 * P.w + x0;
    const d = y1 * P.w + x1;
    const L = P.lum;
    const top = L[a]! + (L[b]! - L[a]!) * tx;
    const lum = top + (L[c]! + (L[d]! - L[c]!) * tx - top) * ty;
    if (this.imgColour) {
      const R = P.rgb;
      const mix = (j: number) => {
        const t0 = R[a * 3 + j]! + (R[b * 3 + j]! - R[a * 3 + j]!) * tx;
        return t0 + (R[c * 3 + j]! + (R[d * 3 + j]! - R[c * 3 + j]!) * tx - t0) * ty;
      };
      this.ir = mix(0);
      this.ig = mix(1);
      this.ib = mix(2);
    }
    this.ilum = lum;
    const t = this.t;
    const n = sample(this.A, px * 0.32 + t * 0.012 + this.sd, py * 0.32 - t * 0.009);
    return lum * (0.84 + 0.32 * n);
  }

  private streaks(px: number, py: number): number {
    const A = this.A;
    const B = this.B;
    const t = this.t;
    const sd = this.sd;
    // Long soft bands of light along a diagonal, like a slow exposure of sunlit leaves: two
    // stretched noise layers of different widths, drifting along the streak, under a broad
    // light that is brightest toward the top right and leaves dark pockets.
    const a = px * this.sc - py * this.ss;
    const c = px * this.ss + py * this.sc;
    const drift = t * 0.011;
    const n1 = sample(A, a * 0.045 + drift + sd, c * 0.42);
    const n2 = sample(B, a * 0.085 + drift * 1.7, c * 1.25 + 0.37);
    const n3 = sample(A, a * 0.16 - drift * 0.6 + 0.5, c * 2.2 + sd);
    const bands = n1 * 0.6 + n2 * 0.42 + n3 * 0.16;
    const lit = sstep(0.22, 0.82, bands);
    const L = sample(B, px * 0.16 + sd * 0.5 + t * 0.0016, py * 0.16 - t * 0.0012);
    const corner = (px / this.aspect) * 0.55 + (1 - py) * 0.45;
    const v = lit * (0.5 + 0.6 * sstep(0.25, 0.75, L)) * (0.38 + 0.95 * corner) * this.breath;
    this.hue = a * 0.12 + n1 * 0.6 + t * 0.006;
    return v;
  }

  private tubes(px: number, py: number): number {
    const A = this.A;
    const t = this.t;
    const sd = this.sd;
    const full = this.full;
    let v: number;
    if (!full) {
      // Tubes never get brighter than ~1.7: where the falloff keeps even that under the floor, it is ground.
      const dx = px - this.fx;
      const dy = py - this.fy;
      if ((1 - sstep(0.12, TUBE_REACH, Math.sqrt(dx * dx + dy * dy))) * this.gainNow * 1.7 < 0.09) return 0;
    }
    // A grid of glowing tubes seen from above. Each leans away from the vanishing point,
    // more so the further out it stands. A wave of light travels across the grid along a
    // slowly turning direction: the rims catch it first, then the bores fill.
    const g = this.tubeG;
    const ix = Math.floor(px / g);
    const iy = Math.floor(py / g);
    const cx = (ix + 0.5) * g;
    const cy = (iy + 0.5) * g;
    if (ix !== this.ci || iy !== this.cj) {
      this.ci = ix;
      this.cj = iy;
      const ox = cx - this.vx;
      const oy = cy - this.vy;
      const od = Math.sqrt(ox * ox + oy * oy) || 1;
      const lean = Math.min(0.3, od * (full ? 0.22 : 0.34)) * g;
      this.c0 = (ox / od) * lean;
      this.c1 = (oy / od) * lean;
      const ph = ((cx * this.wc + cy * this.ws) / g) * this.waveK - t * this.waveW;
      const body = 0.5 + 0.5 * Math.cos(ph);
      const rim = 0.5 + 0.5 * Math.cos(ph - 0.75);
      const n = sample(A, ix * 0.07 + t * 0.03 + sd, iy * 0.07 - t * 0.022);
      if (full) {
        // A calm sweep: softer peaks, so the whole grid glows and the wave passes through it.
        this.c2 = 0.42 + 0.36 * n + 0.6 * body * body;
        this.c3 = 0.6 + 0.26 * n + 0.5 * rim * rim;
      } else {
        this.c2 = 0.34 + 0.4 * n + 0.95 * body * body * body;
        this.c3 = 0.52 + 0.28 * n + 0.8 * rim * rim * rim;
      }
      this.c4 = ix * 0.045 + iy * 0.03 + t * 0.045 + n * 0.5;
    }
    const lx = this.c0;
    const ly = this.c1;
    const R = g * 0.4;
    const rimW = g * 0.07;
    const tx = px - (cx + lx);
    const ty = py - (cy + ly);
    const dTop2 = tx * tx + ty * ty;
    const bx = px - cx;
    const by = py - cy;
    const ll = lx * lx + ly * ly || 1e-6;
    let u = (bx * lx + by * ly) / ll;
    u = u < 0 ? 0 : Math.min(u, 1);
    const ex = bx - lx * u;
    const ey = by - ly * u;
    const dBody2 = ex * ex + ey * ey;
    // Squared distances decide the region; a root is taken only where a distance is needed.
    const Ri = R - rimW;
    let depth = 0;
    if (dTop2 < Ri * Ri) {
      // Looking down the bore: its floor sits toward the vanishing point.
      const hx = px - (cx - lx * 0.9);
      const hy = py - (cy - ly * 0.9);
      const dh = Math.sqrt(hx * hx + hy * hy) / R;
      const lum = 0.05 + 0.82 * sstep(0.2, 1.25, dh);
      depth = 1 - lum;
      v = lum * this.c2;
    } else if (dTop2 < R * R) {
      v = this.c3;
    } else if (dBody2 < R * R) {
      v = (0.2 + 0.5 * u) * this.c2;
      depth = 0.8 - 0.8 * u;
    } else {
      // The halo, spent after a few decay lengths.
      const m2 = Math.min(dTop2, dBody2);
      // Past this the halo is under the floor anyway (sooner on a full stage, where it is dimmer): skip the exp.
      const far = R + g * (full ? 0.14 : 0.3);
      v = m2 > far * far ? 0 : 0.2 * Math.exp(-(Math.sqrt(m2) - R) / (g * 0.09)) * this.c2;
    }
    this.hue = this.c4 + depth * 0.16;
    return v;
  }

  private tiles(px: number, py: number): number {
    const B = this.B;
    const t = this.t;
    const sd = this.sd;
    const full = this.full;
    const fy = this.fy;
    let v: number;
    // Softly rounded frosted keys on a plane seen at a slight angle, lit from underneath by
    // two lights that glide about: faces glow, and the edges facing a light catch it.
    // Everything that depends only on the row is worked out once per row.
    if (py !== this.ry) {
      this.ry = py;
      const H = TILE_H + (this.vy - fy) * 2;
      this.rh = H;
      this.riz = (1 / (py - H)) * TILE_SU;
      const w = TILE_SV / (py - H);
      this.riv = Math.floor(w);
      // Rows run up the screen as w grows, so screen-down within a tile is -w.
      this.rlv = this.riv + 0.5 - w;
    }
    const H = this.rh;
    const iv = this.riv;
    const lv = this.rlv;
    const u = (px - this.vx) * this.riz;
    const iu = Math.floor(u);
    if (iu !== this.ci || iv !== this.cj) {
      this.ci = iu;
      this.cj = iv;
      // The tile's centre on screen.
      const cyy = TILE_SV / (iv + 0.5);
      const cy = cyy + H;
      const cx = this.vx + ((iu + 0.5) * cyy) / TILE_SU;
      const ax = this.l1x - cx;
      const ay = this.l1y - cy;
      const bx = this.l2x - cx;
      const by = this.l2y - cy;
      const a2 = ax * ax + ay * ay;
      const b2 = bx * bx + by * by;
      // A full stage spreads the lights wide, so every key is lit and the lights are broad swells.
      const ia = full ? Math.exp(-a2 / 0.3) : Math.exp(-a2 / 0.12);
      const ib = full ? 0.8 * Math.exp(-b2 / 0.24) : 0.65 * Math.exp(-b2 / 0.07);
      const n = sample(B, iu * 0.13 + sd, iv * 0.13 + t * 0.012);
      // Every key glows a little; the lights passing underneath make it bloom.
      this.c2 = ((full ? 0.42 : 0.32) + ia + ib) * (0.8 + 0.45 * n);
      // Direction toward the light, weighted by how much each light gives.
      const la = Math.sqrt(a2) || 1;
      const lb = Math.sqrt(b2) || 1;
      const mx = (ax / la) * ia + (bx / lb) * ib;
      const my = (ay / la) * ia + (by / lb) * ib;
      const ml = Math.sqrt(mx * mx + my * my) || 1;
      this.c5 = mx / ml;
      this.c6 = my / ml;
      this.c4 = iu * 0.05 + iv * 0.045 + t * 0.06 + n * 0.35;
    }
    const lu = u - iu - 0.5;
    const qx = Math.abs(lu) - (TILE_HALF - TILE_R);
    const qy = Math.abs(lv) - (TILE_HALF - TILE_R);
    const ox = Math.max(qx, 0);
    const oy = Math.max(qy, 0);
    const d = Math.sqrt(ox * ox + oy * oy) + Math.min(Math.max(qx, qy), 0) - TILE_R;
    const I = this.c2;
    if (d > 0) {
      // The gap: dark, with a thin halo where light leaks past the edge.
      v = d > (full ? 0.05 : 0.1) ? 0 : I * 0.22 * Math.exp(-d / 0.035);
    } else {
      // Frosted face, brighter on the side toward the light.
      const toward = lu * this.c5 + lv * this.c6;
      v = I * (0.44 + 0.34 * toward);
      if (d > -TILE_RIM) {
        // The edge, catching the light where it faces it.
        const facing = toward > 0 ? toward / (Math.sqrt(lu * lu + lv * lv) || 1) : 0;
        const e = 1 + d / TILE_RIM;
        v += I * e * e * (0.3 + facing);
      }
    }
    this.hue = this.c4;
    return v;
  }

  /** The broad fields: bloom, streak (the session's floor of light) and aurora. */
  private broad(px: number, py: number): number {
    const A = this.A;
    const B = this.B;
    const t = this.t;
    const sd = this.sd;
    const full = this.full;
    const mode = this.mode;
    const dx = px - this.fx;
    const dy = py - this.fy;
    const dist = Math.sqrt(dx * dx + dy * dy);
    let v: number;
    if (mode === "bloom") {
      const ang = Math.atan2(dy, dx);
      const wob = sample(A, dx * 0.42 + sd + t * 0.008, dy * 0.42 + t * 0.006);
      const petals = 0.5 + 0.5 * Math.cos(ang * 5 + t * 0.07 + wob * 5.2);
      const shape = 1 - sstep(0, 0.6, dist - 0.16 - 0.36 * petals);
      const core = 1 - sstep(0, 0.42, dist);
      const grain = sample(B, px * 0.6 - t * 0.01, py * 0.6 + sd);
      v = shape * (0.34 + 0.6 * grain) + core * 0.3;
    } else if (mode === "streak") {
      const qx = (px * STREAK_COS - py * STREAK_SIN) * 0.085 + sd;
      const qy = (px * STREAK_SIN + py * STREAK_COS) * 0.9;
      const n = sample(A, qx + t * 0.011, qy);
      const n2 = sample(B, qx * 0.5 - t * 0.007, qy * 2.3 + 0.31);
      // Over a floor: the horizon's edge stays one line, and the noise breaks its brightness, not its shape.
      v = 0.62 + 0.38 * sstep(0.3, 0.74, n * 0.62 + n2 * 0.46);
    } else {
      const qx = px * 0.3 + sd;
      const qy = py * 0.3;
      const w1 = sample(A, qx, qy + t * 0.008);
      const w2 = sample(B, qx + 0.52, qy - t * 0.006);
      const n = sample(A, qx + w1 * 0.42 + t * 0.004, qy + w2 * 0.42);
      // A full stage keeps a faint floor of light everywhere, so no part of it goes empty.
      v = full ? 0.24 + 0.62 * sstep(0.22, 0.76, n) : sstep(0.34, 0.72, n);
    }
    return v;
  }

  /**
   * The session's horizon (the streak's falloff): the edge of a wide circle that crests at the
   * middle of the band, where the composer's top edge is, and sinks to the stage's lower corners.
   * Brightest on the edge itself, a glow that thins out above it, a dim ground below it; all of it
   * strongest under the composer and fading towards the sides. The streak's noise is multiplied
   * into it, so the line is drawn in broken dither, not as a stroke.
   */
  private horizon(px: number, py: number): number {
    const a = this.aspect * 0.5;
    const r = (a * a + HORIZON_SAG * HORIZON_SAG) / (2 * HORIZON_SAG);
    const ex = px - a;
    const ey = py - HORIZON_APEX - r;
    // Above the edge: positive.
    const d = Math.sqrt(ex * ex + ey * ey) - r;
    const side = ex / a;
    const reach = 1 - 0.74 * side * side;
    if (reach <= 0) return 0;
    if (d >= 0) {
      const g = 1 - d / HORIZON_GLOW;
      return g <= 0 ? 0 : g * g * 0.8 * reach;
    }
    const k = -d / HORIZON_RIM;
    return (k >= 1 ? HORIZON_GROUND : 1 - (1 - HORIZON_GROUND) * k * k * (3 - 2 * k)) * reach;
  }

  /** Falloff (or, full bleed, the sweep, hollow and roll-off), the floor, the palette and the spectrum's hues. */
  private finish(v: number, px: number, py: number, out: Float32Array, spot = 1) {
    const lut = this.lut;
    const img = this.px != null || this.flat;
    // A crest or the pointer's light can light even an unlit cell of a full stage.
    const lit = this.full && ((this.fn !== 0 && this.wadd > 0) || this.lI > 0);
    if (v <= 0 && !lit) {
      // Unlit (the gaps between tubes and tiles): ground, with nothing more to work out.
      out[0] = lut[0]!;
      out[1] = lut[1]!;
      out[2] = lut[2]!;
      out[3] = 0;
      return;
    }
    const B = this.B;
    const t = this.t;
    const sd = this.sd;
    const full = this.full;
    const mode = this.mode;
    const hue = this.hue;
    const dx = px - this.fx;
    const dy = py - this.fy;
    const dist = full ? 0 : Math.sqrt(dx * dx + dy * dy);
    if (full) {
      // Even light edge to edge. A long, slow sweep crosses the stage (tubes carry their own wave),
      // and a soft, wide hollow behind the centre column keeps the headline and composer on calm ground.
      // An image keeps its own light: no sweep, a shallower hollow and a higher knee.
      if (mode !== "tubes" && !img) {
        const sweep = 0.5 + 0.5 * fcos((px * this.wc + py * this.ws) * 2.2 - t * 0.55);
        v *= mode === "streaks" ? 0.84 + 0.22 * sweep : 0.74 + 0.36 * sweep;
      }
      if (py !== this.hy) {
        this.hy = py;
        const ey = (py - HOLLOW_Y) / HOLLOW_RY;
        this.hq = ey * ey;
      }
      const ex = (px - this.aspect * 0.5) / HOLLOW_RX;
      const q = ex * ex + this.hq;
      // A spot already keeps light off most of the stage, so the hollow eases off with it.
      const hollow = this.hollowK * (img ? 0.6 : 1) * (this.spotOn ? 0.35 + 0.65 * this.spEven : 1);
      v *= (1 - hollow / (1 + q * q)) * (img ? this.gainNow * 1.15 : this.fullK);
      const knee = img ? 0.72 : KNEE;
      if (v > knee) {
        const x = v - knee;
        v = knee + x / (1 + x / ROLL);
      }
      if (v < 0) v = 0;
      // The pointer's light: a broad, faint lift that trails the pointer.
      if (this.lI > 0) {
        const ex2 = px - this.lx;
        const ey2 = py - this.ly;
        const q = 1 - (ex2 * ex2 + ey2 * ey2) / (LIGHT_R * LIGHT_R);
        if (q > 0) v += this.lI * q * q * (0.45 + v);
      }
      // A beat lifts what it passes, lit cells most (and may push past the knee for its moment), or dims it.
      if (this.fn !== 0) {
        const k = this.wadd * this.react;
        if (k !== 0) v += k * (0.18 + 0.6 * v);
        if (v < 0) v = 0;
        // Past the light stop the crest bends over, so it never goes white.
        else if (v > 0.68) v = 0.68 + (v - 0.68) / (1 + (v - 0.68) / 0.14);
        v *= this.wmul;
      }
    } else {
      // Light falls away from the focus, leaving a dark ground for text.
      // Tubes and tiles fade out completely with distance so they never sit behind the reading area.
      // A streak is the session's horizon: an arc of light that crests under the composer.
      const fall =
        mode === "tubes"
          ? 1 - sstep(0.12, TUBE_REACH, dist)
          : mode === "tiles"
            ? 1 - sstep(0.18, TILE_REACH, dist)
            : mode === "streak"
              ? this.horizon(px, py)
              : 0.1 + 0.9 * (1 - sstep(0.05, 1.3, dist));
      v *= fall * this.gainNow;
      // The band under the dock lifts as a whole while a front crosses it, most where the front is,
      // and the crest itself shows as a line of light travelling along the band.
      if (this.fn !== 0) {
        const k = this.react;
        const v0 = v;
        v =
          (v * (1 + k * (0.35 * this.lift + 1.1 * this.wadd)) + k * 0.6 * this.wadd * fall * this.gainNow) *
          this.wmul;
        if (v < 0) v = 0;
        // The crest bends over before the palette's pale end, so it reads as light, not as grey.
        else if (v > v0 && v > 0.6) v = Math.max(v0, 0.6 + (v - 0.6) / (1 + (v - 0.6) / 0.12));
      }
    }
    // The spot's falloff, before the floor and quantization, so its edge dithers away like the rest.
    v *= spot;
    // A floor, so the dark side is clean ground rather than a scatter of stray cells.
    v = v < 0.09 ? 0 : (v - 0.09) * 1.099;
    if (this.px && this.imgColour) {
      // The image's own colour at this point, scaled by what happened to its brightness.
      const k = v <= 0 ? 0 : Math.min(1.6, v / Math.max(this.ilum, 0.03));
      out[0] = lut[0]! + (this.ir - lut[0]!) * k;
      out[1] = lut[1]! + (this.ig - lut[1]!) * k;
      out[2] = lut[2]! + (this.ib - lut[2]!) * k;
      out[3] = v;
      return;
    }
    const vi = v <= 0 ? 0 : v >= 1 ? 765 : ((v * 255) | 0) * 3;
    let r = lut[vi]!;
    let g = lut[vi + 1]!;
    let b = lut[vi + 2]!;

    const spectrum = img ? 0 : this.spectrum;
    if (spectrum > 0.01 && v > 0) {
      const hueAt =
        mode === "tubes" || mode === "tiles" || mode === "streaks"
          ? hue
          : sample(B, px * 0.21 + sd + t * 0.003, py * 0.21) * 1.7 + px * 0.22 - py * 0.2 + t * 0.008;
      const mix = sstep(0.1, 0.6, v) * (full ? 0.72 : 0.9) * spectrum;
      const lum = 0.14 + v * 1.05;
      const hi = (((hueAt - Math.floor(hueAt)) * HUE_N) | 0) * 3;
      r += (HUES[hi]! * lum - r) * mix;
      g += (HUES[hi + 1]! * lum - g) * mix;
      b += (HUES[hi + 2]! * lum - b) * mix;
    }
    out[0] = r!;
    out[1] = g!;
    out[2] = b!;
    out[3] = v;
  }
}
