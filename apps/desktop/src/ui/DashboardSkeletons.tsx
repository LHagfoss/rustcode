/**
 * What each dashboard card draws while its data loads: the shape of the content it is about to
 * show (a figure tile, list rows, heatmap cells, columns, share bars), so the page keeps its
 * layout when the numbers arrive instead of swapping flat blocks for content.
 */

import { sessionsListed } from "../state/engine";
import { useApp } from "../state/store";
import { ROW_HEIGHT, useShownRows } from "./DashboardCard";
import { cx, Skeleton } from "./primitives";

/** Widths that vary row to row, so a list skeleton reads as text and not as stripes. */
const ROW_WIDTHS = [72, 58, 84, 64, 76, 52, 80, 68, 60, 74, 56, 82];
const rowWidth = (index: number): number => ROW_WIDTHS[index % ROW_WIDTHS.length] ?? 70;
/** Column heights for the bar charts' skeleton, as percentages of the card's chart area. */
const COLUMN_HEIGHTS = [55, 80, 40, 65, 90, 50, 70];

/** Whether the thread list is still on its way, so a card built from it shows rows instead of its empty line. */
export function useThreadsListing(): boolean {
  return useApp((s) => !sessionsListed(s.engine) && !s.sessions.length);
}

/** One of the four figures over the grid: label, value, note and a sparkline's band. */
export function FigureTileSkeleton() {
  return (
    <div aria-hidden className="flex min-w-0 flex-col rounded-xl bg-surface px-4 py-3">
      <div className="flex min-h-5 items-center justify-between gap-2">
        <Skeleton className="h-2.5 w-20" />
        <Skeleton className="h-2.5 w-12" />
      </div>
      <Skeleton className="mt-1.5 h-6 w-24" />
      <div className="flex min-h-8 flex-col justify-center gap-1.5">
        <Skeleton className="h-2.5 w-[60%]" />
      </div>
      <Skeleton className="mt-2 h-6 w-full rounded-md opacity-60" />
    </div>
  );
}

/** A list card's rows, as many as the card holds: a title and, with `age`, a short figure at the end. */
export function RowsSkeleton({
  rowHeight = ROW_HEIGHT,
  mark = false,
  age = true,
}: {
  rowHeight?: number;
  mark?: boolean;
  age?: boolean;
}) {
  const rows = useShownRows(rowHeight);
  return (
    <div aria-hidden className="-mt-1 flex min-h-0 flex-1 flex-col">
      {Array.from({ length: rows }, (_, index) => (
        <div key={index} className="flex shrink-0 items-center gap-2" style={{ height: rowHeight }}>
          {mark && <Skeleton className="size-2 shrink-0 rounded-full" />}
          <span className="flex min-w-0 flex-1 flex-col gap-1.5">
            <Skeleton className="h-3" style={{ width: `${rowWidth(index)}%` }} />
            {rowHeight > ROW_HEIGHT && (
              <Skeleton className="h-2.5" style={{ width: `${rowWidth(index + 5) / 2}%` }} />
            )}
          </span>
          {age && <Skeleton className="h-2.5 w-10 shrink-0" />}
        </div>
      ))}
    </div>
  );
}

/** The activity card: its headline figures over a block of heatmap-sized cells. */
export function ActivitySkeleton({ stacked }: { stacked: boolean }) {
  return (
    <div
      aria-hidden
      className={cx("flex min-h-0 flex-1", stacked ? "flex-col justify-between gap-3" : "items-end gap-4")}
    >
      {stacked ? (
        <div className="flex flex-col gap-3">
          <Skeleton className="h-6 w-[40%]" />
          <div className="grid grid-cols-3 gap-2">
            {[0, 1, 2].map((figure) => (
              <Skeleton key={figure} className="h-10 rounded-lg" />
            ))}
          </div>
        </div>
      ) : (
        <div className="flex shrink-0 flex-col gap-1.5">
          <Skeleton className="h-5 w-14" />
          <Skeleton className="h-2.5 w-20" />
        </div>
      )}
      <Skeleton className="min-h-12 w-full flex-1 rounded-md opacity-60" />
    </div>
  );
}

/** Today's and the weekly rhythm's columns, each with its day under it; `lead` adds today's figure beside them. */
export function ColumnsSkeleton({ lead = false, wide = true }: { lead?: boolean; wide?: boolean }) {
  return (
    <div aria-hidden className={cx("flex min-h-0 flex-1 gap-4", wide ? "items-stretch" : "flex-col gap-2")}>
      {lead && (
        <div className="flex shrink-0 flex-col justify-end gap-1.5">
          <Skeleton className="h-6 w-12" />
          <Skeleton className="h-2.5 w-28" />
        </div>
      )}
      <div className="flex min-h-0 flex-1 items-stretch gap-1.5">
        {COLUMN_HEIGHTS.map((height, index) => (
          <div key={index} className="flex min-w-0 flex-1 flex-col items-center gap-1">
            <div className="flex min-h-0 w-full flex-1 items-end">
              <Skeleton className="w-full rounded-xs" style={{ height: `${height}%` }} />
            </div>
            <Skeleton className="h-2.5 w-2.5 rounded-xs" />
          </div>
        ))}
      </div>
    </div>
  );
}

/** The models card's labelled bars, in one or two columns. */
export function SharesSkeleton({ rows, columns }: { rows: number; columns: number }) {
  return (
    <div
      aria-hidden
      className={cx("grid min-h-0 flex-1 content-start gap-x-4", columns === 2 && "grid-cols-2")}
    >
      {Array.from({ length: rows * columns }, (_, index) => (
        <div key={index} className="flex h-8 min-w-0 flex-col justify-center gap-1.5">
          <div className="flex items-center justify-between gap-2">
            <Skeleton className="h-3" style={{ width: `${rowWidth(index) / 2}%` }} />
            <Skeleton className="h-2.5 w-16" />
          </div>
          <Skeleton className="h-1.5 w-full rounded-full" />
        </div>
      ))}
    </div>
  );
}

/** The harnesses card: its split bar and a short legend. */
export function SplitSkeleton({ rows, wide }: { rows: number; wide: boolean }) {
  return (
    <div aria-hidden className="flex min-h-0 flex-1 flex-col gap-3">
      <Skeleton className="h-2.5 w-full shrink-0 rounded-full" />
      <div className={cx("grid min-h-0 content-start gap-x-4 gap-y-1", wide && "grid-cols-2")}>
        {Array.from({ length: Math.min(rows, 4) }, (_, index) => (
          <div key={index} className="flex h-[18px] items-center gap-2">
            <Skeleton className="size-2 shrink-0 rounded-full" />
            <span className="flex min-w-0 flex-1">
              <Skeleton className="h-3" style={{ width: `${rowWidth(index) / 1.5}%` }} />
            </span>
            <Skeleton className="h-2.5 w-7 shrink-0" />
          </div>
        ))}
      </div>
    </div>
  );
}

/** The spend card's figure, token line and sparkline. */
export function SpendSkeleton() {
  return (
    <div aria-hidden className="flex flex-1 flex-col justify-end gap-1.5">
      <Skeleton className="h-5 w-[45%]" />
      <Skeleton className="h-2.5 w-[70%]" />
      <Skeleton className="mt-1 min-h-5 w-full flex-1 rounded-md opacity-60" />
    </div>
  );
}
