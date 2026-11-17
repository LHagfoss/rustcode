// The live background's settings (Settings › Appearance › Live background), after Sable's
// shared/backdrop.ts (MIT, Copyright (c) 2026 Leonxlnx): the default, the presets, and
// `normalizeLiveBackdrop`, which every stored value passes through. Kept in local storage like the
// chat background picture (lib/banner.ts).

import { useSyncExternalStore } from "react";
import { readStorage, writeStorage } from "../storage";

export type LiveSource = "pattern" | "off";
export type LivePattern = "streaks" | "tubes" | "tiles" | "aurora";
export type LiveStyle = "dither" | "ascii" | "halftone" | "soft";
/** Whose colour: the thread's harness, grey, or a hue of the user's. */
export type LiveColour = "harness" | "mono" | "custom";

export interface LiveBackdrop {
  source: LiveSource;
  pattern: LivePattern;
  style: LiveStyle;
  /** Strength behind a new thread, 0–100 (0 draws nothing). */
  amount: number;
  /** Strength of the band behind an open thread's composer, 0–100 (0 draws nothing). */
  threadAmount: number;
  /** Where the light gathers, 0–1 across and down the pane. */
  focus: { x: number; y: number };
  /** How far the light spreads from the focus, 0–100; 100 is the whole pane evenly. */
  reach: number;
  /** Cell size, 0 coarse to 100 fine. */
  detail: number;
  /** 0 is completely still. */
  speed: number;
  /** Direction of streaks and of the tubes' wave, degrees 0–180. */
  angle: number;
  colour: LiveColour;
  /** Custom colour: hue 0–360 and saturation 0–100. */
  hue: number;
  saturation: number;
  /** The light answers the pointer and plays a beat when a message is sent or a turn ends. */
  react: boolean;
}

export const LIVE_PATTERNS: readonly LivePattern[] = ["streaks", "tubes", "tiles", "aurora"];
export const LIVE_STYLES: readonly LiveStyle[] = ["dither", "ascii", "halftone", "soft"];
const SOURCES: readonly LiveSource[] = ["pattern", "off"];
const COLOURS: readonly LiveColour[] = ["harness", "mono", "custom"];

/** Patterns with a direction (`angle` applies). */
export const DIRECTIONAL: readonly LivePattern[] = ["streaks", "tubes"];

/**
 * Calm ink with a hint of light: a low glow rising from the lower edge, fine cells, slow drift,
 * most of the pane plain ground. The presets are the louder steps.
 */
export const LIVE_BACKDROP_DEFAULT: LiveBackdrop = {
  source: "pattern",
  pattern: "streaks",
  style: "dither",
  amount: 8,
  threadAmount: 16,
  focus: { x: 0.62, y: 1 },
  reach: 72,
  detail: 94,
  speed: 30,
  angle: 35,
  colour: "harness",
  hue: 28,
  saturation: 70,
  react: true,
};

export interface LivePreset {
  id: string;
  name: string;
  config: LiveBackdrop;
}

const preset = (id: string, name: string, patch: Partial<LiveBackdrop>): LivePreset => ({
  id,
  name,
  config: { ...LIVE_BACKDROP_DEFAULT, ...patch },
});

/** The whole pane evenly (the default keeps its light low instead). */
const FULL = { reach: 100, focus: { x: 0.72, y: 0.78 } } as const;

export const LIVE_PRESETS: readonly LivePreset[] = [
  preset("quiet", "Quiet", { amount: 4, threadAmount: 8, reach: 60, speed: 20 }),
  preset("default", "Glow", {}),
  preset("streaks", "Streaks", { ...FULL, amount: 22, detail: 86, speed: 40, threadAmount: 26 }),
  preset("corner", "Corner", { pattern: "aurora", amount: 50, reach: 46, focus: { x: 1, y: 1 }, speed: 30 }),
  preset("ascii", "ASCII", { ...FULL, style: "ascii", amount: 36, detail: 76 }),
  preset("halftone", "Halftone", {
    style: "halftone",
    pattern: "aurora",
    amount: 44,
    reach: 64,
    focus: { x: 0.5, y: 0 },
    detail: 76,
  }),
  preset("still", "Still", { pattern: "tiles", amount: 34, speed: 0, reach: 72, focus: { x: 0.2, y: 1 } }),
  preset("mono", "Mono", { ...FULL, colour: "mono", pattern: "tubes", amount: 30 }),
  preset("vivid", "Vivid", { ...FULL, amount: 60, detail: 82, speed: 50, threadAmount: 60 }),
];

const clamp = (v: unknown, lo: number, hi: number, fallback: number) => {
  const n = typeof v === "number" ? v : typeof v === "string" && v.trim() !== "" ? Number(v) : Number.NaN;
  if (!Number.isFinite(n)) return fallback;
  return Math.min(hi, Math.max(lo, n));
};

const pick = <T extends string>(v: unknown, allowed: readonly T[], fallback: T): T =>
  allowed.includes(v as T) ? (v as T) : fallback;

/** A complete, valid config from anything: numbers clamped and rounded, unknown values the default. */
export function normalizeLiveBackdrop(input: unknown): LiveBackdrop {
  const o = input && typeof input === "object" ? (input as Record<string, unknown>) : {};
  const d = LIVE_BACKDROP_DEFAULT;
  const focus = o.focus && typeof o.focus === "object" ? (o.focus as Record<string, unknown>) : {};
  const round = (v: unknown, lo: number, hi: number, f: number) => Math.round(clamp(v, lo, hi, f));
  const unit = (v: unknown, f: number) => Math.round(clamp(v, 0, 1, f) * 1000) / 1000;
  return {
    source: pick(o.source, SOURCES, d.source),
    pattern: pick(o.pattern, LIVE_PATTERNS, d.pattern),
    style: pick(o.style, LIVE_STYLES, d.style),
    amount: round(o.amount, 0, 100, d.amount),
    threadAmount: round(o.threadAmount, 0, 100, d.threadAmount),
    focus: { x: unit(focus.x, d.focus.x), y: unit(focus.y, d.focus.y) },
    reach: round(o.reach, 0, 100, d.reach),
    detail: round(o.detail, 0, 100, d.detail),
    speed: round(o.speed, 0, 100, d.speed),
    angle: round(o.angle, 0, 180, d.angle),
    colour: pick(o.colour, COLOURS, d.colour),
    hue: round(o.hue, 0, 360, d.hue),
    saturation: round(o.saturation, 0, 100, d.saturation),
    react: typeof o.react === "boolean" ? o.react : d.react,
  };
}

/** Whether two configs draw the same thing. */
export function sameLiveBackdrop(a: LiveBackdrop, b: LiveBackdrop): boolean {
  for (const k of Object.keys(a) as (keyof LiveBackdrop)[]) {
    if (k === "focus") {
      if (a.focus.x !== b.focus.x || a.focus.y !== b.focus.y) return false;
    } else if (a[k] !== b[k]) return false;
  }
  return true;
}

/** Cell size in CSS px for the dither texture (detail 100 = 3px, 0 = 14px). */
export const liveCell = (detail: number) => Math.round(3 + ((100 - detail) / 100) * 11);

// ── the stored setting ──

const KEY = "rustcode.liveBackground";

function load(): LiveBackdrop {
  const raw = readStorage(KEY);
  if (!raw) return LIVE_BACKDROP_DEFAULT;
  try {
    return normalizeLiveBackdrop(JSON.parse(raw));
  } catch {
    return LIVE_BACKDROP_DEFAULT;
  }
}

const listeners = new Set<() => void>();
let snapshot = load();

/** Replaces the setting (normalized); the default is stored as nothing. */
export function setLiveBackdrop(next: LiveBackdrop | ((current: LiveBackdrop) => LiveBackdrop)) {
  const value = normalizeLiveBackdrop(typeof next === "function" ? next(snapshot) : next);
  writeStorage(KEY, sameLiveBackdrop(value, LIVE_BACKDROP_DEFAULT) ? null : JSON.stringify(value));
  snapshot = value;
  for (const l of listeners) l();
}

/** Changes some fields of the setting. */
export function patchLiveBackdrop(patch: Partial<LiveBackdrop>) {
  setLiveBackdrop((c) => ({ ...c, ...patch }));
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useLiveBackdrop(): LiveBackdrop {
  return useSyncExternalStore(subscribe, () => snapshot);
}
