// Settings › Statistics: what the local data store recorded of every turn, by
// harness and model and by thread (`_rustcode/data/usage`, docs/data-store.md).
// A thread's total includes the threads it started, so a parent shows the work
// it delegated without the same tokens being counted twice. A count a harness
// never reported stays blank. A cost is the USD a harness reported or, where it
// reported none, the same tokens at public list prices (`state/pricing.ts`);
// a reported number always wins and is never altered, and each row contributes
// either its reported cost or its estimate, never both. A row with neither is
// `unknown`, never $0. The total keeps the reported and estimated parts beside
// it, and ≈ marks a number that is partly or wholly estimated.

import { ChevronRight, RefreshCw } from "lucide-react";
import { Fragment, type ReactNode, useMemo, useState } from "react";
import { parseUsageReport, type UsageRow, type UsageTotals } from "../acp/rustcode";
import { type Harness, Method } from "../acp/types";
import {
  bestCost,
  type CostInput,
  type CostTotals,
  costLine,
  costOf,
  formatCost,
  PRICE_DATE,
  priceOf,
  rollUpCosts,
} from "../state/pricing";
import {
  type HarnessTotal,
  harnessTotals,
  type ThreadReport,
  type ThreadTotal,
  threadReport,
  usedTokens,
} from "../state/statistics";
import { useApp } from "../state/store";
import { formatTokens as compactTokens } from "../state/thread";
import { HarnessIcon } from "./HarnessIcon";
import { cx, Spinner } from "./primitives";
import { Action, Group, Loading, Row, useEngineData } from "./settingsKit";

/** How many top-level threads the page draws before Show all. */
const THREAD_LIMIT = 10;

/** The daily chart's window, today and the 29 days before it. */
const DAY_WINDOW = 30;

/** The cost table's token columns that step out when its card is narrow. */
const NARROW_COLUMN = "@max-[36rem]/usage:hidden";

const DAY_MS = 86_400_000;

/** `days` UTC days before today, as the store's `date(started_at)` writes it. */
function utcDaysAgo(days: number): string {
  const today = Date.parse(`${new Date().toISOString().slice(0, 10)}T00:00:00Z`);
  return new Date(today - days * DAY_MS).toISOString().slice(0, 10);
}

export function StatisticsPage() {
  const since = utcDaysAgo(DAY_WINDOW - 1);
  const models = useEngineData(Method.dataUsage, parseUsageReport, { groupBy: "model" });
  const threads = useEngineData(Method.dataUsage, parseUsageReport, { groupBy: "thread" });
  // The window is asked for, not sliced from the store's 1,000-row cap: without
  // `since`, a store with more than 1,000 days could lose the recent ones.
  const days = useEngineData(Method.dataUsage, parseUsageReport, { groupBy: "day", since });
  const sessions = useApp((s) => s.sessions);
  const sessionsCapped = useApp((s) => s.sessionsCapped);
  const links = useApp((s) => s.links.links);
  const harnesses = useApp((s) => s.harnesses?.harnesses);
  const [refreshing, setRefreshing] = useState(false);
  const [all, setAll] = useState(false);

  const groups = useMemo(() => harnessTotals(models.data?.rows ?? []), [models.data?.rows]);
  const costRows = useMemo(
    () =>
      groups.flatMap((group) =>
        group.models.map((model) => ({
          harness: group.harness,
          model: model.model,
          totals: model.totals,
        })),
      ),
    [groups],
  );
  const cost = useMemo(() => rollUpCosts(costRows), [costRows]);
  const costsByHarness = useMemo(() => {
    const map = new Map<string, CostTotals>();
    for (const group of groups) {
      map.set(
        group.harness,
        rollUpCosts(
          group.models.map((model) => ({
            harness: group.harness,
            model: model.model,
            totals: model.totals,
          })),
        ),
      );
    }
    return map;
  }, [groups]);
  const rolled = useMemo(
    () => (threads.data ? threadReport(threads.data, sessions, links, sessionsCapped) : null),
    [threads.data, sessions, links, sessionsCapped],
  );
  const columns = useMemo(() => lastDays(days.data?.rows ?? [], since), [days.data?.rows, since]);
  const names = useMemo(() => harnessNames(harnesses), [harnesses]);

  if (!models.data || !threads.data || !rolled) return <Loading error={models.error ?? threads.error} />;

  const totals = models.data.totals;
  const error = models.error ?? threads.error;
  if (!groups.length && !rolled.threads.length && !error) return <NothingRecorded />;

  const refresh = async () => {
    setRefreshing(true);
    try {
      await Promise.all([models.reload(), threads.reload(), days.reload()]);
    } finally {
      setRefreshing(false);
    }
  };

  const turns = columns.reduce((sum, column) => sum + column.turns, 0);

  return (
    <>
      <Group
        title="Recorded usage"
        action={
          <Action onClick={() => void refresh()} disabled={refreshing}>
            {refreshing ? <Spinner size={11} /> : <RefreshCw size={12} />} Refresh
          </Action>
        }
        footer={
          <UsageFootnote
            totals={totals}
            cost={cost}
            truncated={models.data.truncated}
            error={error}
            dayError={days.error}
          />
        }
      >
        <div className="grid grid-cols-2 gap-x-6 gap-y-4 px-4 py-4">
          <Stat label="Cost" value={costValue(cost)} exact={costExact(cost)} note={costNote(cost)} />
          <Stat
            label="Turns"
            value={String(totals.turns)}
            note={
              totals.turnsWithoutAnyTokens > 0
                ? `${totals.turnsWithoutAnyTokens} with missing counts`
                : undefined
            }
          />
        </div>
        <div className="grid grid-cols-2 gap-x-6 gap-y-3 px-4 py-3 sm:grid-cols-4">
          <Stat secondary label="Input" value={tokensLabel(totals.input)} exact={totals.input} />
          <Stat secondary label="Output" value={tokensLabel(totals.output)} exact={totals.output} />
          <Stat secondary label="Cached" value={tokensLabel(totals.cached)} exact={totals.cached} />
          <Stat
            secondary
            label="Cache write"
            value={tokensLabel(totals.cacheWrite)}
            exact={totals.cacheWrite}
          />
        </div>
      </Group>

      {groups.length > 0 && (
        <Group
          title="Cost by model"
          footer={
            <span className="block">
              Bars compare each model's best known cost, relative to the costliest; ≈ marks an estimate at API
              list prices on {PRICE_DATE}, and a row with neither number has no bar.
            </span>
          }
        >
          <ModelCosts rows={modelCosts(groups)} names={names} />
        </Group>
      )}

      {turns > 0 && (
        <Group
          title="Last 30 days"
          footer={
            days.data?.truncated ? (
              <span className="block text-warning">
                The day read stopped at its row cap; some days may be missing.
              </span>
            ) : undefined
          }
        >
          <TurnsChart columns={columns} />
        </Group>
      )}

      {groups.length > 0 && (
        <Group title="By harness and model" footer={<UsageTableFootnote cost={cost} />}>
          <UsageTable groups={groups} names={names} costs={costsByHarness} />
        </Group>
      )}

      {rolled.threads.length > 0 && (
        <Group
          title="Threads"
          action={
            rolled.threads.length > THREAD_LIMIT ? (
              <Action onClick={() => setAll(!all)}>{all ? "Show fewer" : "Show all"}</Action>
            ) : undefined
          }
          footer={<ThreadFootnote report={rolled} />}
        >
          {(all ? rolled.threads : rolled.threads.slice(0, THREAD_LIMIT)).map((thread) => (
            <ThreadRow
              key={thread.id}
              thread={thread}
              harness={names.get(thread.harness) ?? thread.harness}
            />
          ))}
        </Group>
      )}
    </>
  );
}

function harnessNames(harnesses: Harness[] | undefined): Map<string, string> {
  return new Map((harnesses ?? []).map((harness) => [harness.id, harness.name]));
}

/** A number from the summary: its name, the compact value, and the exact one on hover. */
function Stat({
  label,
  value,
  exact,
  note,
  secondary,
}: {
  label: string;
  value: string;
  exact?: number | string;
  note?: string;
  /** A quieter line under the primary stats (the token counts). */
  secondary?: boolean;
}) {
  return (
    <div className="min-w-0">
      <div className="text-caption text-fg-faint">{label}</div>
      <div
        className={
          secondary
            ? "truncate text-callout text-fg-muted tabular"
            : "truncate font-medium text-title3 tabular"
        }
        title={typeof exact === "number" ? exact.toLocaleString("en-US") : exact}
      >
        {value}
      </div>
      {note && <div className="truncate text-caption text-fg-faint">{note}</div>}
    </div>
  );
}

function tokensLabel(count: number | undefined): string {
  return count === undefined ? "—" : compactTokens(count);
}

const exactUsd = (usd: number): string => `$${usd.toFixed(4)}`;

/** A total's headline: ≈ when any part is estimated, `unknown` when nothing is. */
function costValue(cost: CostTotals): string {
  if (cost.usd === undefined) return "unknown";
  return `${cost.estimatedUsd !== undefined ? "≈" : ""}${formatCost(cost.usd)}`;
}

/** The reported and estimated parts, and what the total leaves out. */
function costNote(cost: CostTotals): string {
  const parts: string[] = [];
  if (cost.reportedUsd !== undefined) parts.push(`${formatCost(cost.reportedUsd)} reported`);
  if (cost.estimatedUsd !== undefined) parts.push(`${formatCost(cost.estimatedUsd)} estimated`);
  if (cost.unknownTurns > 0) {
    parts.push(`${cost.unknownTurns} ${cost.unknownTurns === 1 ? "turn" : "turns"} unknown`);
  }
  return parts.length ? parts.join(" · ") : "nothing reported or priced";
}

/** The exact breakdown, and the caveat that a list price is not a bill. */
function costExact(cost: CostTotals): string {
  if (cost.usd === undefined) {
    return "No turn reported a cost and no row could be priced at list prices";
  }
  const parts = [
    cost.reportedUsd !== undefined ? `${exactUsd(cost.reportedUsd)} reported` : "",
    cost.estimatedUsd !== undefined ? `${exactUsd(cost.estimatedUsd)} estimated` : "",
  ].filter(Boolean);
  const left = cost.unknownTurns > 0 ? `; ${cost.unknownTurns} turns reported no cost of their own` : "";
  return `${exactUsd(cost.usd)} USD: ${parts.join(" + ")}${left}. Estimates use API list prices on ${PRICE_DATE}, which is not necessarily what a subscription bills`;
}

/** A single row's value: ≈ for an estimate, `unknown` when neither number exists. */
function bestValue(usd: number | undefined, estimated: boolean): string {
  if (usd === undefined) return "unknown";
  return `${estimated ? "≈" : ""}${formatCost(usd)}`;
}

/** The exact count of a token, for a cell's tooltip. */
const exact = (count: number | undefined) =>
  count === undefined ? "None reported" : count.toLocaleString("en-US");

/** A thread's input + output, or absent when neither was reported. */
function used(totals: UsageTotals): number | undefined {
  return totals.input === undefined && totals.output === undefined ? undefined : usedTokens(totals);
}

/** One model row in Cost by model: what it cost, and which number that was. */
interface ModelCost {
  harness: string;
  model: string;
  cost: CostTotals;
  usd?: number;
  turns: number;
}

/** Every model row, with its best known cost, the costliest first and unknown last. */
function modelCosts(groups: HarnessTotal[]): ModelCost[] {
  const rows: ModelCost[] = [];
  for (const group of groups) {
    for (const model of group.models) {
      const one = costOf({ harness: group.harness, model: model.model, totals: model.totals });
      rows.push({
        harness: group.harness,
        model: model.model,
        cost: one,
        usd: one.usd,
        turns: model.totals.turns,
      });
    }
  }
  return rows.sort((one, other) => {
    if (one.usd === undefined || other.usd === undefined) {
      if (one.usd === other.usd) return other.turns - one.turns || one.model.localeCompare(other.model);
      return one.usd === undefined ? 1 : -1;
    }
    return other.usd - one.usd || one.model.localeCompare(other.model);
  });
}

/** Bars of each model's best known cost, relative to the costliest row. */
function ModelCosts({ rows, names }: { rows: ModelCost[]; names: Map<string, string> }) {
  const highest = Math.max(...rows.map((row) => row.usd ?? 0));
  return (
    <div className="flex flex-col gap-2 px-4 py-4">
      {rows.map((row) => {
        const estimated = row.cost.estimatedUsd !== undefined;
        const width = row.usd !== undefined && highest > 0 ? (row.usd / highest) * 100 : 0;
        return (
          <div
            key={`${row.harness}/${row.model}`}
            className="flex min-w-0 items-center gap-2"
            title={modelCostTitle(row, names.get(row.harness) ?? row.harness)}
          >
            <HarnessIcon id={row.harness} name={names.get(row.harness)} size={13} />
            <span className="w-2/5 min-w-0 truncate text-callout text-fg-muted">
              {row.model || "Unknown model"}
            </span>
            {row.usd !== undefined ? (
              <span className="h-2 min-w-0 flex-1 overflow-hidden rounded-full bg-fill-subtle">
                <span
                  className={cx(
                    "block h-full rounded-full",
                    estimated ? "bg-accent/45" : "bg-accent",
                    // A tiny share still draws: a 4px minimum keeps it from
                    // looking like an empty slot.
                    row.usd > 0 && "min-w-[4px]",
                  )}
                  style={{ width: `${width}%` }}
                />
              </span>
            ) : (
              // No track for an unknown row: an empty bar would read as $0.
              <span className="min-w-0 flex-1" />
            )}
            <span
              className={
                row.usd === undefined
                  ? "w-[4.5rem] shrink-0 text-right text-callout text-fg-faint tabular"
                  : "w-[4.5rem] shrink-0 text-right text-callout text-fg-muted tabular"
              }
            >
              {bestValue(row.usd, estimated)}
            </span>
          </div>
        );
      })}
    </div>
  );
}

function modelCostTitle(row: ModelCost, harness: string): string {
  if (row.cost.reportedUsd !== undefined) {
    return `${exactUsd(row.cost.reportedUsd)} USD reported by ${harness}`;
  }
  if (row.cost.estimatedUsd !== undefined) {
    return `${exactUsd(row.cost.estimatedUsd)} USD estimated at API list prices on ${PRICE_DATE}, not necessarily what a subscription bills; no turn in this row reported a cost`;
  }
  const priced = priceOf(row.model) !== undefined;
  return priced
    ? `${harness} records token counts this page cannot split into input, cache read and cache write; this row has no reported cost and no estimate`
    : `No list price for ${row.model || "this model"}; this row has no reported cost`;
}

function UsageTable({
  groups,
  names,
  costs,
}: {
  groups: HarnessTotal[];
  names: Map<string, string>;
  costs: Map<string, CostTotals>;
}) {
  return (
    <div className="@container/usage overflow-x-auto">
      <table className="w-full border-collapse text-callout">
        <thead>
          <tr className="border-border border-b text-caption text-fg-faint">
            <th scope="col" className="px-4 py-2 text-left font-normal">
              Harness / model
            </th>
            <th scope="col" className="px-2 py-2 text-right font-normal">
              Input
            </th>
            <th scope="col" className="px-2 py-2 text-right font-normal">
              Output
            </th>
            <th scope="col" className={`px-2 py-2 text-right font-normal ${NARROW_COLUMN}`}>
              Cached
            </th>
            <th scope="col" className={`px-2 py-2 text-right font-normal ${NARROW_COLUMN}`}>
              Cache write
            </th>
            <th scope="col" className="px-2 py-2 text-right font-normal">
              Cost
            </th>
            <th scope="col" className="px-4 py-2 text-right font-normal">
              Turns
            </th>
          </tr>
        </thead>
        <tbody>
          {groups.map((group) => {
            const groupCost = costs.get(group.harness) ?? { unknownTurns: 0 };
            return (
              <Fragment key={group.harness}>
                <tr className="border-border border-b bg-surface-2/50">
                  <th scope="row" className="min-w-0 px-4 py-2 text-left font-medium">
                    <span className="flex min-w-0 items-center gap-2">
                      <HarnessIcon id={group.harness} name={names.get(group.harness)} size={13} />
                      <span className="truncate">{names.get(group.harness) ?? group.harness}</span>
                    </span>
                  </th>
                  <NumberCell
                    count={group.totals.input}
                    missing={group.totals.turnsWithoutInputTokens}
                    label="input"
                  />
                  <NumberCell
                    count={group.totals.output}
                    missing={group.totals.turnsWithoutOutputTokens}
                    label="output"
                  />
                  <NumberCell
                    narrow
                    count={group.totals.cached}
                    missing={group.totals.turnsWithoutCachedTokens}
                    label="cached"
                  />
                  <NumberCell
                    narrow
                    count={group.totals.cacheWrite}
                    missing={group.totals.turnsWithoutCacheWriteTokens}
                    label="cache write"
                  />
                  <td className="px-2 py-2 text-right tabular" title={costTotalsTitle(groupCost)}>
                    {costValue(groupCost)}
                  </td>
                  <td className="px-4 py-2 text-right text-fg-muted tabular">{group.totals.turns}</td>
                </tr>
                {group.models.map((model) => {
                  const input: CostInput = {
                    harness: group.harness,
                    model: model.model,
                    totals: model.totals,
                  };
                  return (
                    <tr key={model.model || "unknown"} className="border-border border-b last:border-b-0">
                      <td className="min-w-0 px-4 py-2 pl-9 text-fg-muted">
                        <span className="flex min-w-0 items-center gap-2">
                          <span className="truncate">{model.model || "Unknown model"}</span>
                        </span>
                      </td>
                      <NumberCell
                        count={model.totals.input}
                        missing={model.totals.turnsWithoutInputTokens}
                        label="input"
                      />
                      <NumberCell
                        count={model.totals.output}
                        missing={model.totals.turnsWithoutOutputTokens}
                        label="output"
                      />
                      <NumberCell
                        narrow
                        count={model.totals.cached}
                        missing={model.totals.turnsWithoutCachedTokens}
                        label="cached"
                      />
                      <NumberCell
                        narrow
                        count={model.totals.cacheWrite}
                        missing={model.totals.turnsWithoutCacheWriteTokens}
                        label="cache write"
                      />
                      <CostCell input={input} />
                      <td className="px-4 py-2 text-right text-fg-muted tabular">{model.totals.turns}</td>
                    </tr>
                  );
                })}
              </Fragment>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function NumberCell({
  count,
  missing,
  label,
  narrow,
}: {
  count: number | undefined;
  missing: number;
  label: string;
  /** Steps out when the table's card is narrow (cached and cache write). */
  narrow?: boolean;
}) {
  const partial = count !== undefined && missing > 0;
  return (
    <td
      className={narrow ? `px-2 py-2 text-right tabular ${NARROW_COLUMN}` : "px-2 py-2 text-right tabular"}
      title={countTitle(count, missing, label)}
    >
      {tokensLabel(count)}
      {partial && <MissingMark label={`${missing} turns reported no ${label} tokens`} />}
    </td>
  );
}

const countTitle = (count: number | undefined, missing: number, label: string) =>
  count === undefined
    ? `No turn in this row reported ${label} tokens`
    : `${exact(count)}${missing ? `; ${missing} turns reported no ${label} tokens` : ""}`;

/** A model row's cost: its reported USD, else its estimate, else `unknown`. */
function CostCell({ input }: { input: CostInput }) {
  const best = bestCost(input);
  const estimated = best.source === "estimated";
  const partial = input.totals.cost !== undefined && (input.totals.turnsWithoutCost ?? 0) > 0;
  return (
    <td className="px-2 py-2 text-right tabular" title={costCellTitle(input)}>
      {bestValue(best.usd, estimated)}
      {partial && <span className="text-fg-faint">†</span>}
    </td>
  );
}

function costCellTitle(input: CostInput): string {
  const best = bestCost(input);
  if (best.source === "reported" && best.usd !== undefined) {
    const turnsWithoutCost = input.totals.turnsWithoutCost ?? 0;
    const left = turnsWithoutCost ? `; ${turnsWithoutCost} turns reported none` : "";
    return `${exactUsd(best.usd)} USD reported${left}`;
  }
  if (best.source === "estimated" && best.usd !== undefined) {
    return `${exactUsd(best.usd)} USD estimated at API list prices on ${PRICE_DATE}, not necessarily what a subscription bills; no turn in this row reported a cost`;
  }
  return priceOf(input.model) !== undefined
    ? `${input.harness} records token counts this page cannot split into input, cache read and cache write; no turn in this row reported a cost`
    : `No list price for ${input.model || "a model the store did not record"}; no turn in this row reported a cost`;
}

/** A harness row's cost: the sum of its model rows, ≈ when any part is estimated. */
function costTotalsTitle(cost: CostTotals): string {
  if (cost.usd === undefined) {
    return "No model this harness ran has a reported cost or a list price this page can apply; not priced";
  }
  const parts = [
    cost.reportedUsd !== undefined ? `${exactUsd(cost.reportedUsd)} reported` : "",
    cost.estimatedUsd !== undefined ? `${exactUsd(cost.estimatedUsd)} estimated` : "",
  ].filter(Boolean);
  const left = cost.unknownTurns > 0 ? `; ${cost.unknownTurns} turns reported no cost` : "";
  return `${exactUsd(cost.usd)} USD: ${parts.join(" + ")}${left}`;
}

/** A small mark after a number that covers only the turns that reported it. */
function MissingMark({ label }: { label: string }) {
  return (
    <span role="img" aria-label={label} title={label} className="shrink-0 text-fg-faint">
      *
    </span>
  );
}

/** One column of Last 30 days: a UTC day the store grouped by, and its turns. */
interface DayColumn {
  day: string;
  turns: number;
}

/** The queried window's UTC days, from the store's `date(started_at)`; a missing day is an empty slot. */
function lastDays(rows: UsageRow[], since: string, count = DAY_WINDOW): DayColumn[] {
  const turnsByDay = new Map<string, number>();
  for (const row of rows) {
    if (row.day) turnsByDay.set(row.day, (turnsByDay.get(row.day) ?? 0) + row.turns);
  }
  const start = Date.parse(`${since}T00:00:00Z`);
  return Array.from({ length: count }, (_, index) => {
    const day = new Date(start + index * DAY_MS).toISOString().slice(0, 10);
    return { day, turns: turnsByDay.get(day) ?? 0 };
  });
}

const shortDay = (day: string): string =>
  new Date(`${day}T00:00:00Z`).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    timeZone: "UTC",
  });

/** Turns per day over the last 30 days, with no axis and a peak count instead. */
function TurnsChart({ columns }: { columns: DayColumn[] }) {
  const peak = Math.max(...columns.map((column) => column.turns));
  if (!columns.length || peak === 0) return null;
  const first = columns[0]?.day ?? "";
  const last = columns[columns.length - 1]?.day ?? "";
  return (
    <div className="px-4 py-4">
      <div
        className="flex h-14 items-end gap-[3px]"
        role="img"
        aria-label={`Turns per day over the last ${columns.length} days`}
      >
        {columns.map((column) => (
          <div
            key={column.day}
            className="flex h-full min-w-0 flex-1 items-end"
            title={`${shortDay(column.day)} (UTC): ${column.turns} ${column.turns === 1 ? "turn" : "turns"}`}
          >
            {column.turns > 0 && (
              <span
                className="block w-full rounded-xs bg-fg-muted/45"
                style={{ height: `${(column.turns / peak) * 100}%` }}
              />
            )}
          </div>
        ))}
      </div>
      <div className="mt-1.5 flex items-center justify-between text-caption text-fg-faint tabular">
        <span>{shortDay(first)}</span>
        <span>
          peak {peak} {peak === 1 ? "turn" : "turns"}
        </span>
        <span>{shortDay(last)}</span>
      </div>
    </div>
  );
}

function ThreadRow({ thread, harness }: { thread: ThreadTotal; harness: string }) {
  const own = tokensLabel(used(thread.own));
  const total = used(thread.total);
  const delegates = thread.delegateCount;
  const notes = [
    thread.total.turnsWithoutAnyTokens > 0
      ? `${thread.total.turnsWithoutAnyTokens} turns with missing counts`
      : "",
    thread.unlinked ? "Parent recorded by title only; counted on its own" : "",
  ].filter(Boolean);
  return (
    <Row
      title={
        <span className="flex min-w-0 items-center gap-2">
          <HarnessIcon id={thread.harness} name={harness} size={13} />
          <span className="truncate">{thread.title}</span>
          {thread.unlinked && <span className="shrink-0 text-caption text-fg-faint">unlinked</span>}
        </span>
      }
      subtitle={`${harness} · own ${own} in+out${
        delegates > 0 ? ` + ${delegates} delegated ${delegates === 1 ? "thread" : "threads"}` : ""
      } · ${thread.total.turns} turns`}
      status={notes.length ? notes.join(" · ") : undefined}
    >
      <div className="text-right leading-tight">
        <div className="font-medium text-body tabular" title={exact(total)}>
          {tokensLabel(total)}
        </div>
        <div className="text-caption text-fg-faint tabular" title={costExact(thread.cost)}>
          {costLine(thread.cost)}
        </div>
      </div>
    </Row>
  );
}

/** Which token columns are missing on some turns, with how many turns each. */
function missingColumns(totals: UsageTotals): string {
  const missing = [
    [totals.turnsWithoutInputTokens, "input"],
    [totals.turnsWithoutOutputTokens, "output"],
    [totals.turnsWithoutCachedTokens, "cached"],
    [totals.turnsWithoutCacheWriteTokens, "cache write"],
  ].flatMap(([count, name]) => (Number(count) > 0 ? [`${name} on ${Number(count)}`] : []));
  return missing.length ? `: ${missing.join(", ")} turns` : "";
}

/** The method behind a number, folded away: one sentence shows, the rest on demand. */
function HowThisIsCounted({ children }: { children: ReactNode }) {
  return (
    <details className="group">
      <summary className="flex w-fit cursor-pointer list-none items-center gap-1 rounded-sm text-caption text-fg-faint hover:text-fg-muted">
        <ChevronRight size={11} className="shrink-0 transition-transform group-open:rotate-90" aria-hidden />
        How this is counted
      </summary>
      <div className="mt-1 space-y-1">{children}</div>
    </details>
  );
}

function UsageFootnote({
  totals,
  cost,
  truncated,
  error,
  dayError,
}: {
  totals: UsageTotals;
  cost: CostTotals;
  truncated: boolean;
  error: string | null;
  dayError: string | null;
}) {
  return (
    <div className="space-y-1">
      <span className="block">
        Every turn in this Mac's local data store; a blank count means no turn in that row reported it.
      </span>
      <HowThisIsCounted>
        <span className="block">
          Each column sums only the counts that were reported, so a turn that reported one count and not
          another is included as it was reported. A * after a number means not every turn in that row reported
          that count. Cost is the USD a harness reported, or the same tokens at API list prices on{" "}
          {PRICE_DATE} where it reported none; a reported number is never altered. The total adds each row's
          reported cost or its estimate, never both, and shows the two parts beneath it. Estimates are not a
          bill: a subscription can charge differently. The CLI's rustcode usage is still reported-only.
        </span>
        {totals.turnsWithoutAnyTokens > 0 && (
          <span className="block">
            {totals.turnsWithoutAnyTokens} of {totals.turns} turns are missing at least one token count
            {missingColumns(totals)}.
          </span>
        )}
        {totals.turnsWithoutCost > 0 && (
          <span className="block">
            {totals.turnsWithoutCost} of {totals.turns} turns reported no cost
            {cost.unknownTurns > 0 ? `; ${cost.unknownTurns} have no cost we can show` : ""}.
          </span>
        )}
      </HowThisIsCounted>
      {truncated && (
        <span className="block text-warning">
          The harness and model read stopped at 1,000 groups; these totals cover only what it returned.
        </span>
      )}
      {error && <span className="block text-warning">Could not refresh: {error}</span>}
      {dayError && <span className="block text-warning">Could not read turns per day: {dayError}</span>}
    </div>
  );
}

/** How the cost column is priced, and what its marks mean. */
function UsageTableFootnote({ cost }: { cost: CostTotals }) {
  return (
    <div className="space-y-1">
      <span className="block">
        A harness row totals its models; a * means not every turn in that row reported a count.
      </span>
      <HowThisIsCounted>
        <span className="block">
          Cost is the USD a harness reported, or the same tokens at public API list prices on {PRICE_DATE}
          where it reported none. ≈ marks an estimate, and a reported number is never altered. Each harness's
          counts are normalized to uncached input, cache read, cache write and output first, because the
          built-in harness and Codex count cached input inside input tokens while Claude Code and Pi report it
          separately. A model with no list price, or a harness whose counts cannot be split, shows unknown,
          never $0.
        </span>
        {cost.unknownTurns > 0 && (
          <span className="block">
            {cost.unknownTurns} turns have no cost we can show: no turn-level cost was reported for them, or
            their model has no list price here, or their harness's token counts cannot be split.
          </span>
        )}
        {cost.usd === undefined && <span className="block">No row here could be priced.</span>}
      </HowThisIsCounted>
    </div>
  );
}

function ThreadFootnote({ report }: { report: ThreadReport }) {
  return (
    <div className="space-y-1">
      <span className="block">
        Each thread is counted once: under the thread that started it, or on its own when that thread has no
        recorded turns.
      </span>
      <HowThisIsCounted>
        <span className="block">
          A parent's total includes its delegates at any depth; the number on the right is input and output
          tokens, and cached tokens are not added to it. A thread's cost is its reported cost or, when every
          turn ran one harness and model, its estimate, rolled up the same way; a thread that switched is
          estimated only where a reported cost exists, so a parent can mix reported and estimated parts.
        </span>
        {report.unlinked > 0 && (
          <span className="block">
            {report.unlinked} {report.unlinked === 1 ? "thread records its" : "threads record their"} parent
            by title only, so {report.unlinked === 1 ? "it is" : "they are"} counted on{" "}
            {report.unlinked === 1 ? "its" : "their"} own.
          </span>
        )}
        {report.brokenParents > 0 && (
          <span className="block">
            {report.brokenParents} parent {report.brokenParents === 1 ? "record forms" : "records form"} a
            loop; one edge was ignored so every thread is counted once.
          </span>
        )}
      </HowThisIsCounted>
      {report.hierarchyIncomplete && (
        <span className="block text-warning">
          The thread list {report.sessionsCapped ? "stopped at its page cap" : "is missing threads"}, so some
          delegated threads may be counted on their own.
        </span>
      )}
      {report.truncated && (
        <span className="block text-warning">
          The thread read stopped at 1,000 groups; these totals cover only the threads it returned.
        </span>
      )}
    </div>
  );
}

function NothingRecorded() {
  return (
    <Group title="Recorded usage">
      <Row
        title="Nothing recorded yet"
        subtitle="Run a turn, or open the app once so existing threads are collected into the local data store."
      />
    </Group>
  );
}
