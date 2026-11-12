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
//
// The page is read in one period at a time (`state/period.ts`): the totals at
// the top are one report for the period and one for the period before, the day
// read covers both so a sparkline and the daily chart have their days, and the
// breakdown's bar and table share the model rows.

import { ChevronRight, RefreshCw } from "lucide-react";
import { Fragment, type ReactNode, useEffect, useMemo, useState } from "react";
import { parseUsageReport, type UsageRow, type UsageTotals, windowTitle } from "../acp/rustcode";
import { type Harness, Method } from "../acp/types";
import { count } from "../lib/plural";
import {
  dailySeries,
  previousRows,
  rangeDays,
  STATS_PERIODS,
  type StatsPeriod,
  statsRange,
  sumRows,
  trend,
} from "../state/period";
import {
  bestCost,
  type CostInput,
  type CostTotals,
  costLine,
  costOf,
  costOfRows,
  formatCost,
  PRICE_DATE,
  priceOf,
  rollUpCosts,
} from "../state/pricing";
import {
  type HarnessTotal,
  harnessTotals,
  type ModelTotal,
  type ThreadReport,
  type ThreadTotal,
  threadReport,
  usedTokens,
} from "../state/statistics";
import { openSettings, useApp } from "../state/store";
import { formatTokens as compactTokens, relativeAge } from "../state/thread";
import { PlanWindow } from "./ComposerUsage";
import { HarnessIcon } from "./HarnessIcon";
import { cx, Skeleton, Spinner } from "./primitives";
import {
  ChartSkeleton,
  CostShare,
  DailyChart,
  type DailyMetric,
  type HarnessShare,
  Tile,
  useGlide,
} from "./StatisticsCharts";
import { Action, Chip, Choice, Group, Loading, Row, SectionTitle, useEngineData } from "./settingsKit";

/** How many top-level threads the page draws before Show all. */
const THREAD_LIMIT = 10;

/** The cost table's token columns that step out when its card is narrow. */
const NARROW_COLUMN = "@max-[36rem]/usage:hidden";

const DEFAULT_PERIOD: StatsPeriod = "30d";

/** The cost of a set of rows, each row's reported number or its estimate. */
const costsOf = costOfRows;

const modelInput = (harness: string, model: ModelTotal): CostInput => ({
  harness,
  model: model.model,
  totals: model.totals,
});

/** A row's best known cost, or -1 so an unknown row ranks after every known one. */
const rankOf = (cost: CostTotals | undefined): number => cost?.usd ?? -1;

/** Harnesses and their models by cost, costliest first: the bar and the table share this order. */
function rankGroups(groups: HarnessTotal[], costs: Map<string, CostTotals>): HarnessTotal[] {
  return groups
    .map((group) => ({
      ...group,
      models: [...group.models].sort(
        (one, other) =>
          rankOf(costOf(modelInput(group.harness, other))) - rankOf(costOf(modelInput(group.harness, one))) ||
          usedTokens(other.totals) - usedTokens(one.totals) ||
          one.model.localeCompare(other.model),
      ),
    }))
    .sort(
      (one, other) =>
        rankOf(costs.get(other.harness)) - rankOf(costs.get(one.harness)) ||
        usedTokens(other.totals) - usedTokens(one.totals) ||
        one.harness.localeCompare(other.harness),
    );
}

/** The period's models by harness, the cost of each harness, and its models ordered by cost. */
function useUsage(rows: UsageRow[] | undefined) {
  return useMemo(() => {
    const groups = harnessTotals(rows ?? []);
    const costs = new Map<string, CostTotals>();
    for (const group of groups) {
      costs.set(group.harness, rollUpCosts(group.models.map((model) => modelInput(group.harness, model))));
    }
    return { groups, costs, total: costsOf(rows ?? []), ranked: rankGroups(groups, costs) };
  }, [rows]);
}

export function StatisticsPage() {
  const [period, setPeriod] = useState<StatsPeriod>(DEFAULT_PERIOD);
  const [metric, setMetric] = useState<DailyMetric>("turns");
  const [hidden, setHidden] = useState<ReadonlySet<string>>(() => new Set());
  const [hovered, setHovered] = useState<string | null>(null);
  const [all, setAll] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const range = useMemo(() => statsRange(period, new Date()), [period]);

  // The period, and the same-length period before it, each read on its own so
  // their totals are the store's own sums rather than a page slicing.
  const current = useEngineData(Method.dataUsage, parseUsageReport, {
    groupBy: "model",
    ...(range.since ? { since: range.since } : {}),
  });
  const before = useEngineData(
    Method.dataUsage,
    parseUsageReport,
    { groupBy: "model", ...(range.previousSince ? { since: range.previousSince } : {}) },
    { skip: period === "all" },
  );
  // One day read covers both periods: the sparklines and the daily chart draw
  // the current period's days, and the previous period's days count for its
  // change. "All" asks for every day the store recorded.
  const days = useEngineData(Method.dataUsage, parseUsageReport, {
    groupBy: "day",
    ...(range.previousSince ? { since: range.previousSince } : {}),
  });
  const threads = useEngineData(Method.dataUsage, parseUsageReport, { groupBy: "thread" });

  const sessions = useApp((s) => s.sessions);
  const sessionsCapped = useApp((s) => s.sessionsCapped);
  const links = useApp((s) => s.links.links);
  const harnesses = useApp((s) => s.harnesses?.harnesses);
  const names = useMemo(() => harnessNames(harnesses), [harnesses]);

  const usage = useUsage(current.data?.rows);
  const beforeRows = useMemo(() => {
    if (period === "all" || !before.data) return undefined;
    return previousRows(before.data.rows, current.data?.rows ?? []);
  }, [period, before.data, current.data?.rows]);
  // The comparison period's cost is priced from the rows it had on its own.
  const beforeCostTotals = useMemo(() => (beforeRows ? costsOf(beforeRows) : undefined), [beforeRows]);

  const rolled = useMemo(
    () => (threads.data ? threadReport(threads.data, sessions, links, sessionsCapped) : null),
    [threads.data, sessions, links, sessionsCapped],
  );
  const points = useMemo(() => {
    const rows = days.data?.rows ?? [];
    return dailySeries(rows, range.days ? rangeDays(range.since, range.days) : []);
  }, [days.data?.rows, range.since, range.days]);
  const beforePoints = useMemo(() => {
    if (!range.days || !range.previousSince) return [];
    const rows = (days.data?.rows ?? []).filter(
      (row) => row.day !== undefined && row.day < range.since! && row.day >= range.previousSince!,
    );
    return dailySeries(rows, rangeDays(range.previousSince, range.days));
  }, [days.data?.rows, range.days, range.since, range.previousSince]);

  const error = current.error ?? days.error ?? threads.error;
  const compare =
    period === "all"
      ? "all time"
      : period === "today"
        ? "against yesterday"
        : `against the ${range.days} days before`;

  const refresh = async () => {
    setRefreshing(true);
    try {
      await Promise.all([current.reload(), before.reload(), days.reload(), threads.reload()]);
    } finally {
      setRefreshing(false);
    }
  };

  const blocked = current.error;
  if (!current.data && blocked) return <Loading error={blocked} />;
  if (!current.data) return <StatisticsSkeleton period={period} onPeriod={setPeriod} onRefresh={refresh} />;

  // Only a store with neither turns nor threads is empty: a period with nothing
  // in it still shows its tiles at zero, with the daily card saying so.
  if (threads.data && !usage.groups.length && !rolled?.threads.length) {
    return <NothingRecorded refresh={refresh} refreshing={refreshing} error={error} />;
  }

  const totals = sumRows(current.data.rows);
  const previousTotals = beforeRows ? sumRows(beforeRows) : undefined;
  // A period that recorded no turns spent nothing: its zero is a number, not a
  // blank, so the tile can say "up" instead of falling back to the comparison.
  const previousEmpty = previousTotals?.turns === 0;
  const beforeCost = previousEmpty ? 0 : beforeCostTotals?.usd;
  const beforeTokens = previousEmpty ? 0 : previousTotals ? used(previousTotals) : undefined;
  const activeDays = points.filter((point) => point.turns > 0).length;
  const beforeActiveDays = beforePoints.filter((point) => point.turns > 0).length;
  const change = (now?: number, was?: number) => (period === "all" ? undefined : trend(now, was));
  const activeCompare =
    period === "all" ? `${count(points.length, "day")} recorded` : `of ${count(range.days, "day")}`;

  return (
    <>
      <section className="mb-6 scroll-mt-5" data-setting="Totals">
        <SectionTitle
          title="Totals"
          action={
            <div className="flex flex-wrap items-center justify-end gap-2">
              <Choice<StatsPeriod>
                label="Period"
                value={period}
                options={STATS_PERIODS.map((one) => ({ value: one.value, label: one.label }))}
                onChange={setPeriod}
              />
              <Action onClick={() => void refresh()} disabled={refreshing}>
                {refreshing ? <Spinner size={11} /> : <RefreshCw size={12} />} Refresh
              </Action>
            </div>
          }
        />
        <div className="grid grid-cols-2 gap-3 @min-[44rem]:grid-cols-4">
          <Tile
            label="Cost"
            value={costValue(usage.total)}
            exact={costExact(usage.total)}
            note={costNote(usage.total)}
            trend={change(usage.total.usd, beforeCost)}
            compare={compare}
            series={points.map((point) => point.cost)}
            seriesLabel="Reported cost per day; estimates are not in the line"
          />
          <Tile
            label="Tokens"
            value={tokensLabel(used(totals))}
            exact={exact(used(totals))}
            note={`${tokensLabel(totals.input)} in · ${tokensLabel(totals.output)} out`}
            trend={change(used(totals), beforeTokens)}
            compare={compare}
            series={points.map((point) => point.tokens)}
            seriesLabel="Input and output tokens per day"
          />
          <Tile
            label="Turns"
            value={String(totals.turns)}
            count={totals.turns}
            note={
              totals.turnsWithoutAnyTokens > 0
                ? `${count(totals.turnsWithoutAnyTokens, "turn")} with missing counts`
                : `${count(totals.turns, "turn")} recorded`
            }
            trend={change(totals.turns, previousTotals?.turns)}
            compare={compare}
            series={points.map((point) => point.turns)}
            seriesLabel="Turns per day"
          />
          <Tile
            label="Active days"
            value={String(activeDays)}
            count={activeDays}
            note={activeCompare}
            trend={change(activeDays, period === "all" ? undefined : beforeActiveDays)}
            compare={compare}
            series={points.map((point) => (point.turns > 0 ? 1 : 0))}
            seriesLabel="Days with a turn, one mark each"
          />
        </div>
        <div className="mt-2 max-w-[70ch] px-3 text-caption text-fg-faint sm:px-4">
          <UsageFootnote
            totals={totals}
            cost={usage.total}
            truncated={current.data.truncated}
            error={error}
            dayError={days.error}
            period={period === "all" ? undefined : compare}
          />
        </div>
      </section>

      <Group
        title="Daily activity"
        action={days.data?.truncated ? <Chip tone="warn">Day read capped</Chip> : undefined}
        footer={
          <span className="block">
            One bar a day, in UTC as the store groups them. Cost is what harnesses reported that day; the
            tiles add a list-price estimate where none was reported.
          </span>
        }
      >
        {!days.data ? (
          <ChartSkeleton />
        ) : points.some((point) => point.turns > 0) ? (
          <DailyChart
            points={points}
            metric={metric}
            onMetric={setMetric}
            format={(value) =>
              metric === "cost"
                ? value === undefined
                  ? "unknown"
                  : formatCost(value)
                : value === undefined
                  ? "—"
                  : metric === "tokens"
                    ? compactTokens(value)
                    : String(value)
            }
          />
        ) : (
          <Row
            title="Nothing recorded in this period"
            subtitle="Pick a longer period, or run a turn and it lands here."
          >
            {period !== "all" && <Action onClick={() => setPeriod("all")}>Show all time</Action>}
          </Row>
        )}
      </Group>

      {usage.groups.length > 0 && (
        <Group
          title="Cost by harness and model"
          footer={
            <span className="block">
              The bar is each harness's share of the period's cost; picking a name in the legend takes it off
              the bar, and a row lights its harness. ≈ marks an estimate at API list prices on {PRICE_DATE},
              and a row with neither number has no bar.
            </span>
          }
        >
          {usage.ranked.some((group) => rankOf(usage.costs.get(group.harness)) > 0) ? (
            <CostShare
              shares={sharesOf(usage.ranked, usage.costs, names)}
              hidden={hidden}
              onToggle={(key) =>
                setHidden((was) => {
                  const next = new Set(was);
                  if (next.has(key)) next.delete(key);
                  else next.add(key);
                  return next;
                })
              }
              hovered={hovered}
              onHover={setHovered}
              formatCost={formatCost}
            />
          ) : (
            <Row
              title="No cost to compare"
              subtitle="No model this period ran reported a cost, and none has a list price this page can apply."
            />
          )}
          <UsageTable
            groups={usage.ranked}
            names={names}
            costs={usage.costs}
            hovered={hovered}
            hidden={hidden}
            onHover={setHovered}
          />
        </Group>
      )}

      <PlanLimits names={names} />

      {rolled && rolled.threads.length > 0 && (
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

/** Each harness's part of the cost, costliest first, for the bar and its legend. */
function sharesOf(
  groups: HarnessTotal[],
  costs: Map<string, CostTotals>,
  names: Map<string, string>,
): HarnessShare[] {
  return groups.flatMap((group) => {
    const cost = costs.get(group.harness);
    if (!cost?.usd || cost.usd <= 0) return [];
    return [
      {
        key: group.harness,
        label: names.get(group.harness) ?? group.harness,
        usd: cost.usd,
        estimated: cost.estimatedUsd !== undefined,
      },
    ];
  });
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
  if (cost.estimatedUsd !== undefined) parts.push(`${formatCost(cost.estimatedUsd)} est.`);
  if (cost.unknownTurns > 0) parts.push(`${count(cost.unknownTurns, "turn")} unknown`);
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

function UsageTable({
  groups,
  names,
  costs,
  hovered,
  hidden,
  onHover,
}: {
  groups: HarnessTotal[];
  names: Map<string, string>;
  costs: Map<string, CostTotals>;
  hovered: string | null;
  hidden: ReadonlySet<string>;
  onHover: (harness: string | null) => void;
}) {
  // Rows glide when the period changes the ranking: the order is the key.
  const order = groups
    .map((group) => `${group.harness}(${group.models.map((model) => model.model).join(",")})`)
    .join("|");
  const body = useGlide<HTMLTableSectionElement>(order);
  // On a touch pointer there is no hover to link a row to its segment.
  const [touch, setTouch] = useState(false);
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const media = window.matchMedia("(pointer: coarse)");
    setTouch(media.matches);
    const listen = () => setTouch(media.matches);
    media.addEventListener("change", listen);
    return () => media.removeEventListener("change", listen);
  }, []);
  const lit = (harness: string) => !touch && hovered === harness;
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
        <tbody ref={body}>
          {groups.map((group) => {
            const groupCost = costs.get(group.harness) ?? { unknownTurns: 0 };
            const off = hidden.has(group.harness);
            return (
              <Fragment key={group.harness}>
                <tr
                  data-glide={`h:${group.harness}`}
                  onPointerEnter={() => onHover(group.harness)}
                  onPointerLeave={() => onHover(null)}
                  className={cx(
                    "border-border border-b",
                    lit(group.harness) ? "bg-fill" : "bg-surface-2/50",
                    off && "opacity-50",
                  )}
                >
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
                  const input = modelInput(group.harness, model);
                  return (
                    <tr
                      key={model.model || "unknown"}
                      data-glide={`m:${group.harness}/${model.model}`}
                      onPointerEnter={() => onHover(group.harness)}
                      onPointerLeave={() => onHover(null)}
                      className={cx(
                        "border-border border-b last:border-b-0",
                        lit(group.harness) && "bg-fill-subtle",
                        off && "opacity-50",
                      )}
                    >
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

/** A ticking clock for the plan windows' reset countdowns, once every half minute. */
function useNow(every = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), every);
    return () => clearInterval(timer);
  }, [every]);
  return now;
}

/**
 * The plan windows this computer is signed in to, from the cache the composer's meter
 * reads (`_rustcode/usage/plans`): each window's used bar and when it resets.
 * Nothing is asked for here — Settings › Usage and the background poll read it.
 */
function PlanLimits({ names }: { names: Map<string, string> }) {
  const cache = useApp((s) => s.planCache);
  const now = useNow();
  const plans = useMemo(
    () =>
      Object.values(cache)
        .filter((plan) => plan.windows.length)
        .sort(
          (one, other) =>
            (one.name ?? one.source).localeCompare(other.name ?? other.source) ||
            (one.account ?? "").localeCompare(other.account ?? ""),
        ),
    [cache],
  );
  if (!plans.length) return null;
  return (
    <Group
      title="Usage limits"
      action={
        <Action quiet size="sm" onClick={() => openSettings("usage")}>
          Usage…
        </Action>
      }
      footer="Read from each subscription's own sign-in by Settings › Usage and the background poll; nothing new is read here."
    >
      <div className="flex flex-col gap-4 px-4 py-4">
        {plans.map((plan) => {
          const old = plan.fetchedAt !== undefined && now - plan.fetchedAt > 10 * 60_000;
          return (
            <div
              key={`${plan.source}/${plan.harness ?? ""}/${plan.account ?? ""}`}
              className="flex flex-col gap-2"
            >
              <div className="flex min-w-0 items-center gap-2">
                <span className="min-w-0 flex-1 truncate font-medium text-body">
                  {plan.name ?? (plan.harness ? (names.get(plan.harness) ?? plan.harness) : plan.source)}
                </span>
                {plan.plan && (
                  <span className="shrink-0 text-caption text-fg-faint capitalize">{plan.plan}</span>
                )}
                {plan.stale && <Chip tone="warn">stale</Chip>}
                {old && !plan.stale && <Chip tone="warn">not read lately</Chip>}
                {plan.fetchedAt !== undefined && (
                  <span className="shrink-0 text-caption text-fg-faint">
                    read {relativeAge(plan.fetchedAt, now)}
                  </span>
                )}
              </div>
              {plan.windows.map((window) => (
                <PlanWindow key={windowTitle(window)} window={window} title={windowTitle(window)} />
              ))}
            </div>
          );
        })}
      </div>
    </Group>
  );
}

function ThreadRow({ thread, harness }: { thread: ThreadTotal; harness: string }) {
  const own = tokensLabel(used(thread.own));
  const total = used(thread.total);
  const delegates = thread.delegateCount;
  const notes = [
    thread.total.turnsWithoutAnyTokens > 0
      ? `${count(thread.total.turnsWithoutAnyTokens, "turn")} with missing counts`
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
        delegates > 0 ? ` + ${count(delegates, "delegated thread")}` : ""
      } · ${count(thread.total.turns, "turn")}`}
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
  period,
}: {
  totals: UsageTotals;
  cost: CostTotals;
  truncated: boolean;
  error: string | null;
  dayError: string | null;
  /** What the tiles' change is compared with, when there is one. */
  period?: string;
}) {
  return (
    <div className="space-y-1">
      <span className="block">
        Every turn in this computer's local data store.{period ? ` Each tile compares ${period}.` : ""} A
        blank count means no turn in that row reported it.
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
            {count(totals.turnsWithoutAnyTokens, "turn")} of {count(totals.turns, "turn")}{" "}
            {totals.turnsWithoutAnyTokens === 1 ? "is" : "are"} missing at least one token count
            {missingColumns(totals)}.
          </span>
        )}
        {totals.turnsWithoutCost > 0 && (
          <span className="block">
            {count(totals.turnsWithoutCost, "turn")} of {count(totals.turns, "turn")} reported no cost
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

/** Where the tiles and the two charts will be, at the shape they take. */
function StatisticsSkeleton({
  period,
  onPeriod,
  onRefresh,
}: {
  period: StatsPeriod;
  onPeriod: (period: StatsPeriod) => void;
  onRefresh: () => void;
}) {
  return (
    <>
      <section className="mb-6">
        <SectionTitle
          title="Totals"
          action={
            <div className="flex flex-wrap items-center justify-end gap-2">
              <Choice<StatsPeriod>
                label="Period"
                value={period}
                options={STATS_PERIODS.map((one) => ({ value: one.value, label: one.label }))}
                onChange={onPeriod}
              />
              <Action onClick={onRefresh}>
                <RefreshCw size={12} /> Refresh
              </Action>
            </div>
          }
        />
        <div role="status" aria-label="Loading" className="grid grid-cols-2 gap-3 @min-[44rem]:grid-cols-4">
          {[0, 1, 2, 3].map((tile) => (
            <div key={tile} className="flex flex-col gap-2 rounded-xl bg-surface px-4 py-3">
              <Skeleton className="h-2 w-16" />
              <Skeleton className="h-6 w-24" />
              <Skeleton className="h-2 w-28" />
              <Skeleton className="mt-2 h-6 w-full" />
            </div>
          ))}
        </div>
      </section>
      {[0, 1].map((card) => (
        <section key={card} className="mb-6" role="status" aria-label="Loading">
          <div className="mb-2 flex min-h-7 items-center px-4">
            <Skeleton className="h-3 w-32" />
          </div>
          <div className="flex flex-col gap-3 rounded-xl bg-surface px-4 py-4">
            <Skeleton className="h-3 w-40" />
            <Skeleton className="h-32 w-full" />
          </div>
        </section>
      ))}
    </>
  );
}

function NothingRecorded({
  refresh,
  refreshing,
  error,
}: {
  refresh: () => void;
  refreshing: boolean;
  error: string | null;
}) {
  return (
    <Group
      title="Recorded usage"
      action={
        <Action onClick={() => void refresh()} disabled={refreshing}>
          {refreshing ? <Spinner size={11} /> : <RefreshCw size={12} />} Refresh
        </Action>
      }
      footer={error ? <span className="block text-warning">Could not refresh: {error}</span> : undefined}
    >
      <Row
        title="Nothing recorded yet"
        subtitle="Run a turn, or open the app once so existing threads are collected into the local data store."
      />
    </Group>
  );
}
