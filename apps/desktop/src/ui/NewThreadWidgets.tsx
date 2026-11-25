/**
 * The cards under the composer on a new thread: what this computer recorded lately (the activity
 * heatmap and the month's spend), the project's recent threads and pull requests, what waits on
 * the user and what is running. The user picks, orders and sizes them (`WidgetGrid.tsx`).
 *
 * Every card draws from a read the app already makes: one `_rustcode/data/usage` report grouped
 * by day, two by model to price the spend, the session list the sidebar shows, the inbox, and the
 * project's pulls cache (`state/pulls.ts`). No new engine call.
 *
 * Every card is one height, so two rows fit under the composer in a laptop window. Lists show
 * three rows and say how many more there are at the end of their label row.
 */

import { type ReactNode, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import type { PullRequest, UsageRow } from "../acp/rustcode";
import { parseUsageReport } from "../acp/rustcode";
import { Method } from "../acp/types";
import { openExternal } from "../lib/native";
import { count } from "../lib/plural";
import { useInboxItems } from "../state/inboxRead";
import { daysAgo, previousRows, sparklinePath, sparklineSeries, sumRows, trend } from "../state/period";
import { costLine, costOfRows, formatBestCost } from "../state/pricing";
import { cachedPulls, loadPulls, pullState, pullsVersion, subscribePulls } from "../state/pulls";
import { openSettings, selectThread, setPanel, setView, useApp } from "../state/store";
import { formatTokens, relativeAge } from "../state/thread";
import { isRunning } from "../state/turn";
import type { WidgetId } from "../state/widgetLayout";
import {
  HEATMAP_WEEKS,
  type HeatCell,
  heatmapWeeks,
  monthBounds,
  projectThreads,
  type Streak,
  streakOf,
  turnsPerDay,
} from "../state/widgets";
import {
  Activity,
  Flame,
  Gauge,
  GitMerge,
  GitPullRequest,
  GitPullRequestArrow,
  GitPullRequestClosed,
  GitPullRequestDraft,
  Hand,
  History,
  type LucideIcon,
  Play,
} from "./icons";
import { cx, Pill, Skeleton, Tooltip } from "./primitives";
import { TrendPill } from "./StatisticsCharts";
import { useEngineData } from "./settingsKit";
import { useWidgetEditClass, useWidgetId, useWidgetSpan, WidgetChrome, WidgetGrid } from "./WidgetGrid";

/** How many rows a list card shows before its label says how many are left. */
const SHOWN_ROWS = 3;

/** The spend card's token line: the last thirty days, counted as whole UTC days. */
const USAGE_DAYS = 30;

/** A heatmap cell's side and the space between cells, in pixels: squares, as a contribution graph draws them. */
const HEAT_CELL = 8;
const HEAT_GAP = 2;
/** The fewest weeks the heatmap draws, however narrow its card. */
const MIN_WEEKS = 8;

/** Short columns leave the overview out to keep space for the composer. */
export const NEW_THREAD_WIDGETS_MIN_HEIGHT = 700;

/** A day's shade in the heatmap, as the design system's own fills and accent. */
const SHADE: Record<HeatCell["level"], string> = {
  0: "bg-fill-subtle",
  1: "bg-accent/25",
  2: "bg-accent/45",
  3: "bg-accent/70",
  4: "bg-accent",
};

/**
 * One card: a small icon and label, then whatever the card has to say. It is see-through glass
 * with a hairline edge drawn inside it, so the overview's scroll box never clips the edge. The
 * surface is the stable part: the overview's open and close fade its fill and move what is
 * inside it, never the blur (index.css, `rc-overview`).
 */
function Card({
  label,
  icon: Icon,
  aside,
  children,
}: {
  label: string;
  icon: LucideIcon;
  /** Something small at the end of the card's label row. */
  aside?: ReactNode;
  children: ReactNode;
}) {
  const span = useWidgetSpan();
  const editClass = useWidgetEditClass();
  return (
    <section
      aria-label={label}
      data-widget={useWidgetId()}
      className={cx(
        "rc-glass-faint rc-overview-card flex h-[128px] w-[240px] shrink-0 snap-start flex-col overflow-hidden rounded-xl p-3 ring-inset lg:w-auto",
        span,
        editClass ?? "ring-1 ring-border-strong",
      )}
    >
      <div className="rc-overview-body flex min-h-0 flex-1 flex-col gap-2">
        <div className="flex h-5 shrink-0 items-center justify-between gap-2">
          <h3 className="flex min-w-0 items-center gap-1.5 text-caption font-medium text-fg-muted">
            <Icon size={12} className="shrink-0" />
            <span className="truncate">{label}</span>
          </h3>
          <WidgetChrome fallback={aside} />
        </div>
        {children}
      </div>
    </section>
  );
}

/** A card's one quiet line when it has nothing else to draw. */
function Nothing({ children }: { children: ReactNode }) {
  return (
    <div className="flex flex-1 items-center">
      <p className="text-caption text-fg-muted">{children}</p>
    </div>
  );
}

/** A small count at the end of a label row, or a link where it opens what it counts. */
function LabelNote({
  children,
  onClick,
  title,
}: {
  children: ReactNode;
  onClick?: () => void;
  title: string;
}) {
  const className = "shrink-0 rounded-sm text-caption text-fg-muted tabular";
  return (
    <Tooltip label={title}>
      {onClick ? (
        <button type="button" onClick={onClick} className={cx(className, "hover:text-fg")}>
          {children}
        </button>
      ) : (
        <span className={className}>{children}</span>
      )}
    </Tooltip>
  );
}

/** A row in a card's list: a control, so the card is walkable with the keyboard. */
function CardRow({ onClick, title, children }: { onClick: () => void; title: string; children: ReactNode }) {
  return (
    <Tooltip label={title}>
      <button
        type="button"
        onClick={onClick}
        className="-mx-1 flex h-6 min-w-0 shrink-0 items-center gap-2 rounded-sm px-1 text-left text-callout hover:bg-fill-subtle"
      >
        {children}
      </button>
    </Tooltip>
  );
}

function CardList({ children }: { children: ReactNode }) {
  return <div className="-mt-1 flex min-h-0 flex-1 flex-col">{children}</div>;
}

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
function useFittedWeeks() {
  const [element, setElement] = useState<HTMLDivElement | null>(null);
  const [weeks, setWeeks] = useState(MIN_WEEKS);
  useEffect(() => {
    if (!element) return;
    const fit = () =>
      setWeeks(
        Math.max(
          MIN_WEEKS,
          Math.min(HEATMAP_WEEKS, Math.floor((element.clientWidth + HEAT_GAP) / (HEAT_CELL + HEAT_GAP))),
        ),
      );
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(element);
    return () => observer.disconnect();
  }, [element]);
  return [setElement, weeks] as const;
}

function ActivityCard({ rows, since }: { rows?: UsageRow[]; since: string }) {
  const sessions = useApp((s) => s.sessions);
  const [box, weeks] = useFittedWeeks();
  const turns = useMemo(() => turnsPerDay(rows ?? []), [rows]);
  const columns = useMemo(() => heatmapWeeks(turns, weeks), [turns, weeks]);
  const streak = useMemo(() => streakOf(turns), [turns]);
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
        <Skeleton className="h-full w-full flex-1" />
      ) : turns.size === 0 ? (
        <Nothing>No turns recorded yet.</Nothing>
      ) : (
        <div className="flex min-h-0 flex-1 items-end gap-4">
          <div className="flex shrink-0 flex-col">
            <span className="font-semibold text-title3 text-fg tabular">{shownTurns.toLocaleString()}</span>
            <span className="text-caption text-fg-muted">turns in {weeks} weeks</span>
            <span className="text-caption text-fg-muted">{count(activeThreads, "thread")} active</span>
          </div>
          <div
            ref={box}
            role="img"
            aria-label={`Turns per day over the last ${weeks} weeks: ${count(shownTurns, "turn")}.`}
            className="flex min-w-0 flex-1 justify-end overflow-hidden"
            style={{ gap: HEAT_GAP }}
          >
            {columns.map((week, index) => (
              <div
                key={week.find(Boolean)?.day ?? index}
                className="flex shrink-0 flex-col"
                style={{ gap: HEAT_GAP }}
              >
                {week.map((cell, day) => (
                  <span
                    key={cell?.day ?? `future-${index}-${day}`}
                    title={cell ? `${cell.day} · ${count(cell.turns, "turn")}` : undefined}
                    className={cx("rc-heat-cell rounded-xs", cell && SHADE[cell.level])}
                    style={{ width: HEAT_CELL, height: HEAT_CELL }}
                  />
                ))}
              </div>
            ))}
          </div>
        </div>
      )}
    </Card>
  );
}

function RecentThreadsCard({ project }: { project: string }) {
  const sessions = useApp((s) => s.sessions);
  const threads = useMemo(() => projectThreads(sessions, project), [sessions, project]);
  const shown = threads.slice(0, SHOWN_ROWS);
  const more = threads.length - shown.length;
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
      {shown.length ? (
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
        <div className="flex flex-1 flex-col justify-end gap-2">
          <Skeleton className="h-5 w-[50%]" />
          <Skeleton className="h-3 w-[70%]" />
        </div>
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
            className="mt-1 h-5 w-full text-accent/60"
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
  const shown = threads.slice(0, SHOWN_ROWS);
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
      {shown.length ? (
        <CardList>
          {shown.map((item) => (
            <CardRow
              key={item.key}
              onClick={() => selectThread(item.threadId)}
              title={`${item.title}: ${item.detail}`}
            >
              <span className="size-1.5 shrink-0 rounded-full bg-warning" />
              <span className="min-w-0 flex-1 truncate text-fg">{item.title}</span>
            </CardRow>
          ))}
        </CardList>
      ) : (
        <Nothing>Nothing is waiting on you.</Nothing>
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
  const shown = threads.slice(0, SHOWN_ROWS);
  const more = threads.length - shown.length;
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
      {shown.length ? (
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
        <Nothing>Nothing is running.</Nothing>
      )}
    </Card>
  );
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
  const recent = useMemo(
    () => [...(pulls ?? [])].sort((one, other) => (other.updatedAt ?? 0) - (one.updatedAt ?? 0)),
    [pulls],
  );
  const shown = recent.slice(0, SHOWN_ROWS);
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
      ) : pulls === undefined ? (
        <div className="flex flex-1 flex-col justify-center gap-2">
          {[0, 1, 2].map((row) => (
            <Skeleton key={row} className="h-3 w-[75%]" />
          ))}
        </div>
      ) : shown.length ? (
        <CardList>
          {shown.map((pull) => (
            <CardRow
              key={pull.number}
              title={pull.url ? `Open #${pull.number} on GitHub` : `#${pull.number} ${pull.title}`}
              onClick={() => (pull.url ? openExternal(pull.url) : setPanel("pulls"))}
            >
              <PullMark pull={pull} />
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
export function NewThreadWidgets({ project }: { project: string }) {
  const since = useMemo(() => daysAgo(new Date(), HEATMAP_WEEKS * 7 - 1), []);
  const usageSince = useMemo(() => daysAgo(new Date(), USAGE_DAYS - 1), []);
  const { since: monthSince, previousSince } = useMemo(() => monthBounds(new Date()), []);
  const days = useEngineData(Method.dataUsage, parseUsageReport, { groupBy: "day", since });
  const rows = days.data?.rows;
  const month = useEngineData(Method.dataUsage, parseUsageReport, { groupBy: "model", since: monthSince });
  const twoMonths = useEngineData(Method.dataUsage, parseUsageReport, {
    groupBy: "model",
    since: previousSince,
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
    }
  };
  return <WidgetGrid render={render} />;
}
