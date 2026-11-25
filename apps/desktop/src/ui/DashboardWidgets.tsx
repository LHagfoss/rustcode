/**
 * The dashboard's figures and cards: what this computer recorded lately (the activity heatmap,
 * spend, models, harnesses and rhythm), the project's recent threads and pull requests, what
 * waits on the user and what is running. The user picks, orders and sizes the cards
 * (`WidgetGrid.tsx`).
 *
 * Every card draws from a read the app already makes: one `_rustcode/data/usage` report grouped
 * by day, two by model to price the spend, the session list the sidebar shows, the inbox, and the
 * project's pulls cache (`state/pulls.ts`). No new engine call. Lists show the rows a card's
 * height holds and say how many more there are at the end of their label row.
 */

import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import type { PullRequest, UsageRow } from "../acp/rustcode";
import { parseUsageReport, pullErrorTitle } from "../acp/rustcode";
import { Method } from "../acp/types";
import { openExternal } from "../lib/native";
import { count } from "../lib/plural";
import { useInboxItems } from "../state/inboxRead";
import {
  daysAgo,
  previousRows,
  sparklinePath,
  sparklineSeries,
  sumRows,
  trend,
  utcOffsetMinutes,
} from "../state/period";
import { costLine, costOfRows, formatBestCost } from "../state/pricing";
import {
  cachedPulls,
  loadPulls,
  pullState,
  pullsError,
  pullsVersion,
  retryPulls,
  subscribePulls,
} from "../state/pulls";
import { openSettings, selectThread, setPanel, setView, useApp } from "../state/store";
import { formatTokens, relativeAge } from "../state/thread";
import { isRunning } from "../state/turn";
import type { WidgetId } from "../state/widgetLayout";
import {
  activitySummary,
  HEATMAP_WEEKS,
  type HeatCell,
  heatmapWeeks,
  monthBounds,
  projectThreads,
  type Streak,
  streakOf,
  turnsPerDay,
  weeksOfHistory,
} from "../state/widgets";
import {
  CARD_CHROME,
  Calm,
  Card,
  CardList,
  CardRow,
  LabelNote,
  Nothing,
  ROW_HEIGHT,
  TWO_LINE_ROW_HEIGHT,
  useShownRows,
  useTwoLines,
} from "./DashboardCard";
import { DashboardFigures, HarnessesCard, ModelsCard, RhythmCard, TodayCard } from "./DashboardCards";
import { ActivitySkeleton, RowsSkeleton, SpendSkeleton, useThreadsListing } from "./DashboardSkeletons";
import {
  Activity,
  CircleCheck,
  Flame,
  Gauge,
  GitMerge,
  GitPullRequest,
  GitPullRequestArrow,
  GitPullRequestClosed,
  GitPullRequestDraft,
  Hand,
  History,
  Play,
} from "./icons";
import { cx, Pill, Tooltip } from "./primitives";
import { TrendPill } from "./StatisticsCharts";
import { useEngineData } from "./settingsKit";
import { useCardHeight, useWidgetRows, WidgetGrid } from "./WidgetGrid";

/** The spend card's token line: the last thirty days, counted as whole local days. */
const USAGE_DAYS = 30;

/** A heatmap cell's smallest and largest side and the space between cells, in pixels: squares, as a contribution graph draws them. */
const HEAT_CELL = 8;
const HEAT_CELL_MAX = 14;
/** A two-row activity card stacks its numbers over the heatmap: their line's height, and the larger cells it allows. */
const STACKED_STATS = 100;
const STACKED_HEAT_CELL_MAX = 24;
/** A two-row card with a short history puts its numbers beside the heatmap, whose few weeks then take larger cells. */
const SIDE_HEAT_CELL_MAX = 30;
/** Under this many weeks of history the heatmap draws only those weeks rather than blank ones before them. */
const SHORT_HISTORY_WEEKS = 16;
const HEAT_GAP = 2;
/** The fewest weeks the heatmap draws, however narrow its card. */
const MIN_WEEKS = 8;

/** A day's shade in the heatmap, as the design system's own fills and accent. */
const SHADE: Record<HeatCell["level"], string> = {
  0: "bg-fill-subtle",
  1: "bg-accent/25",
  2: "bg-accent/45",
  3: "bg-accent/70",
  4: "bg-accent",
};

/**
 * The streak at the end of the activity card's label: a flame and the days in a row with a turn.
 * It is lit once today counts; until then it is quiet and says what keeps the run going.
 */
function StreakBadge({ streak }: { streak: Streak }) {
  if (streak.current === 0 && streak.longest === 0) return null;
  const best = streak.longest > streak.current ? ` · best ${count(streak.longest, "day")}` : "";
  const label =
    streak.current === 0
      ? `No streak right now${best}. Run a turn today to start one.`
      : streak.today
        ? `${count(streak.current, "day")} in a row${best}. Today counts.`
        : `${count(streak.current, "day")} in a row${best}. Run a turn today to keep it.`;
  return (
    <Tooltip label={label}>
      <span
        role="img"
        aria-label={label}
        className={cx(
          "rc-streak flex shrink-0 items-center gap-1 rounded-full px-2 text-caption font-medium tabular",
          streak.today ? "bg-accent/15 text-accent" : "bg-fill-subtle text-fg-muted",
        )}
      >
        <Flame size={12} className={cx("shrink-0", streak.today && "rc-streak-flame")} />
        {streak.current === 0 ? "Start a streak" : `${streak.current}-day streak`}
      </span>
    </Tooltip>
  );
}

/** How many whole weeks of square cells fit the heatmap's width; the ref goes on the heatmap's box. */
function useFittedWeeks(cell: number, most: number) {
  const [element, setElement] = useState<HTMLDivElement | null>(null);
  const [weeks, setWeeks] = useState(MIN_WEEKS);
  useEffect(() => {
    if (!element) return;
    const fit = () =>
      setWeeks(
        Math.max(MIN_WEEKS, Math.min(most, Math.floor((element.clientWidth + HEAT_GAP) / (cell + HEAT_GAP)))),
      );
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(element);
    return () => observer.disconnect();
  }, [element, cell, most]);
  return [setElement, weeks] as const;
}

/** From this cell side the heatmap names the months over its columns. */
const MONTH_LABELS_FROM = 12;
/** The height the month names take over the heatmap, with the gap under them. */
const MONTH_ROW = 14;

/**
 * The month to name over each of the heatmap's columns: where a new month starts, unless the last
 * name is too close to leave room for this one.
 */
function monthLabels(
  columns: readonly (readonly (HeatCell | null)[])[],
  nameFirst = false,
): (string | undefined)[] {
  let last = -3;
  let month = "";
  return columns.map((week, index) => {
    const day = week.find(Boolean)?.day;
    if (!day) return undefined;
    const next = day.slice(0, 7);
    if (next === month) return undefined;
    const first = month === "";
    month = next;
    if ((first && !nameFirst) || index - last < 3) return undefined;
    last = index;
    return new Date(`${day}T00:00:00Z`).toLocaleString(undefined, { month: "short", timeZone: "UTC" });
  });
}

/**
 * How the activity card lays out: its numbers over the heatmap in a tall card, beside it in a
 * tall card with a short history (whose few weeks would leave the width empty), or a short
 * column beside it in a one-row card.
 */
type ActivityLayout = "stacked" | "side" | "compact";

/** The heatmap cell's side that fits a week's seven days in the card's height, under the stacked numbers or beside them. */
function useHeatCell(layout: ActivityLayout): number {
  const stacked = layout === "stacked";
  const room = useCardHeight() - CARD_CHROME - 6 * HEAT_GAP - MONTH_ROW - (stacked ? STACKED_STATS : 0);
  const most = stacked ? STACKED_HEAT_CELL_MAX : layout === "side" ? SIDE_HEAT_CELL_MAX : HEAT_CELL_MAX;
  return Math.max(HEAT_CELL, Math.min(most, Math.floor(room / 7)));
}

const shortDay = (day: string) =>
  new Date(`${day}T00:00:00Z`).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    timeZone: "UTC",
  });

/** One of the two-row activity card's small figures: a label over its value. */
function ActivityFigure({ label, value, detail }: { label: string; value: string; detail?: string }) {
  return (
    <div className="flex min-w-0 flex-col rounded-lg bg-fill-subtle px-2 py-1">
      <dt className="truncate text-caption text-fg-muted">{label}</dt>
      <dd className="truncate text-callout font-medium text-fg tabular">
        {value}
        {detail && <span className="font-normal text-caption text-fg-muted"> · {detail}</span>}
      </dd>
    </div>
  );
}

function ActivityCard({ rows, since }: { rows?: UsageRow[]; since: string }) {
  const sessions = useApp((s) => s.sessions);
  const turns = useMemo(() => turnsPerDay(rows ?? []), [rows]);
  const history = useMemo(() => weeksOfHistory(turns), [turns]);
  const short = history > 0 && history < SHORT_HISTORY_WEEKS;
  const tall = useWidgetRows() === 2;
  const layout: ActivityLayout = !tall ? "compact" : short ? "side" : "stacked";
  const stacked = layout === "stacked";
  const side = useHeatCell(layout);
  const [box, weeks] = useFittedWeeks(side, short ? Math.max(MIN_WEEKS, history) : HEATMAP_WEEKS);
  const columns = useMemo(() => heatmapWeeks(turns, weeks), [turns, weeks]);
  const months = useMemo(
    () => (side >= MONTH_LABELS_FROM ? monthLabels(columns, layout === "side") : undefined),
    [columns, side, layout],
  );
  const streak = useMemo(() => streakOf(turns), [turns]);
  const summary = useMemo(() => activitySummary(turns), [turns]);
  const shownTurns = useMemo(
    () => columns.flat().reduce((sum, cell) => sum + (cell?.turns ?? 0), 0),
    [columns],
  );
  const activeThreads = useMemo(() => {
    const from = Date.parse(`${since}T00:00:00Z`);
    return sessions.filter((session) => (session.updatedAt ?? 0) >= from).length;
  }, [sessions, since]);
  return (
    <Card
      label="Activity"
      icon={Activity}
      aside={rows === undefined ? undefined : <StreakBadge streak={streak} />}
    >
      {rows === undefined ? (
        <ActivitySkeleton stacked={tall} />
      ) : turns.size === 0 ? (
        <Nothing>No turns recorded yet.</Nothing>
      ) : (
        <div
          className={cx(
            "flex min-h-0 flex-1",
            stacked
              ? "flex-col justify-between gap-2"
              : layout === "side"
                ? "items-center gap-6"
                : "items-end gap-4",
          )}
        >
          {layout !== "compact" ? (
            <div className={cx("flex flex-col gap-3", layout === "side" && "w-[38%] max-w-64 shrink-0")}>
              <div className={cx("flex items-baseline gap-x-2", layout === "side" && "flex-wrap")}>
                <span className="font-semibold text-title2 text-fg tabular">
                  {shownTurns.toLocaleString()}
                </span>
                <span className={cx("text-caption text-fg-muted", stacked && "truncate")}>
                  turns in {weeks} weeks · {count(activeThreads, "thread")} active
                </span>
              </div>
              <dl className={cx("grid gap-2", stacked && "grid-cols-3")}>
                <ActivityFigure label="This week" value={count(summary.week, "turn")} />
                <ActivityFigure
                  label="Busiest day"
                  value={summary.busiest ? count(summary.busiest.turns, "turn") : "None yet"}
                  detail={summary.busiest && shortDay(summary.busiest.day)}
                />
                <ActivityFigure label="Longest streak" value={count(streak.longest, "day")} />
              </dl>
            </div>
          ) : (
            <div className="flex shrink-0 flex-col">
              <span className="font-semibold text-title3 text-fg tabular">{shownTurns.toLocaleString()}</span>
              <span className="text-caption text-fg-muted">turns in {weeks} weeks</span>
              <span className="text-caption text-fg-muted">{count(activeThreads, "thread")} active</span>
            </div>
          )}
          <div className={cx("flex min-w-0 flex-1 flex-col", layout !== "compact" && "justify-center gap-2")}>
            <div
              ref={box}
              role="img"
              aria-label={`Turns per day over the last ${weeks} weeks: ${count(shownTurns, "turn")}.`}
              className={cx(
                "flex min-w-0 overflow-hidden",
                layout === "side" ? "justify-center" : "justify-end",
              )}
              style={{ gap: HEAT_GAP }}
            >
              {columns.map((week, index) => (
                <div
                  key={week.find(Boolean)?.day ?? index}
                  className="flex shrink-0 flex-col"
                  style={{ gap: HEAT_GAP }}
                >
                  {months && (
                    <span className="relative shrink-0" style={{ height: MONTH_ROW - HEAT_GAP }}>
                      {months[index] && (
                        <span className="absolute bottom-0 left-0 whitespace-nowrap text-caption text-fg-muted leading-none">
                          {months[index]}
                        </span>
                      )}
                    </span>
                  )}
                  {week.map((cell, day) => (
                    <span
                      key={cell?.day ?? `future-${index}-${day}`}
                      title={cell ? `${cell.day} · ${count(cell.turns, "turn")}` : undefined}
                      className={cx("rc-heat-cell rounded-xs", cell && SHADE[cell.level])}
                      style={{ width: side, height: side }}
                    />
                  ))}
                </div>
              ))}
            </div>
            {layout !== "compact" && (
              <div
                aria-hidden
                className={cx(
                  "flex items-center gap-1 text-caption text-fg-muted",
                  layout === "side" ? "justify-center" : "justify-end",
                )}
              >
                Less
                {([0, 1, 2, 3, 4] as const).map((level) => (
                  <span key={level} className={cx("size-2.5 rounded-xs", SHADE[level])} />
                ))}
                More
              </div>
            )}
          </div>
        </div>
      )}
    </Card>
  );
}

function RecentThreadsCard({ project }: { project: string }) {
  const sessions = useApp((s) => s.sessions);
  const threads = useMemo(() => projectThreads(sessions, project), [sessions, project]);
  const shown = threads.slice(0, useShownRows());
  const more = threads.length - shown.length;
  const listing = useThreadsListing();
  return (
    <Card
      label="Recent threads"
      icon={History}
      aside={
        more > 0 ? (
          <LabelNote title={`${count(more, "more thread", "more threads")} in the sidebar`}>
            +{more}
          </LabelNote>
        ) : undefined
      }
    >
      {listing ? (
        <RowsSkeleton />
      ) : shown.length ? (
        <CardList>
          {shown.map((thread) => (
            <CardRow
              key={thread.id}
              onClick={() => selectThread(thread.id)}
              title={thread.title || "Untitled thread"}
            >
              <span className="min-w-0 flex-1 truncate text-fg">{thread.title || "Untitled thread"}</span>
              <span className="shrink-0 text-caption text-fg-muted tabular">
                {relativeAge(thread.updatedAt)}
              </span>
            </CardRow>
          ))}
        </CardList>
      ) : (
        <Nothing>No threads in this project yet.</Nothing>
      )}
    </Card>
  );
}

/** This month's spend against the month before, and thirty days of tokens as a line. */
function SpendCard({
  rows,
  month,
  twoMonths,
  usageSince,
}: {
  rows?: UsageRow[];
  /** By model since the month began, and since the month before began. */
  month?: UsageRow[];
  twoMonths?: UsageRow[];
  usageSince: string;
}) {
  const spend = useMemo(
    () =>
      month && twoMonths
        ? { now: costOfRows(month), before: costOfRows(previousRows(twoMonths, month)) }
        : undefined,
    [month, twoMonths],
  );
  const { tokens, turns, series } = useMemo(() => {
    const recent = (rows ?? []).filter((row) => (row.day ?? "") >= usageSince);
    const perDay = new Map<string, number>();
    for (const row of recent) {
      if (!row.day) continue;
      perDay.set(row.day, (perDay.get(row.day) ?? 0) + (row.input ?? 0) + (row.output ?? 0));
    }
    const now = new Date();
    const totals = sumRows(recent);
    return {
      tokens: (totals.input ?? 0) + (totals.output ?? 0),
      turns: totals.turns,
      series: Array.from({ length: USAGE_DAYS }, (_, i) => perDay.get(daysAgo(now, USAGE_DAYS - 1 - i))),
    };
  }, [rows, usageSince]);
  const change =
    spend?.now.usd === undefined || spend.before.usd === undefined
      ? undefined
      : trend(spend.now.usd, spend.before.usd);
  const path = sparklinePath(sparklineSeries(series), 100, 20);
  return (
    <Card
      label="Spend"
      icon={Gauge}
      aside={change ? <TrendPill trend={change} compare="against the month before" /> : undefined}
    >
      {rows === undefined || !spend ? (
        <SpendSkeleton />
      ) : turns === 0 && spend.now.usd === undefined ? (
        <Nothing>No usage recorded in {USAGE_DAYS} days.</Nothing>
      ) : (
        <div className="flex min-h-0 flex-1 flex-col justify-end">
          <span className="flex items-baseline gap-1 truncate" title={costLine(spend.now)}>
            <span className="font-semibold text-title3 text-fg tabular">{formatBestCost(spend.now)}</span>
            <span className="text-caption text-fg-muted">this month</span>
          </span>
          <span
            className="truncate text-caption text-fg-muted tabular"
            title={`${tokens.toLocaleString()} input and output tokens in ${USAGE_DAYS} days`}
          >
            {formatTokens(tokens)} tokens · {count(turns, "turn")} in {USAGE_DAYS} days
          </span>
          <svg
            viewBox="0 0 100 20"
            preserveAspectRatio="none"
            aria-hidden
            className="mt-1 min-h-5 w-full flex-1 text-accent/60"
          >
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
      )}
    </Card>
  );
}

function NeedsYouCard() {
  const items = useInboxItems();
  const threads = useMemo(() => {
    const seen = new Set<string>();
    return items.filter((item) => !seen.has(item.threadId) && seen.add(item.threadId));
  }, [items]);
  const twoLines = useTwoLines();
  const shown = threads.slice(0, useShownRows(twoLines ? TWO_LINE_ROW_HEIGHT : ROW_HEIGHT));
  const listing = useThreadsListing();
  return (
    <Card
      label="Needs you"
      icon={Hand}
      aside={
        <LabelNote title="Open the inbox" onClick={() => setView({ kind: "inbox" })}>
          {threads.length ? (
            <span className="rounded-full bg-warning/14 px-2 font-medium text-warning">{threads.length}</span>
          ) : (
            "Inbox"
          )}
        </LabelNote>
      }
    >
      {listing ? (
        <RowsSkeleton mark age={false} rowHeight={twoLines ? TWO_LINE_ROW_HEIGHT : ROW_HEIGHT} />
      ) : shown.length ? (
        <CardList>
          {shown.map((item) => (
            <CardRow
              key={item.key}
              onClick={() => selectThread(item.threadId)}
              title={`${item.title}: ${item.detail}`}
              detail={twoLines ? item.detail : undefined}
              mark={<span className="size-1.5 shrink-0 rounded-full bg-warning" />}
            >
              <span className="min-w-0 flex-1 truncate text-fg">{item.title}</span>
            </CardRow>
          ))}
        </CardList>
      ) : (
        <Calm icon={CircleCheck}>Nothing is waiting on you</Calm>
      )}
    </Card>
  );
}

function RunningCard() {
  const running = useApp((s) =>
    Object.values(s.threads)
      .filter(isRunning)
      .map((thread) => `${thread.id}\t${thread.turn.startedAt}\t${thread.title ?? ""}`)
      .join("\n"),
  );
  const sessions = useApp((s) => s.sessions);
  const threads = useMemo(
    () =>
      running
        ? running
            .split("\n")
            .map((line) => {
              const [id = "", at = "0", title = ""] = line.split("\t");
              const name = title || sessions.find((session) => session.id === id)?.title || "Untitled thread";
              return { id, at: Number(at), title: name };
            })
            .sort((one, other) => other.at - one.at)
        : [],
    [running, sessions],
  );
  const shown = threads.slice(0, useShownRows());
  const more = threads.length - shown.length;
  const listing = useThreadsListing();
  const lastRun = useMemo(
    () =>
      sessions.reduce<(typeof sessions)[number] | undefined>(
        (latest, session) => ((session.updatedAt ?? 0) > (latest?.updatedAt ?? 0) ? session : latest),
        undefined,
      ),
    [sessions],
  );
  return (
    <Card
      label="Running now"
      icon={Play}
      aside={
        more > 0 ? (
          <LabelNote title={`${count(more, "more thread", "more threads")} running`}>+{more}</LabelNote>
        ) : undefined
      }
    >
      {listing ? (
        <RowsSkeleton mark />
      ) : shown.length ? (
        <CardList>
          {shown.map((thread) => (
            <CardRow key={thread.id} onClick={() => selectThread(thread.id)} title={thread.title}>
              <span className="size-1.5 shrink-0 animate-pulse rounded-full bg-accent" />
              <span className="min-w-0 flex-1 truncate text-fg">{thread.title}</span>
              <span className="shrink-0 text-caption text-fg-muted tabular">{relativeAge(thread.at)}</span>
            </CardRow>
          ))}
        </CardList>
      ) : (
        <Calm
          icon={Play}
          detail={
            lastRun && (
              <button
                type="button"
                onClick={() => selectThread(lastRun.id)}
                className="flex max-w-full items-baseline gap-1 rounded-sm hover:text-fg"
              >
                <span className="shrink-0">Last ran</span>
                <span className="truncate text-fg">{lastRun.title || "Untitled thread"}</span>
                <span className="shrink-0 tabular">· {relativeAge(lastRun.updatedAt)}</span>
              </button>
            )
          }
        >
          Nothing is running
        </Calm>
      )}
    </Card>
  );
}

/** From this long a read is slow enough to say it is still going. */
const SLOW_READ_MS = 8_000;

/** Whether `waiting` has held for `SLOW_READ_MS`; it resets when the wait ends. */
function useLongWait(waiting: boolean): boolean {
  const [long, setLong] = useState(false);
  useEffect(() => {
    setLong(false);
    if (!waiting) return;
    const timer = setTimeout(() => setLong(true), SLOW_READ_MS);
    return () => clearTimeout(timer);
  }, [waiting]);
  return long;
}

/** What a pull request is, as the sidebar's rows mark it. */
function PullMark({ pull }: { pull: PullRequest }) {
  const state = pullState(pull);
  const Icon =
    state === "merged"
      ? GitMerge
      : state === "closed"
        ? GitPullRequestClosed
        : state === "draft"
          ? GitPullRequestDraft
          : GitPullRequestArrow;
  return (
    <span
      role="img"
      aria-label={`Pull request #${pull.number}: ${state}`}
      className={cx(
        "flex shrink-0 items-center",
        state === "merged"
          ? "text-accent"
          : state === "closed"
            ? "text-danger"
            : state === "draft"
              ? "text-fg-muted"
              : "text-success",
      )}
    >
      <Icon size={12} />
    </span>
  );
}

function PullsCard({ project }: { project: string }) {
  const available = useApp((s) => !!s.agent?.pullRequests);
  useSyncExternalStore(subscribePulls, pullsVersion);
  useEffect(() => {
    if (!available || !project) return;
    const t = setTimeout(() => loadPulls(project), 400);
    return () => clearTimeout(t);
  }, [available, project]);
  const pulls = available ? cachedPulls(project) : undefined;
  const error = available && pulls === undefined ? pullsError(project) : undefined;
  const slow = useLongWait(available && !!project && pulls === undefined && !error);
  const recent = useMemo(
    () => [...(pulls ?? [])].sort((one, other) => (other.updatedAt ?? 0) - (one.updatedAt ?? 0)),
    [pulls],
  );
  const twoLines = useTwoLines();
  const shown = recent.slice(0, useShownRows(twoLines ? TWO_LINE_ROW_HEIGHT : ROW_HEIGHT));
  const more = recent.length - shown.length;
  return (
    <Card
      label="Pull requests"
      icon={GitPullRequest}
      aside={
        more > 0 ? (
          <LabelNote title="Open the pull requests panel" onClick={() => setPanel("pulls")}>
            +{more}
          </LabelNote>
        ) : undefined
      }
    >
      {!project ? (
        <Nothing>Choose a folder to see its pull requests.</Nothing>
      ) : !available ? (
        <div className="flex flex-1 flex-col justify-center gap-2">
          <p className="text-caption text-fg-muted">Connect GitHub to see this project's pull requests.</p>
          <Pill quiet size="sm" className="self-start" onClick={() => openSettings("connections")}>
            Connect GitHub
          </Pill>
        </div>
      ) : error ? (
        <div className="flex flex-1 flex-col justify-center gap-2">
          <p className="text-caption text-fg">{pullErrorTitle(error.code)}</p>
          <p className="line-clamp-2 text-caption text-fg-muted">{error.message}</p>
          <Pill quiet size="sm" className="self-start" onClick={() => retryPulls(project)}>
            Try again
          </Pill>
        </div>
      ) : pulls === undefined ? (
        <div className="flex min-h-0 flex-1 flex-col">
          <RowsSkeleton mark rowHeight={twoLines ? TWO_LINE_ROW_HEIGHT : ROW_HEIGHT} />
          {slow && (
            <p className="shrink-0 pt-1 text-caption text-fg-muted">
              Still asking GitHub for this project's pull requests…
            </p>
          )}
        </div>
      ) : shown.length ? (
        <CardList>
          {shown.map((pull) => (
            <CardRow
              key={pull.number}
              title={pull.url ? `Open #${pull.number} on GitHub` : `#${pull.number} ${pull.title}`}
              onClick={() => (pull.url ? openExternal(pull.url) : setPanel("pulls"))}
              detail={
                twoLines
                  ? [pull.headRef, pull.author, pull.updatedAt && relativeAge(pull.updatedAt)]
                      .filter(Boolean)
                      .join(" · ")
                  : undefined
              }
              mark={<PullMark pull={pull} />}
            >
              <span className="min-w-0 flex-1 truncate text-fg">{pull.title}</span>
              <span className="shrink-0 text-caption text-fg-muted tabular">#{pull.number}</span>
            </CardRow>
          ))}
        </CardList>
      ) : (
        <Nothing>No pull requests in this project.</Nothing>
      )}
    </Card>
  );
}

/**
 * The cards under a new thread's composer. One day-by-day usage read covers the heatmap and the
 * token line; two reads by model price the month's spend and the one before it, a harness's
 * reported cost or the list-price estimate (`state/pricing.ts`).
 */
export function DashboardBody({ project }: { project: string }) {
  const since = useMemo(() => daysAgo(new Date(), HEATMAP_WEEKS * 7 - 1), []);
  const usageSince = useMemo(() => daysAgo(new Date(), USAGE_DAYS - 1), []);
  const { since: monthSince, previousSince } = useMemo(() => monthBounds(new Date()), []);
  const offset = utcOffsetMinutes();
  const days = useEngineData(Method.dataUsage, parseUsageReport, {
    groupBy: "day",
    since,
    utcOffsetMinutes: offset,
  });
  const rows = days.data?.rows;
  const month = useEngineData(Method.dataUsage, parseUsageReport, {
    groupBy: "model",
    since: monthSince,
    utcOffsetMinutes: offset,
  });
  const twoMonths = useEngineData(Method.dataUsage, parseUsageReport, {
    groupBy: "model",
    since: previousSince,
    utcOffsetMinutes: offset,
  });
  const render = (id: WidgetId) => {
    switch (id) {
      case "activity":
        return <ActivityCard rows={rows} since={since} />;
      case "threads":
        return <RecentThreadsCard project={project} />;
      case "spend":
        return (
          <SpendCard
            rows={rows}
            month={month.data?.rows}
            twoMonths={twoMonths.data?.rows}
            usageSince={usageSince}
          />
        );
      case "needs":
        return <NeedsYouCard />;
      case "pulls":
        return <PullsCard project={project} />;
      case "running":
        return <RunningCard />;
      case "today":
        return <TodayCard rows={rows} />;
      case "models":
        return <ModelsCard month={month.data?.rows} />;
      case "harnesses":
        return <HarnessesCard month={month.data?.rows} />;
      case "rhythm":
        return <RhythmCard rows={rows} />;
    }
  };
  return (
    <>
      <DashboardFigures rows={rows} month={month.data?.rows} twoMonths={twoMonths.data?.rows} />
      <div className="rc-overview-in min-h-0 w-full flex-1 overflow-y-auto">
        <WidgetGrid render={render} fill />
      </div>
    </>
  );
}
