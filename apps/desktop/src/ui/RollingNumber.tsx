// A count that rolls when it changes (docs/design.md, Motion): each digit slides vertically
// to its new value, the columns keep their place counted from the right, so 9 → 10 rolls the
// units and grows a tens. Screen readers get the plain number once; the rolling copy is hidden
// from them. Used by the sidebar's counts and the header's plan badge, never for text.

import { useLayoutEffect, useRef, useState } from "react";
import { cx } from "./primitives";

/** How long a digit takes to slide. */
export const ROLL_MS = 220;

export interface DigitColumn {
  /** Stable from the right: the units column keeps its key as the number grows. */
  key: string;
  digit: string;
}

export interface DigitRoll extends DigitColumn {
  /** The digit this column showed before, when it changed. */
  from?: string;
  /** Which way the old digit leaves: up for a higher value, down for a lower one. */
  direction: "up" | "down";
}

/**
 * The digits of a count, left to right, each keyed by its place from the right. The key is
 * what lets a new leading digit join without renumbering the columns below it.
 */
export function digitColumns(value: number): DigitColumn[] {
  const text = Math.trunc(Math.abs(value)).toString();
  return [...text].map((digit, i) => ({ key: `d${text.length - 1 - i}`, digit }));
}

/** How each column of `next` differs from `previous`: its old digit, and which way it rolls. */
export function rollDigits(previous: number, next: number): DigitRoll[] {
  const before = new Map(digitColumns(previous).map((column) => [column.key, column.digit]));
  // The whole count decides the way the column slides, not each digit: an odometer going 9 → 10
  // rolls its units up to 0 and brings a 1 in above it, like every other upward step.
  const direction = next < previous ? "down" : "up";
  return digitColumns(next).map((column) => {
    const from = before.get(column.key);
    if (from === undefined || from === column.digit) return { ...column, direction };
    return { ...column, from, direction };
  });
}

function RollingDigit({ digit, roll }: { digit: string; roll?: DigitRoll }) {
  if (!roll?.from) {
    return <span className="inline-block h-[1em] w-[1ch] text-center leading-[1em]">{digit}</span>;
  }
  const down = roll.direction === "down";
  return (
    <span className="inline-block h-[1em] w-[1ch] overflow-hidden text-center align-baseline leading-[1em]">
      <span
        // A new pair remounts the strip, so a second change starts a fresh roll.
        key={`${roll.from}-${digit}`}
        className={cx("rc-digit flex flex-col", down && "rc-digit-down")}
        aria-hidden
      >
        <span className="block h-[1em] leading-[1em]">{down ? digit : roll.from}</span>
        <span className="block h-[1em] leading-[1em]">{down ? roll.from : digit}</span>
      </span>
    </span>
  );
}

/** A count as rolling digits. `className` sizes and colours it; the digits are always tabular. */
export function RollingNumber({ value, className }: { value: number; className?: string }) {
  const previous = useRef(value);
  // The rolls still running, by column: kept until the slide is over, so an unrelated
  // re-render does not cut the animation short and snap the digit into place.
  const [rolls, setRolls] = useState<Record<string, DigitRoll>>({});
  useLayoutEffect(() => {
    if (previous.current === value) return;
    const next = rollDigits(previous.current, value);
    previous.current = value;
    const changed = Object.fromEntries(next.filter((column) => column.from).map((c) => [c.key, c]));
    if (!Object.keys(changed).length) return;
    setRolls(changed);
    const timer = setTimeout(() => setRolls({}), ROLL_MS);
    return () => clearTimeout(timer);
  }, [value]);
  return (
    <span className={cx("inline-flex items-baseline tabular", className)}>
      <span className="sr-only">{value}</span>
      <span aria-hidden className="inline-flex">
        {digitColumns(value).map((column) => (
          <RollingDigit key={column.key} digit={column.digit} roll={rolls[column.key]} />
        ))}
      </span>
    </span>
  );
}
