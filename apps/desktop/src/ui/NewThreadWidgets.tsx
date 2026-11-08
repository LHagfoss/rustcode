// The cards under the composer on a new thread: what this computer recorded lately
// (the activity heatmap, this month's turns and spend), the project's recent
// threads and pull requests, and the last thirty days of reported usage. Devin's
// home screen is the shape: a quiet dashboard of see-through glass cards under
// the field, wider than the field itself, so the picture behind them shows.
//
// Every card draws from a read the app already makes: one `_rustcode/data/usage`
// report grouped by day (the Activity, Usage and This month cards all slice it),
// the session list the sidebar shows, and the project's pulls cache
// (`state/pulls.ts`, the one the sidebar's rows read). No new engine call.
//
// The optional overview sits outside the composer's centring stack. Cards keep
// a consistent height while reads land; empty cards explain what is missing.

import {
  Activity,
  CalendarDays,
  Gauge,
  GitMerge,
  GitPullRequest,
  GitPullRequestArrow,
  GitPullRequestClosed,
  GitPullRequestDraft,
  History,
  type LucideIcon,
} from "lucide-react";
import { type ReactNode, useEffect, useMemo, useSyncExternalStore } from "react";
import type { PullRequest, UsageRow } from "../acp/rustcode";
import { parseUsageReport } from "../acp/rustcode";
import { Method } from "../acp/types";
import { openExternal } from "../lib/native";
import { count } from "../lib/plural";
import { daysAgo, sumRows, trend } from "../state/period";
import { formatCost } from "../state/pricing";
import { cachedPulls, loadPulls, pullState, pullsVersion, subscribePulls } from "../state/pulls";
import { openSettings, selectThread, setPanel, useApp } from "../state/store";
import { formatTokens, relativeAge } from "../state/thread";
import {
  HEATMAP_WEEKS,
  type HeatCell,
  heatmapWeeks,
  monthBounds,
  projectThreads,
  turnsPerDay,
} from "../state/widgets";
import { cx, Pill, Skeleton } from "./primitives";
import { TrendPill } from "./StatisticsCharts";
import { useEngineData } from "./settingsKit";

/** How many of each list a card shows before it says how many are left. */
const SHOWN_THREADS = 3;
const SHOWN_PULLS = 3;

/** The usage card's window: the last thirty days, counted as whole UTC days. */
const USAGE_DAYS = 30;

/** Short columns leave the overview out to keep space for the composer. */
export const NEW_THREAD_WIDGETS_MIN_HEIGHT = 700;

/** A day's shade in the heatmap, as the design system's own fills and accent. */
const SHADE: Record<HeatCell["level"], string> = {
  0: "bg-fill-subtle",
  1: "bg-accent/20",
  2: "bg-accent/35",
  3: "bg-accent/55",
  4: "bg-accent/80",
};

/** One card: a small icon and label, then whatever the card has to say. */
function Card({
  label,
  icon: Icon,
  span,
  children,
}: {
  label: string;
  icon: LucideIcon;
  /** How wide the card is in the dashboard's six columns (from `lg`). */
  span: string;
  children: ReactNode;
}) {
  return (
    <section
      aria-label={label}
      // One height for every card and state. It is see-through: a third of
      // the surface and the glass filter, no edge ring, so the picture runs on behind it.
      className={cx(
        "rc-glass-faint flex min-h-[132px] w-[240px] shrink-0 snap-start flex-col gap-2 rounded-xl p-3 lg:w-auto",
        span,
      )}
    >
      <h3 className="flex shrink-0 items-center gap-1.5 text-caption font-medium text-fg-muted">
        <Icon size={12} className="shrink-0" />
        {label}
      </h3>
      {children}
    </section>
  );
}

/** A card's one quiet line when it has nothing else to draw. */
function Nothing({ children }: { children: ReactNode }) {
  return <p className="text-caption text-fg-muted">{children}</p>;
}

/** A row in a card's list: a control, so the card is walkable with the keyboard. */
function CardRow({ onClick, title, children }: { onClick: () => void; title: string; children: ReactNode }) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      className="-mx-1 flex h-6 min-w-0 shrink-0 items-center gap-1.5 rounded-sm px-1 text-left text-callout hover:bg-fill-subtle"
    >
      {children}
    </button>
  );
}

function ActivityCard({ rows, since }: { rows?: UsageRow[]; since: string }) {
  const sessions = useApp((s) => s.sessions);
  const turns = useMemo(() => turnsPerDay(rows ?? []), [rows]);
  const columns = useMemo(() => heatmapWeeks(turns, HEATMAP_WEEKS), [turns]);
  const totalTurns = useMemo(() => [...turns.values()].reduce((sum, n) => sum + n, 0), [turns]);
  const activeThreads = useMemo(() => {
    const from = Date.parse(`${since}T00:00:00Z`);
    return sessions.filter((session) => (session.updatedAt ?? 0) >= from).length;
  }, [sessions, since]);
  return (
    <Card label="Activity" icon={Activity} span="lg:col-span-3">
      {rows === undefined ? (
        <Skeleton className="h-[62px] w-full flex-1" />
      ) : totalTurns === 0 ? (
        <div className="flex flex-1 items-center">
          <Nothing>No turns recorded yet.</Nothing>
        </div>
      ) : (
        <div
          role="img"
          aria-label={`Turns per day over the last ${HEATMAP_WEEKS} weeks: ${count(totalTurns, "turn")}.`}
          className="flex flex-1 items-end gap-[3px]"
        >
          {columns.map((week, index) => (
            <div key={week.find(Boolean)?.day ?? index} className="flex min-w-0 flex-1 flex-col gap-[3px]">
              {week.map((cell, day) =>
                cell ? (
                  <span
                    key={cell.day}
                    title={`${cell.day} · ${count(cell.turns, "turn")}`}
                    className={cx("rc-heat-cell h-[7px] w-full", SHADE[cell.level])}
                  />
                ) : (
                  <span key={`future-${index}-${day}`} className="h-[7px] w-full" />
                ),
              )}
            </div>
          ))}
        </div>
      )}
      <p className="shrink-0 truncate text-caption text-fg-muted" title={`Last ${HEATMAP_WEEKS} weeks`}>
        {rows === undefined
          ? "Reading what this computer recorded…"
          : `${count(totalTurns, "turn")} · ${count(activeThreads, "thread")}`}
      </p>
    </Card>
  );
}

function RecentThreadsCard({ project }: { project: string }) {
  const sessions = useApp((s) => s.sessions);
  const threads = useMemo(() => projectThreads(sessions, project), [sessions, project]);
  const shown = threads.slice(0, SHOWN_THREADS);
  const more = threads.length - shown.length;
  return (
    <Card label="Recent threads" icon={History} span="lg:col-span-3">
      {shown.length ? (
        <div className="flex flex-1 flex-col gap-0.5">
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
        </div>
      ) : (
        <Nothing>No threads in this project yet.</Nothing>
      )}
      <span className="mt-auto shrink-0 text-caption text-fg-muted">
        {more > 0 ? `${count(more, "more thread", "more threads")} in the sidebar` : "Open in the sidebar"}
      </span>
    </Card>
  );
}

function UsageCard({ rows, since }: { rows?: UsageRow[]; since: string }) {
  const totals = useMemo(
    () => sumRows((rows ?? []).filter((row) => (row.day ?? "") >= since)),
    [rows, since],
  );
  const tokens = (totals.input ?? 0) + (totals.output ?? 0);
  return (
    <Card label="Usage" icon={Gauge} span="lg:col-span-2">
      {rows === undefined ? (
        <div className="flex flex-1 flex-col justify-end gap-1.5">
          <Skeleton className="h-4 w-[60%]" />
          <Skeleton className="h-3 w-[40%]" />
        </div>
      ) : totals.turns === 0 ? (
        <div className="flex flex-1 items-center">
          <Nothing>No usage recorded in {USAGE_DAYS} days.</Nothing>
        </div>
      ) : (
        <div className="flex flex-1 flex-col justify-end gap-0.5">
          <span
            className="truncate font-semibold text-title3 text-fg tabular"
            title={`${tokens.toLocaleString()} input and output tokens`}
          >
            {formatTokens(tokens)}
            <span className="pl-1 font-normal text-caption text-fg-muted">tokens</span>
          </span>
          <span className="truncate text-callout text-fg-muted tabular">
            {totals.cost === undefined ? "spend not reported" : formatCost(totals.cost)}
          </span>
        </div>
      )}
      <span className="shrink-0 text-caption text-fg-muted">
        {rows === undefined ? "Last 30 days" : `${count(totals.turns, "turn")} in ${USAGE_DAYS} days`}
      </span>
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
  // The cache's version: a project's read landing re-renders the rows that read it.
  useSyncExternalStore(subscribePulls, pullsVersion);
  useEffect(() => {
    if (!available || !project) return;
    // `gh` is slow and a home screen is not worth waiting for: read once, a moment after it settles.
    const t = setTimeout(() => loadPulls(project), 400);
    return () => clearTimeout(t);
  }, [available, project]);
  const pulls = available ? cachedPulls(project) : undefined;
  const recent = useMemo(
    () =>
      [...(pulls ?? [])]
        .sort((one, other) => (other.updatedAt ?? 0) - (one.updatedAt ?? 0))
        .slice(0, SHOWN_PULLS),
    [pulls],
  );
  return (
    <Card label="Pull requests" icon={GitPullRequest} span="lg:col-span-2">
      {!project ? (
        <div className="flex flex-1 items-center">
          <Nothing>Choose a folder to see its pull requests.</Nothing>
        </div>
      ) : !available ? (
        <div className="flex flex-1 flex-col gap-2">
          <Nothing>Connect GitHub to see this project's pull requests.</Nothing>
          <Pill quiet size="sm" className="self-start" onClick={() => openSettings("connections")}>
            Connections…
          </Pill>
        </div>
      ) : pulls === undefined ? (
        <div className="flex flex-1 flex-col gap-2">
          {[0, 1, 2].map((row) => (
            <Skeleton key={row} className="h-3 w-[75%]" />
          ))}
        </div>
      ) : recent.length ? (
        <div className="flex flex-1 flex-col gap-0.5">
          {recent.map((pull) => (
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
        </div>
      ) : (
        <Nothing>No pull requests in this project.</Nothing>
      )}
      <span className="mt-auto shrink-0 text-caption text-fg-muted">
        {recent.length
          ? "Opens on GitHub"
          : !project
            ? "No folder chosen"
            : available
              ? "Read from this project's branches"
              : "Settings › Connections"}
      </span>
    </Card>
  );
}

function Metric({
  label,
  value,
  title,
  change,
}: {
  label: string;
  value: string;
  title?: string;
  change?: ReturnType<typeof trend>;
}) {
  return (
    <div className="flex items-center justify-between gap-2">
      <span className="shrink-0 text-callout text-fg-muted">{label}</span>
      <span className="flex min-w-0 items-center gap-1.5">
        <span className="truncate font-semibold text-body text-fg tabular" title={title}>
          {value}
        </span>
        {change && <TrendPill trend={change} compare="against the month before" />}
      </span>
    </div>
  );
}

function ThisMonthCard({ rows }: { rows?: UsageRow[] }) {
  const now = useMemo(() => new Date(), []);
  const { since, previousSince } = useMemo(() => monthBounds(now), [now]);
  const { thisMonth, lastMonth } = useMemo(() => {
    const of = (rows ?? []).filter((row): row is UsageRow & { day: string } => !!row.day);
    return {
      thisMonth: sumRows(of.filter((row) => row.day >= since)),
      lastMonth: sumRows(of.filter((row) => row.day >= previousSince && row.day < since)),
    };
  }, [rows, since, previousSince]);
  const empty = thisMonth.turns === 0 && lastMonth.turns === 0;
  return (
    <Card label="This month" icon={CalendarDays} span="lg:col-span-2">
      {rows === undefined ? (
        <div className="flex flex-1 flex-col justify-center gap-2">
          <Skeleton className="h-3 w-full" />
          <Skeleton className="h-3 w-[70%]" />
        </div>
      ) : empty ? (
        <div className="flex flex-1 items-center">
          <Nothing>Nothing recorded this month.</Nothing>
        </div>
      ) : (
        <div className="flex flex-1 flex-col justify-center gap-1.5">
          <Metric
            label="Turns"
            value={String(thisMonth.turns)}
            change={trend(thisMonth.turns, lastMonth.turns)}
          />
          <Metric
            label="Spend"
            value={formatCost(thisMonth.cost)}
            title={thisMonth.cost === undefined ? "No harness reported a cost this month" : undefined}
            change={
              thisMonth.cost === undefined || lastMonth.cost === undefined
                ? undefined
                : trend(thisMonth.cost, lastMonth.cost)
            }
          />
        </div>
      )}
      <span className="shrink-0 text-caption text-fg-muted">Compared with last month</span>
    </Card>
  );
}

/**
 * The cards under a new thread's composer. One day-by-day usage read covers the
 * activity heatmap, the usage card and this month's figures; the thread and pull
 * lists come from what the sidebar already read.
 */
export function NewThreadWidgets({ project }: { project: string }) {
  const since = useMemo(() => daysAgo(new Date(), HEATMAP_WEEKS * 7 - 1), []);
  const usageSince = useMemo(() => daysAgo(new Date(), USAGE_DAYS - 1), []);
  const days = useEngineData(Method.dataUsage, parseUsageReport, { groupBy: "day", since });
  const rows = days.data?.rows;
  return (
    <div
      // Below `lg` the cards are one row that scrolls sideways: five columns of a 240px card in a
      // narrow window would squeeze the labels to nothing. From `lg` they are a dashboard two rows
      // tall: the activity heatmap and the recent threads across the top (three columns each), then
      // usage, pull requests and this month in three.
      className="flex snap-x gap-2 overflow-x-auto pb-0.5 lg:grid lg:grid-cols-6 lg:overflow-visible lg:pb-0"
      role="group"
      aria-label="Activity, usage and recent work"
    >
      <ActivityCard rows={rows} since={since} />
      <RecentThreadsCard project={project} />
      <UsageCard rows={rows} since={usageSince} />
      <PullsCard project={project} />
      <ThisMonthCard rows={rows} />
    </div>
  );
}
