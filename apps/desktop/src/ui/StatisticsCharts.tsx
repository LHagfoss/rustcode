// What Settings › Statistics draws: a tile with a change and a sparkline, the
// daily chart whose tooltip follows the pointer, the cost share bar with its
// legend, and the glide a reordered table uses. The numbers are handed in, so
// this file only draws them (the reads and the arithmetic live in
// ui/StatisticsPage.tsx and state/period.ts).
//
// Motion follows docs/design.md: digits roll once when a value changes, rows
// glide to their new place (transform only), and the global reduced-motion rule
// collapses both.

import { ArrowDown, ArrowUp } from "lucide-react";
import { type KeyboardEvent, type ReactNode, useLayoutEffect, useRef, useState } from "react";
import { count } from "../lib/plural";
import { type DayPoint, sparklinePath, type Trend } from "../state/period";
import { cx } from "./primitives";
import { RollingNumber } from "./RollingNumber";
import { Choice } from "./settingsKit";

/**
 * A value that is not a count — a cost, a compact token total — rolling up
 * once when the period changes. A whole count rolls digit by digit instead
 * (`RollingNumber`, the same one the sidebar's badges use).
 */
export function RollingText({ children, title }: { children: string; title?: string }) {
  return (
    <span key={children} title={title} className="rc-roll inline-block tabular">
      {children}
    </span>
  );
}

/** How a tile moved against the period before: an arrow pill, never a coloured judgement. */
export function TrendPill({ trend, compare }: { trend: Trend; compare: string }) {
  if (trend.direction === "level") {
    return (
      <span title={compare} className="shrink-0 text-caption text-fg-faint">
        no change
      </span>
    );
  }
  const up = trend.direction === "up";
  const Icon = up ? ArrowUp : ArrowDown;
  return (
    <span
      title={compare}
      className="inline-flex h-5 shrink-0 items-center gap-0.5 rounded-full bg-fill px-1.5 font-medium text-caption text-fg-muted tabular"
    >
      <Icon size={10} strokeWidth={2.5} aria-label={up ? "up" : "down"} />
      {trend.percent === undefined ? "new" : `${Math.abs(Math.round(trend.percent))}%`}
    </span>
  );
}

/**
 * One measure for the period: its name and change, the value (which rolls when
 * the period changes), one line of what it is made of, and the days behind it.
 */
export function Tile({
  label,
  value,
  count,
  exact,
  note,
  trend,
  compare,
  series,
  seriesLabel,
}: {
  label: string;
  value: string;
  /** The number behind a count's value, so it can roll digit by digit; absent for a cost or a compact total. */
  count?: number;
  /** The exact number behind a compact value, on hover. */
  exact?: string;
  note?: string;
  /** Absent when there is no period before ("All"), or a side is unknown. */
  trend?: Trend;
  compare: string;
  /** One value per day, `undefined` for a day nothing was reported. */
  series: (number | undefined)[];
  seriesLabel: string;
}) {
  // One or two reported days would draw a lone dash: with fewer than three,
  // the whole period is drawn and its empty days sit on the zero baseline, the
  // way the Turns tile always reads.
  const known = series.filter((value) => value !== undefined).length;
  const drawn = known >= 3 ? series : series.map((value) => value ?? 0);
  // One day is a point, not a line: the tile keeps its shape but draws nothing.
  const path = series.length > 1 ? sparklinePath(drawn, 100, 24) : "";
  return (
    <div className="flex min-w-0 flex-col rounded-xl border border-border-strong bg-surface-2/40 px-4 py-3.5">
      <div className="flex min-h-5 items-center gap-2">
        <span className="shrink-0 text-caption text-fg-faint">{label}</span>
        <span className="flex min-w-0 flex-1 justify-end">
          {trend ? (
            <TrendPill trend={trend} compare={compare} />
          ) : (
            <span className="truncate text-caption text-fg-faint" title={compare}>
              {compare}
            </span>
          )}
        </span>
      </div>
      <div className="pt-0.5 font-semibold text-title2 text-fg" title={exact}>
        {count === undefined ? (
          <RollingText>{value}</RollingText>
        ) : (
          <RollingNumber value={count} className="font-semibold text-title2 text-fg" />
        )}
      </div>
      <div className="line-clamp-2 min-h-8 text-balance text-caption text-fg-faint leading-4" title={note}>
        {note ?? "\u00a0"}
      </div>
      <svg
        viewBox="0 0 100 24"
        preserveAspectRatio="none"
        aria-hidden
        className="mt-2 h-6 w-full text-accent/55"
      >
        <title>{seriesLabel}</title>
        {path && (
          <path
            d={path}
            fill="none"
            stroke="currentColor"
            strokeWidth={1.5}
            strokeLinecap="round"
            strokeLinejoin="round"
            vectorEffect="non-scaling-stroke"
          />
        )}
      </svg>
    </div>
  );
}

// ---------------------------------------------------------------- the daily chart

export type DailyMetric = "turns" | "tokens" | "cost";

/** What the daily chart can plot: all three are reported, never estimated. */
export const DAILY_METRICS: { value: DailyMetric; label: string }[] = [
  { value: "turns", label: "Turns" },
  { value: "tokens", label: "Tokens" },
  { value: "cost", label: "Cost" },
];

const shortDay = (day: string): string =>
  new Date(`${day}T00:00:00Z`).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    timeZone: "UTC",
  });

/**
 * One bar per day, one measure at a time. The tooltip follows the pointer and
 * the bar under it lights; the plot is one tab stop, and ↑ ↓ ← → walk the days
 * so the same numbers are reachable from the keyboard.
 */
export function DailyChart({
  points,
  metric,
  onMetric,
  format,
}: {
  points: DayPoint[];
  metric: DailyMetric;
  onMetric: (metric: DailyMetric) => void;
  /** A measure's exact text ("1,204", "12.4k", "$1.20"). */
  format: (value: number | undefined) => string;
}) {
  const plot = useRef<HTMLDivElement>(null);
  const [active, setActive] = useState<{ index: number; x: number } | null>(null);
  const value = (point: DayPoint): number | undefined =>
    metric === "turns" ? point.turns : metric === "tokens" ? point.tokens : point.cost;
  const values = points.map(value);
  const peak = Math.max(0, ...values.map((one) => one ?? 0));
  const shown = active ? points[active.index] : undefined;

  const pick = (index: number, x?: number) => {
    const box = plot.current?.getBoundingClientRect();
    const width = box?.width ?? 0;
    const centre = width ? ((index + 0.5) / points.length) * width : 0;
    // Keep the tip inside the card, so a day at either end still reads.
    setActive({ index, x: Math.min(Math.max(x ?? centre, 48), Math.max(width - 48, 48)) });
  };

  const onKey = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!points.length) return;
    const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
    if (step === undefined && event.key !== "Home" && event.key !== "End" && event.key !== "Escape") return;
    event.preventDefault();
    if (event.key === "Escape") {
      setActive(null);
      return;
    }
    const from = active?.index ?? (step && step > 0 ? -1 : points.length);
    const next =
      event.key === "Home"
        ? 0
        : event.key === "End"
          ? points.length - 1
          : Math.min(Math.max(from + (step ?? 0), 0), points.length - 1);
    pick(next);
  };

  return (
    <div className="flex flex-col gap-3 px-4 py-4">
      <div className="flex min-h-8 flex-wrap items-center gap-2">
        <span className="min-w-0 flex-1 text-callout text-fg-muted">
          {shown ? (
            <span className="tabular">
              <span className="font-medium text-fg">{shortDay(shown.day)}</span> ·{" "}
              {metric === "cost" ? "reported cost" : metric === "tokens" ? "input and output" : "turns"}{" "}
              {values[active!.index] === undefined ? "unknown" : format(values[active!.index])}
            </span>
          ) : (
            <span>
              {count(points.length, "day")} · peak {format(peak || undefined)}
            </span>
          )}
        </span>
        <Choice<DailyMetric>
          label="Daily measure"
          value={metric}
          options={DAILY_METRICS}
          onChange={onMetric}
        />
      </div>
      <div
        ref={plot}
        role="group"
        // biome-ignore lint/a11y/noNoninteractiveTabindex: one tab stop for the whole plot, walked with the arrow keys
        tabIndex={0}
        aria-label={`Daily ${metric}, ${points.length} days; use the arrow keys for a day`}
        onKeyDown={onKey}
        onPointerMove={(event) => {
          if (!points.length) return;
          const box = event.currentTarget.getBoundingClientRect();
          const share = (event.clientX - box.left) / box.width;
          const index = Math.min(Math.max(Math.floor(share * points.length), 0), points.length - 1);
          pick(index, event.clientX - box.left);
        }}
        onPointerLeave={() => setActive(null)}
        onBlur={() => setActive(null)}
        className="relative flex h-32 items-end gap-px rounded-sm outline-none"
      >
        {active && shown && (
          <div
            aria-hidden
            className="pointer-events-none absolute top-0 bottom-0 z-10 w-px bg-border-strong"
            style={{ left: active.x }}
          />
        )}
        {points.map((point, index) => {
          const one = values[index];
          const height = peak > 0 && one !== undefined ? Math.max(one > 0 ? 2 : 0, (one / peak) * 100) : 0;
          return (
            <div key={point.day} className="flex h-full min-w-0 flex-1 items-end justify-center">
              <span
                className={cx(
                  "block w-full rounded-xs",
                  // A day with nothing recorded keeps a 1px foot, so the gap reads as a day.
                  height > 0
                    ? active?.index === index
                      ? "bg-accent"
                      : "bg-accent/35"
                    : active?.index === index
                      ? "bg-fg-muted/40"
                      : "bg-fg-muted/15",
                )}
                style={{ height: height > 0 ? `${height}%` : 2 }}
              />
            </div>
          );
        })}
        {active && shown && (
          <div
            className="pointer-events-none absolute top-0 z-20 rounded-lg bg-surface-3 px-2.5 py-1 whitespace-nowrap text-caption shadow-2"
            style={{ left: active.x, transform: "translate(-50%, -100%)" }}
          >
            <span className="font-medium text-fg tabular">{shortDay(shown.day)}</span>
            <span className="text-fg-muted tabular">
              {" "}
              {values[active.index] === undefined ? "unknown" : format(values[active.index])}
            </span>
            <span className="block text-fg-faint">{count(shown.turns, "turn")}</span>
          </div>
        )}
      </div>
      <div className="flex items-center justify-between text-caption text-fg-faint tabular">
        <span>{points[0] ? shortDay(points[0].day) : ""}</span>
        <span>{points.length ? shortDay(points[Math.floor((points.length - 1) / 2)]!.day) : ""}</span>
        <span>{points.length ? shortDay(points[points.length - 1]!.day) : ""}</span>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- cost share

/** One harness's part of the period's cost, for the stacked bar and its legend. */
export interface HarnessShare {
  key: string;
  /** The harness's display name. */
  label: string;
  usd: number;
  /** Part of it is a list-price estimate, not a reported number. */
  estimated: boolean;
}

/** Shades of the one accent, as the composer's context bar does: rank, not a new palette. */
function shareShade(index: number): string {
  if (index === 0) return "var(--rc-accent)";
  if (index === 1) return "color-mix(in srgb, var(--rc-accent) 72%, transparent)";
  if (index === 2) return "color-mix(in srgb, var(--rc-accent) 50%, transparent)";
  if (index === 3) return "color-mix(in srgb, var(--rc-accent) 34%, transparent)";
  if (index === 4) return "var(--rc-text-muted)";
  return "color-mix(in srgb, var(--rc-text-muted) 55%, transparent)";
}

const percentLabel = (part: number, whole: number): string =>
  whole > 0 ? `${Math.round((part / whole) * 100)}%` : "—";

/**
 * One bar split by harness, and a legend that both names the parts and picks
 * which of them the bar and the table show. A hidden harness leaves the bar and
 * its rows step back, so a comparison can be made by elimination.
 */
export function CostShare({
  shares,
  hidden,
  onToggle,
  hovered,
  onHover,
  formatCost,
}: {
  shares: HarnessShare[];
  hidden: ReadonlySet<string>;
  onToggle: (key: string) => void;
  hovered: string | null;
  onHover: (key: string | null) => void;
  formatCost: (usd: number) => string;
}) {
  const rank = new Map(shares.map((share, index) => [share.key, index]));
  const shown = shares.filter((share) => !hidden.has(share.key));
  const total = shown.reduce((sum, share) => sum + share.usd, 0);
  return (
    <div className="flex flex-col gap-3 px-4 py-4">
      <div
        role="img"
        aria-label={shown
          .map((share) => `${share.label} ${formatCost(share.usd)}, ${percentLabel(share.usd, total)}`)
          .join("; ")}
        className="flex h-2.5 gap-px overflow-hidden rounded-full bg-fill-subtle"
      >
        {shown.map((share) => (
          <span
            key={share.key}
            title={`${share.label}: ${formatCost(share.usd)} (${percentLabel(share.usd, total)})`}
            onPointerEnter={() => onHover(share.key)}
            onPointerLeave={() => onHover(null)}
            className="h-full first:rounded-l-full last:rounded-r-full"
            style={{
              width: `${total > 0 ? (share.usd / total) * 100 : 0}%`,
              background: shareShade(rank.get(share.key) ?? 0),
              opacity: hovered && hovered !== share.key ? 0.45 : 1,
            }}
          />
        ))}
      </div>
      <div className="flex flex-wrap gap-x-4 gap-y-1.5">
        {shares.map((share) => {
          const off = hidden.has(share.key);
          return (
            <button
              key={share.key}
              type="button"
              aria-pressed={!off}
              title={off ? `Show ${share.label} again` : `Hide ${share.label} from the bar`}
              onClick={() => onToggle(share.key)}
              onPointerEnter={() => onHover(share.key)}
              onPointerLeave={() => onHover(null)}
              onFocus={() => onHover(share.key)}
              onBlur={() => onHover(null)}
              className={cx(
                "flex min-w-0 items-center gap-1.5 rounded-sm text-caption",
                off ? "text-fg-faint line-through" : "text-fg-muted hover:text-fg",
              )}
            >
              <span
                aria-hidden
                className="size-2 shrink-0 rounded-xs"
                style={{ background: shareShade(rank.get(share.key) ?? 0), opacity: off ? 0.4 : 1 }}
              />
              <span className="truncate">{share.label}</span>
              <span className="tabular">{formatCost(share.usd)}</span>
              <span className="text-fg-faint tabular">{percentLabel(share.usd, total)}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- the glide

/**
 * A container whose direct `[data-glide]` rows glide to their new place when
 * `key` changes (the period switch reorders the table). A row's first position
 * is its own, so opening the page never animates; only `transform` moves.
 */
export function useGlide<T extends HTMLElement>(key: string) {
  const box = useRef<T>(null);
  const last = useRef(new Map<string, number>());
  // biome-ignore lint/correctness/useExhaustiveDependencies: the key is the reorder that moves the rows
  useLayoutEffect(() => {
    const element = box.current;
    if (!element) return;
    const seen = new Set<string>();
    const rows = [...element.querySelectorAll<HTMLElement>("[data-glide]")];
    for (const row of rows) {
      const id = row.dataset.glide ?? "";
      seen.add(id);
      const top = row.getBoundingClientRect().top;
      const was = last.current.get(id);
      last.current.set(id, top);
      if (was === undefined || Math.abs(was - top) < 1) continue;
      row.style.transition = "none";
      row.style.transform = `translateY(${was - top}px)`;
      requestAnimationFrame(() => {
        row.style.transition = "transform 240ms var(--ease-snappy)";
        row.style.transform = "";
      });
    }
    for (const id of [...last.current.keys()]) {
      if (!seen.has(id)) last.current.delete(id);
    }
  }, [key]);
  return box;
}

/** A one-line placeholder where a chart will be, at the size the chart takes. */
export function ChartSkeleton({ children }: { children?: ReactNode }) {
  return (
    <div role="status" aria-label="Loading" className="flex flex-col gap-3 px-4 py-4">
      {children ?? <div className="rc-skeleton h-32 w-full" />}
    </div>
  );
}
