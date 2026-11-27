import { type CSSProperties, useEffect, useRef, useState } from "react";
import { cyclePatterns, GRID, PATTERNS, type PatternName } from "../lib/workingMotion";
import { cx } from "./primitives";

const STEP_MS = 90;
const REST_LEVEL = 0.2;
const HUE_STEPS = 400;
const WAVE_CLOCK_MS = 7200;

/** The step every grid on screen is at now: a row that mounts mid-turn joins the motion where it is. */
const clockStep = () => Math.floor(Date.now() / STEP_MS);

/** How far into the shared wave cycle a mounting word starts, so a new row never restarts the wave. */
function useWavePhase(): CSSProperties {
  const [phase] = useState(() => Date.now() % WAVE_CLOCK_MS);
  return { "--rc-wave-phase": `${phase}ms` } as CSSProperties;
}

/**
 * The grid's colour: `accent` for the theme accent, `mono` for the text colour, `spectrum` for the
 * accent with its hue turning round the colour wheel while the turn works.
 */
export type DotTone = "accent" | "mono" | "spectrum";

const hueTurn = (step: number) => `hue-rotate(${Math.round(((step % HUE_STEPS) / HUE_STEPS) * 360)}deg)`;

/**
 * A small grid of dots that moves while a turn works: the accent for the trail, the text colour for
 * its head. With `cycle` it moves through the whole set on a clock every grid shares.
 */
export function DotMatrix({
  pattern,
  size = 16,
  cycle = false,
  tone = "accent",
  className,
}: {
  pattern: PatternName;
  size?: number;
  cycle?: boolean;
  tone?: DotTone;
  className?: string;
}) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const canvas = ref.current;
    const g = canvas?.getContext("2d");
    if (!canvas || !g) return;
    const ratio = window.devicePixelRatio || 1;
    canvas.width = Math.round(size * ratio);
    canvas.height = Math.round(size * ratio);
    const pitch = canvas.width / GRID;
    const radius = pitch * 0.38;
    const frames = cycle ? cyclePatterns(pattern) : PATTERNS[pattern];
    const draw = (step: number) => {
      const style = getComputedStyle(canvas);
      const head = style.getPropertyValue("--rc-text").trim() || style.color;
      const trail = tone === "mono" ? head : style.color;
      if (tone === "spectrum") canvas.style.filter = hueTurn(step);
      const levels = new Float32Array(GRID * GRID);
      const heads = new Set<number>();
      for (const lit of frames(step)) {
        levels[lit.cell] = Math.max(levels[lit.cell]!, lit.level);
        if (lit.head) heads.add(lit.cell);
      }
      g.clearRect(0, 0, canvas.width, canvas.height);
      for (let cell = 0; cell < GRID * GRID; cell++) {
        const x = (cell % GRID) * pitch + pitch / 2;
        const y = Math.floor(cell / GRID) * pitch + pitch / 2;
        const level = levels[cell]!;
        g.globalAlpha = REST_LEVEL + (1 - REST_LEVEL) * level;
        g.fillStyle = heads.has(cell) ? head : trail;
        g.beginPath();
        g.arc(x, y, radius, 0, Math.PI * 2);
        g.fill();
      }
    };
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) {
      draw(4);
      return;
    }
    draw(clockStep());
    const timer = setInterval(() => draw(clockStep()), STEP_MS);
    return () => clearInterval(timer);
  }, [pattern, size, cycle, tone]);
  return (
    <canvas
      ref={ref}
      aria-hidden
      className={cx("shrink-0 text-accent", className)}
      style={{ width: size, height: size }}
    />
  );
}

/** A word whose letters brighten one after another, a wave that keeps running while the turn works. */
export function WaveWord({ word, className }: { word: string; className?: string }) {
  const phase = useWavePhase();
  return (
    <span className={cx("rc-wave shrink-0", className)} style={phase}>
      <span className="sr-only">{word}</span>
      {Array.from(word, (letter, i) => (
        <span key={i} aria-hidden style={{ "--rc-wave-i": i } as CSSProperties}>
          {letter}
        </span>
      ))}
    </span>
  );
}

/**
 * A line of text the wave runs across from end to end. Letters keep their place when the text
 * changes (a clock ticking), so the wave carries on instead of starting over.
 */
export function WaveText({ text, className }: { text: string; className?: string }) {
  const phase = useWavePhase();
  return (
    <span className={cx("rc-wave shrink-0 whitespace-pre", className)} style={phase}>
      <span className="sr-only">{text}</span>
      {Array.from(text, (letter, i) => (
        <span key={i} aria-hidden style={{ "--rc-wave-i": i } as CSSProperties}>
          {letter}
        </span>
      ))}
    </span>
  );
}
