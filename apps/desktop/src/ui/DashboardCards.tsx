/**
 * The dashboard's charts: the figures over the grid (spend, turns, tokens and threads, each with
 * its trend and thirty days behind it), and the cards that read the same usage report a second
 * way: today against yesterday, the models and harnesses that ran, and the weekdays you work on.
 * Each card lays out for the room the grid gives it: side by side when wide, stacked when narrow,
 * and as many rows as its height holds.
 */

import { useMemo } from "react";
import type { UsageRow } from "../acp/rustcode";
import { count } from "../lib/plural";
import { useInboxItems } from "../state/inboxRead";
import { DAY_MS, daysAgo, previousRows, trend } from "../state/period";
import { costLine, costOfRows, formatBestCost } from "../state/pricing";
import { useApp } from "../state/store";
import { formatTokens } from "../state/thread";
import { isRunning } from "../state/turn";
import { periodTotals, type Share, sharesBy, turnsPerDay, weekdayTurns } from "../state/widgets";
import { Card, Nothing, useShownRows } from "./DashboardCard";
import { Bot, CalendarDays, Layers, Sun } from "./icons";
import { cx, Skeleton, Tooltip } from "./primitives";
import { shareShade, Tile, TrendPill } from "./StatisticsCharts";
import { useCardHeight, useWidgetColumns } from "./WidgetGrid";

/** The days the figures' lines and the harness split cover. */
const RECENT_DAYS = 30;
const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const WEEKDAY_NAMES = ["Mondays", "Tuesdays", "Wednesdays", "Thursdays", "Fridays", "Saturdays", "Sundays"];

const today = () => new Date().toISOString().slice(0, 10);
const lastDays = (days: number) => Array.from({ length: days }, (_, i) => daysAgo(new Date(), days - 1 - i));

function useHarnessNames(): Map<string, string> {
  const harnesses = useApp((s) => s.harnesses?.harnesses);
  return useMemo(() => new Map((harnesses ?? []).map((harness) => [harness.id, harness.name])), [harnesses]);
}

/**
 * The four figures over the grid. Spend is this month against the month before; turns are this
 * week against the week before; tokens are thirty days against the thirty before; threads are the
 * ones that ran this week, with what is running and waiting now.
 */
export function DashboardFigures({
  rows,
  month,
  twoMonths,
}: {
  rows?: UsageRow[];
  month?: UsageRow[];
  twoMonths?: UsageRow[];
}) {
  const sessions = useApp((s) => s.sessions);
  const running = useApp((s) => Object.values(s.threads).filter(isRunning).length);
  const waiting = new Set(useInboxItems().map((item) => item.threadId)).size;
  const figures = useMemo(() => {
    const days = lastDays(RECENT_DAYS);
    const all = rows ?? [];
    const byDay = new Map<string, UsageRow[]>();
    for (const row of all) if (row.day) byDay.set(row.day, [...(byDay.get(row.day) ?? []), row]);
    const week = periodTotals(all, daysAgo(new Date(), 6));
    const weekBefore = periodTotals(all, daysAgo(new Date(), 13), daysAgo(new Date(), 7));
    const recent = periodTotals(all, days[0] ?? "");
    const recentBefore = periodTotals(
      all,
      daysAgo(new Date(), 2 * RECENT_DAYS - 1),
      daysAgo(new Date(), RECENT_DAYS),
    );
    const weekFrom = Date.now() - 7 * DAY_MS;
    const dayOf = (at?: number) => (at ? new Date(at).toISOString().slice(0, 10) : "");
    return {
      week,
      weekBefore,
      recent,
      recentBefore,
      threads: sessions.filter((session) => (session.updatedAt ?? 0) >= weekFrom).length,
      turns: days.map((day) => periodTotals(byDay.get(day) ?? [], day, day).turns),
      tokens: days.map((day) => periodTotals(byDay.get(day) ?? [], day, day).tokens),
      cost: days.map((day) => costOfRows(byDay.get(day) ?? []).usd),
      active: days.map((day) => sessions.filter((session) => dayOf(session.updatedAt) === day).length),
    };
  }, [rows, sessions]);
  const spend = useMemo(
    () =>
      month && twoMonths
        ? { now: costOfRows(month), before: costOfRows(previousRows(twoMonths, month)) }
        : undefined,
    [month, twoMonths],
  );
  if (!rows || !spend) {
    return (
      <div className="grid shrink-0 grid-cols-[repeat(auto-fit,minmax(200px,1fr))] gap-2">
        {[0, 1, 2, 3].map((tile) => (
          <Skeleton key={tile} className="h-[132px] rounded-xl" />
        ))}
      </div>
    );
  }
  return (
    <div className="rc-overview-in grid shrink-0 grid-cols-[repeat(auto-fit,minmax(200px,1fr))] gap-2">
      <Tile
        label="Spend this month"
        value={formatBestCost(spend.now)}
        exact={costLine(spend.now)}
        note={`${formatBestCost(spend.before)} the month before`}
        trend={trend(spend.now.usd, spend.before.usd)}
        compare="against the month before"
        series={figures.cost}
        seriesLabel={`Cost per day over ${RECENT_DAYS} days`}
      />
      <Tile
        label="Turns this week"
        value={figures.week.turns.toLocaleString()}
        count={figures.week.turns}
        note={`${count(figures.weekBefore.turns, "turn")} the week before`}
        trend={trend(figures.week.turns, figures.weekBefore.turns)}
        compare="against the week before"
        series={figures.turns}
        seriesLabel={`Turns per day over ${RECENT_DAYS} days`}
      />
      <Tile
        label={`Tokens · ${RECENT_DAYS} days`}
        value={formatTokens(figures.recent.tokens)}
        exact={`${figures.recent.tokens.toLocaleString()} input and output tokens`}
        note={`${count(figures.recent.turns, "turn")} · ${formatTokens(figures.recentBefore.tokens)} the ${RECENT_DAYS} days before`}
        trend={trend(figures.recent.tokens, figures.recentBefore.tokens)}
        compare={`against the ${RECENT_DAYS} days before`}
        series={figures.tokens}
        seriesLabel={`Tokens per day over ${RECENT_DAYS} days`}
      />
      <Tile
        label="Threads this week"
        value={figures.threads.toLocaleString()}
        count={figures.threads}
        note={`${running} running · ${waiting} waiting on you`}
        compare="last active in 7 days"
        series={figures.active}
        seriesLabel={`Threads by the day they last ran, over ${RECENT_DAYS} days`}
      />
    </div>
  );
}

/** Upright bars for a short run of values, the last or the busiest one lit, each named under it. */
function Columns({
  values,
  labels,
  lit,
  tip,
}: {
  values: readonly number[];
  labels: readonly string[];
  lit: number;
  tip: (index: number) => string;
}) {
  const peak = Math.max(1, ...values);
  return (
    <div className="flex min-h-0 flex-1 items-stretch gap-1.5">
      {values.map((value, index) => (
        <Tooltip key={tip(index)} label={tip(index)}>
          <div className="flex min-w-0 flex-1 flex-col items-center gap-1">
            <div className="flex min-h-0 w-full flex-1 items-end">
              <div
                className={cx(
                  "rc-dash-bar w-full rounded-xs",
                  index === lit ? "bg-accent" : value > 0 ? "bg-accent/35" : "bg-fill-subtle",
                )}
                style={{ height: `${Math.max(4, (value / peak) * 100)}%` }}
              />
            </div>
            <span
              className={cx(
                "text-caption leading-none tabular",
                index === lit ? "font-medium text-fg" : "text-fg-muted",
              )}
            >
              {labels[index]}
            </span>
          </div>
        </Tooltip>
      ))}
    </div>
  );
}

/** Today's turns against yesterday's, and the last seven days as columns, today's lit. */
export function TodayCard({ rows }: { rows?: UsageRow[] }) {
  const wide = useWidgetColumns() > 1;
  const data = useMemo(() => {
    const days = lastDays(7);
    const turns = turnsPerDay(rows ?? []);
    const now = periodTotals(rows ?? [], today(), today());
    const yesterday = periodTotals(rows ?? [], daysAgo(new Date(), 1), daysAgo(new Date(), 1));
    return {
      days,
      values: days.map((day) => turns.get(day) ?? 0),
      now,
      yesterday,
      labels: days.map((day) =>
        new Date(`${day}T00:00:00Z`).toLocaleDateString(undefined, { weekday: "narrow", timeZone: "UTC" }),
      ),
    };
  }, [rows]);
  const change = trend(data.now.turns, data.yesterday.turns);
  return (
    <Card
      label="Today"
      icon={Sun}
      aside={rows && change ? <TrendPill trend={change} compare="against yesterday" /> : undefined}
    >
      {rows === undefined ? (
        <Skeleton className="w-full flex-1" />
      ) : (
        <div className={cx("flex min-h-0 flex-1 gap-4", wide ? "items-stretch" : "flex-col gap-2")}>
          <div className="flex shrink-0 flex-col justify-end">
            <span className="flex items-baseline gap-1.5">
              <span className="font-semibold text-title2 text-fg tabular">
                {data.now.turns.toLocaleString()}
              </span>
              <span className="text-caption text-fg-muted">{data.now.turns === 1 ? "turn" : "turns"}</span>
            </span>
            <span className="text-caption text-fg-muted tabular">
              {formatTokens(data.now.tokens)} tokens · {data.yesterday.turns.toLocaleString()} yesterday
            </span>
          </div>
          <Columns
            values={data.values}
            labels={data.labels}
            lit={data.values.length - 1}
            tip={(index) => `${data.days[index]} · ${count(data.values[index] ?? 0, "turn")}`}
          />
        </div>
      )}
    </Card>
  );
}

/** A share as a labelled bar: its name and part over a track filled to it. */
function ShareRow({
  name,
  part,
  value,
  shade,
}: {
  name: string;
  part: number;
  value: string;
  shade: string;
}) {
  return (
    <div className="flex h-8 min-w-0 shrink-0 flex-col justify-center gap-1">
      <div className="flex min-w-0 items-baseline justify-between gap-2 text-callout">
        <span className="truncate text-fg">{name}</span>
        <span className="shrink-0 text-caption text-fg-muted tabular">{value}</span>
      </div>
      <div className="h-1.5 w-full overflow-hidden rounded-full bg-fill-subtle">
        <div
          className="rc-dash-bar h-full rounded-full"
          style={{ width: `${Math.max(2, part * 100)}%`, background: shade }}
        />
      </div>
    </div>
  );
}

const SHARE_ROW = 36;

/** This month's turns by model, each with its cost when one is known. */
export function ModelsCard({ month }: { month?: UsageRow[] }) {
  const columns = useWidgetColumns() > 1 ? 2 : 1;
  const fit = useShownRows(SHARE_ROW) * columns;
  const shares = useMemo(() => sharesBy(month ?? [], "model"), [month]);
  const costs = useMemo(() => {
    const byModel = new Map<string, UsageRow[]>();
    for (const row of month ?? []) {
      const key = row.model || "Default model";
      byModel.set(key, [...(byModel.get(key) ?? []), row]);
    }
    return new Map([...byModel].map(([key, rows]) => [key, costOfRows(rows)]));
  }, [month]);
  const shown = shares.slice(0, fit);
  const rest = shares.length - shown.length;
  return (
    <Card
      label="Models this month"
      icon={Bot}
      aside={rest > 0 ? <span className="text-caption text-fg-muted tabular">+{rest}</span> : undefined}
    >
      {month === undefined ? (
        <Skeleton className="w-full flex-1" />
      ) : shown.length === 0 ? (
        <Nothing>No turns this month yet.</Nothing>
      ) : (
        <div className={cx("grid min-h-0 flex-1 content-start gap-x-4", columns === 2 && "grid-cols-2")}>
          {shown.map((share, index) => {
            const cost = costs.get(share.key);
            const priced = cost?.usd !== undefined ? ` · ${formatBestCost(cost)}` : "";
            return (
              <Tooltip
                key={share.key}
                label={`${share.key}: ${count(share.turns, "turn")}, ${formatTokens(share.tokens)} tokens${priced}`}
              >
                <div>
                  <ShareRow
                    name={share.key}
                    part={share.part}
                    shade={shareShade(index)}
                    value={`${Math.round(share.part * 100)}%${priced}`}
                  />
                </div>
              </Tooltip>
            );
          })}
        </div>
      )}
    </Card>
  );
}

/** This month's turns by harness: one bar split between them, and a legend under it. */
export function HarnessesCard({ month }: { month?: UsageRow[] }) {
  const names = useHarnessNames();
  const shares: Share[] = useMemo(() => sharesBy(month ?? [], "harness"), [month]);
  const legend = Math.max(1, Math.floor((useCardHeight() - 52 - 18) / 22)) * (useWidgetColumns() > 1 ? 2 : 1);
  const wide = useWidgetColumns() > 1;
  return (
    <Card
      label="Harnesses"
      icon={Layers}
      aside={<span className="text-caption text-fg-muted">This month</span>}
    >
      {month === undefined ? (
        <Skeleton className="w-full flex-1" />
      ) : shares.length === 0 ? (
        <Nothing>No turns this month yet.</Nothing>
      ) : (
        <div className="flex min-h-0 flex-1 flex-col gap-3">
          <div
            className="flex h-2.5 w-full shrink-0 gap-0.5 overflow-hidden rounded-full"
            role="img"
            aria-label="Turns by harness"
          >
            {shares.map((share, index) => (
              <div
                key={share.key}
                className="rc-dash-bar h-full"
                style={{ width: `${share.part * 100}%`, background: shareShade(index) }}
              />
            ))}
          </div>
          <ul className={cx("grid min-h-0 content-start gap-x-4 gap-y-1", wide && "grid-cols-2")}>
            {shares.slice(0, legend).map((share, index) => (
              <li key={share.key} className="flex h-[18px] min-w-0 items-center gap-2 text-callout">
                <span className="size-2 shrink-0 rounded-full" style={{ background: shareShade(index) }} />
                <span className="min-w-0 flex-1 truncate text-fg">{names.get(share.key) ?? share.key}</span>
                <span className="shrink-0 text-caption text-fg-muted tabular">
                  {Math.round(share.part * 100)}%
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </Card>
  );
}

/** Turns per weekday over the activity window, the busiest day lit and named. */
export function RhythmCard({ rows }: { rows?: UsageRow[] }) {
  const values = useMemo(() => weekdayTurns(turnsPerDay(rows ?? [])), [rows]);
  const busiest = values.indexOf(Math.max(...values));
  const any = values.some((value) => value > 0);
  const letters = useWidgetColumns() > 1 ? 3 : 1;
  return (
    <Card
      label="Weekly rhythm"
      icon={CalendarDays}
      aside={
        any ? (
          <span className="truncate text-caption text-fg-muted">Most on {WEEKDAY_NAMES[busiest]}</span>
        ) : undefined
      }
    >
      {rows === undefined ? (
        <Skeleton className="w-full flex-1" />
      ) : !any ? (
        <Nothing>No turns recorded yet.</Nothing>
      ) : (
        <Columns
          values={values}
          labels={WEEKDAYS.map((day) => day.slice(0, letters))}
          lit={busiest}
          tip={(index) => `${WEEKDAY_NAMES[index]}: ${count(values[index] ?? 0, "turn")}`}
        />
      )}
    </Card>
  );
}
