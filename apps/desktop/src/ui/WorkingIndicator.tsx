import { type CSSProperties, useEffect, useRef } from "react";
import { cyclePatterns, GRID, PATTERNS, type PatternName } from "../lib/workingMotion";
import { cx } from "./primitives";

const STEP_MS = 90;
const REST_LEVEL = 0.2;

/**
 * A small grid of dots that moves while a turn works: the accent for the trail, the text colour for
 * its head. With `cycle` it starts on `pattern` and moves through the whole set.
 */
export function DotMatrix({
  pattern,
  size = 16,
  cycle = false,
  className,
}: {
  pattern: PatternName;
  size?: number;
  cycle?: boolean;
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
      const trail = style.color;
      const head = style.getPropertyValue("--rc-text").trim() || trail;
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
    let step = 0;
    draw(step);
    const timer = setInterval(() => draw(++step), STEP_MS);
    return () => clearInterval(timer);
  }, [pattern, size, cycle]);
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
  return (
    <span key={word} className={cx("rc-wave shrink-0 rc-in", className)}>
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
  return (
    <span className={cx("rc-wave shrink-0 whitespace-pre", className)}>
      <span className="sr-only">{text}</span>
      {Array.from(text, (letter, i) => (
        <span key={i} aria-hidden style={{ "--rc-wave-i": i } as CSSProperties}>
          {letter}
        </span>
      ))}
    </span>
  );
}
