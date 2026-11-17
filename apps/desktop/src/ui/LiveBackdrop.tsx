// The live background (Settings › Appearance › Live background) for one surface, after Sable's
// art/Backdrop.tsx (MIT, Copyright (c) 2026 Leonxlnx). It maps the setting onto a DitherField;
// changes reach the running field as messages. Nothing is mounted (no canvas, no worker) when the
// source is off or the strength is 0.
//
// - new: the whole new-thread pane, the light gathered where the setting puts it.
// - thread: the band of light behind an open thread's composer, breathing with the turn.
// - swatch: a preset's tiny still frame in Settings.

import { useEffect, useRef, useState } from "react";
import type { ThreadStatus } from "../acp/v2";
import {
  DIRECTIONAL,
  type LiveBackdrop as LiveConfig,
  liveCell,
  useLiveBackdrop,
} from "../lib/backdrop/config";
import { cssVar, onAppThemeChange } from "../lib/theme";
import { DitherField, type DitherPalette } from "./DitherField";

type RGB = [number, number, number];

/** A harness's light, by the same names its mark is picked by (ui/HarnessIcon.tsx). */
function harnessPalette(harness: string | undefined): DitherPalette | null {
  const key = (harness ?? "").toLowerCase();
  if (key.includes("claude")) return "ember";
  if (key.includes("gemini")) return "spectrum";
  if (key.includes("opencode")) return "mono";
  if (key.includes("codex") || key.includes("openai")) return "iris";
  // RustCode and every other harness: the theme's accent.
  return null;
}

/** `#rgb`, `#rrggbb` or `#rrggbbaa` to channels; null for anything else. */
export function parseHex(value: string): RGB | null {
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6}|[0-9a-f]{8})$/i.exec(value.trim());
  if (!m?.[1]) return null;
  const h = m[1].length === 3 ? [...m[1]].map((c) => c + c).join("") : m[1].slice(0, 6);
  return [
    Number.parseInt(h.slice(0, 2), 16),
    Number.parseInt(h.slice(2, 4), 16),
    Number.parseInt(h.slice(4, 6), 16),
  ];
}

/** Hue (0–360) and saturation (0–1) of a colour. */
export function hueOf([r, g, b]: RGB): { hue: number; sat: number } {
  const [R, G, B] = [r / 255, g / 255, b / 255];
  const max = Math.max(R, G, B);
  const min = Math.min(R, G, B);
  const d = max - min;
  const l = (max + min) / 2;
  if (d === 0) return { hue: 0, sat: 0 };
  const sat = d / (1 - Math.abs(2 * l - 1));
  const hue = max === R ? ((G - B) / d) % 6 : max === G ? (B - R) / d + 2 : (R - G) / d + 4;
  return { hue: Math.round((hue * 60 + 360) % 360), sat: Math.min(1, sat) };
}

/** The pane colour and the accent, read again when the theme changes. */
function useThemeColours() {
  const read = () => ({ ground: parseHex(cssVar("--rc-bg")), accent: parseHex(cssVar("--rc-accent")) });
  const [colours, setColours] = useState(read);
  // biome-ignore lint/correctness/useExhaustiveDependencies: `read` only reads the document
  useEffect(() => onAppThemeChange(() => setColours(read())), []);
  return colours;
}

/** True while `on`, and for `ms` after it turns off: the band holds its light while a turn's end plays. */
function useLinger(on: boolean, ms: number) {
  const [held, setHeld] = useState(on);
  const was = useRef(on);
  useEffect(() => {
    if (on) {
      was.current = true;
      setHeld(true);
      return;
    }
    if (!was.current) return;
    was.current = false;
    const t = window.setTimeout(() => setHeld(false), ms);
    return () => window.clearTimeout(t);
  }, [on, ms]);
  return on || held;
}

/** The strength at which a surface looks as Sable first drew it. */
const REFERENCE_AMOUNT = 60;
const THREAD_REFERENCE = 30;

/**
 * Strength to the field's gain (input light) and opacity (output). Low amounts both fade and thin
 * out, so a subtle setting stays a sparse dither over the ground instead of a faint wash of colour.
 */
function strength(a: number) {
  const k = a / REFERENCE_AMOUNT;
  return {
    gain: a <= REFERENCE_AMOUNT ? 0.45 + 0.55 * k : 1 + ((a - REFERENCE_AMOUNT) / 40) * 0.6,
    opacity: Math.min(1, 0.15 + 0.85 * k),
  };
}

interface Props {
  surface: "new" | "thread" | "swatch";
  /** The thread's harness id (or the one a new thread will run on): whose light it is. */
  harness?: string;
  /** Thread: what the turn is doing. */
  status?: ThreadStatus;
  /** Any thread is working: a new thread's light drifts a little quicker and does not rest. */
  busy?: boolean;
  /** A dialog covers it: the field holds still underneath. */
  covered?: boolean;
  /** Draw this instead of the stored setting (Settings' swatches). */
  config?: LiveConfig;
}

export function LiveBackdrop({
  surface,
  harness,
  status = "idle",
  busy = false,
  covered = false,
  config,
}: Props) {
  const stored = useLiveBackdrop();
  const cfg = config ?? stored;
  const { ground, accent } = useThemeColours();
  const working = status === "running";
  const lit = useLinger(working, 620);
  const thread = surface === "thread";
  const swatch = surface === "swatch";
  const amount = thread
    ? cfg.threadAmount
    : swatch
      ? Math.min(100, Math.max(cfg.amount * 1.5, cfg.amount + 12))
      : cfg.amount;
  if (cfg.source === "off" || amount <= 0) return null;

  const own = harnessPalette(harness);
  const accentHue = accent ? hueOf(accent) : null;
  const custom = cfg.colour === "custom" || (cfg.colour === "harness" && !own && !!accentHue);
  const hue = cfg.colour === "custom" ? cfg.hue : (accentHue?.hue ?? cfg.hue);
  const sat = cfg.colour === "custom" ? cfg.saturation / 100 : Math.max(0.55, accentHue?.sat ?? 0.7);
  const palette: DitherPalette = cfg.colour === "mono" ? "mono" : custom ? "custom" : (own ?? "iris");
  // A swatch is a miniature: cells shrink with it so it reads as the whole pane.
  const cell = swatch ? Math.max(2, Math.round(liveCell(cfg.detail) * 0.5)) : liveCell(cfg.detail);
  const common = {
    texture: cfg.style,
    cell,
    react: cfg.react && !swatch,
    pointer: cfg.react && !swatch,
    hue: custom ? hue : undefined,
    sat: custom ? sat : undefined,
    ground: ground ?? undefined,
    covered,
  } as const;

  if (thread) {
    // Working: the band drifts quicker. Waiting on you: it turns amber and stills, so the
    // stillness itself says "your turn". Just finished: still lit while the turn's end plays.
    const awaiting = status === "needs_input";
    const glowing = working || (lit && !awaiting);
    const k = amount / THREAD_REFERENCE;
    const base = glowing ? 0.9 : awaiting ? 0.82 : 0.72;
    const rest = Math.min(0.7, 0.3 + 0.2 * k);
    const bright = Math.min(1, 0.52 + 0.3 * k);
    return (
      <DitherField
        {...common}
        variant="streak"
        palette={awaiting ? "amber" : palette}
        focus={[0.5, 1.15]}
        gain={base * (k <= 1 ? 1 : 1 + ((amount - THREAD_REFERENCE) / 70) * 0.6)}
        speed={(working ? 2.2 : awaiting ? 0 : glowing ? 1.1 : 0.35) * (cfg.speed / 30)}
        still={cfg.speed === 0}
        opacity={glowing || awaiting ? bright : rest}
        reactGain={0.85}
        awake={working}
      />
    );
  }

  const { gain, opacity } = strength(amount);
  return (
    <DitherField
      {...common}
      still={swatch || cfg.speed === 0}
      variant={cfg.pattern}
      palette={palette}
      full
      gain={gain}
      opacity={opacity}
      // A dim pane lifts its beats, so a sent message still reads at a quiet strength.
      reactGain={Math.min(2.4, Math.max(1, 0.62 / opacity))}
      awake={busy}
      speed={(cfg.speed / 50) * (busy ? 1.45 : 1)}
      spot={[cfg.focus.x, cfg.focus.y, cfg.reach / 100]}
      angle={DIRECTIONAL.includes(cfg.pattern) ? cfg.angle : undefined}
    />
  );
}
