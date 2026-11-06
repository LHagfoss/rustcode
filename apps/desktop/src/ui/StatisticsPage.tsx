// Settings › Statistics: what the local data store recorded of every turn, by
// harness and model and by thread (`_rustcode/data/usage`, docs/data-store.md).
// A thread's total includes the threads it started, so a parent shows the work
// it delegated without the same tokens being counted twice. A count a harness
// never reported stays blank, and **Reported** cost is only ever the USD a
// harness reported. **Estimated** is the same tokens at public list prices
// (`state/pricing.ts`), kept in its own column: it never merges into Reported,
// and a harness whose token semantics or a model whose price is unknown shows
// `—` rather than a guessed number.

import { RefreshCw } from "lucide-react";
import { Fragment, useMemo, useState } from "react";
import { parseUsageReport, type UsageTotals } from "../acp/rustcode";
import { type Harness, Method } from "../acp/types";
import {
  estimateAll,
  estimateUsd,
  type GroupEstimate,
  PRICE_DATE,
  priceOf,
  type ReportEstimate,
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
import { Spinner } from "./primitives";
import { Action, Group, Loading, Row, useEngineData } from "./settingsKit";

/** How many top-level threads the page draws before Show all. */
const THREAD_LIMIT = 10;

export function StatisticsPage() {
  const models = useEngineData(Method.dataUsage, parseUsageReport, { groupBy: "model" });
  const threads = useEngineData(Method.dataUsage, parseUsageReport, { groupBy: "thread" });
  const sessions = useApp((s) => s.sessions);
  const sessionsCapped = useApp((s) => s.sessionsCapped);
  const links = useApp((s) => s.links.links);
  const harnesses = useApp((s) => s.harnesses?.harnesses);
  const [refreshing, setRefreshing] = useState(false);
  const [all, setAll] = useState(false);

  const groups = useMemo(() => harnessTotals(models.data?.rows ?? []), [models.data?.rows]);
  const estimates = useMemo(() => estimateAll(groups), [groups]);
  const rolled = useMemo(
    () => (threads.data ? threadReport(threads.data, sessions, links, sessionsCapped) : null),
    [threads.data, sessions, links, sessionsCapped],
  );
  const names = useMemo(() => harnessNames(harnesses), [harnesses]);

  if (!models.data || !threads.data || !rolled) return <Loading error={models.error ?? threads.error} />;

  const totals = models.data.totals;
  const error = models.error ?? threads.error;
  if (!groups.length && !rolled.threads.length && !error) return <NothingRecorded />;

  const refresh = async () => {
    setRefreshing(true);
    try {
      await Promise.all([models.reload(), threads.reload()]);
    } finally {
      setRefreshing(false);
    }
  };

  return (
    <>
      <Group
        title="Recorded usage"
        action={
          <Action onClick={() => void refresh()} disabled={refreshing}>
            {refreshing ? <Spinner size={11} /> : <RefreshCw size={12} />} Refresh
          </Action>
        }
        footer={<UsageFootnote totals={totals} truncated={models.data.truncated} error={error} />}
      >
        <div className="grid grid-cols-2 gap-x-6 gap-y-4 px-4 py-4 sm:grid-cols-3">
          <Stat label="Input" value={tokensLabel(totals.input)} exact={totals.input} />
          <Stat label="Output" value={tokensLabel(totals.output)} exact={totals.output} />
          <Stat label="Cached" value={tokensLabel(totals.cached)} exact={totals.cached} />
          <Stat label="Cache write" value={tokensLabel(totals.cacheWrite)} exact={totals.cacheWrite} />
          <Stat
            label="Reported cost"
            value={formatCost(totals.cost)}
            exact={totals.cost === undefined ? "No turn reported a cost" : `${totals.cost} USD reported`}
            note={totals.cost === undefined ? "none reported" : "USD, as reported"}
          />
          <Stat
            label="Estimated cost"
            value={formatCost(estimates.usd)}
            exact={
              estimates.usd === undefined
                ? "No row could be priced"
                : `${estimates.usd} USD at list prices on ${PRICE_DATE}`
            }
            note={
              estimates.usd === undefined
                ? "no list price"
                : estimates.turnsNotEstimated > 0
                  ? `${estimates.turnsNotEstimated} turns not estimated`
                  : "USD, list prices"
            }
          />
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
      </Group>

      {groups.length > 0 && (
        <Group title="By harness and model" footer={<UsageTableFootnote estimates={estimates} />}>
          <UsageTable groups={groups} names={names} estimates={estimates} />
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
}: {
  label: string;
  value: string;
  exact?: number | string;
  note?: string;
}) {
  return (
    <div className="min-w-0">
      <div className="text-caption text-fg-faint">{label}</div>
      <div
        className="truncate font-medium text-title3 tabular"
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

function formatCost(cost: number | undefined): string {
  if (cost === undefined) return "—";
  if (cost > 0 && cost < 0.01) return "<$0.01";
  return `$${cost.toFixed(2)}`;
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
  estimates,
}: {
  groups: HarnessTotal[];
  names: Map<string, string>;
  estimates: ReportEstimate;
}) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full min-w-[680px] border-collapse text-callout">
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
            <th scope="col" className="px-2 py-2 text-right font-normal">
              Cached
            </th>
            <th scope="col" className="px-2 py-2 text-right font-normal">
              Cache write
            </th>
            <th scope="col" className="px-2 py-2 text-right font-normal">
              Reported
            </th>
            <th scope="col" className="px-2 py-2 text-right font-normal">
              Estimated
            </th>
            <th scope="col" className="px-4 py-2 text-right font-normal">
              Turns
            </th>
          </tr>
        </thead>
        <tbody>
          {groups.map((group) => {
            const estimate = estimates.byHarness.get(group.harness);
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
                    count={group.totals.cached}
                    missing={group.totals.turnsWithoutCachedTokens}
                    label="cached"
                  />
                  <NumberCell
                    count={group.totals.cacheWrite}
                    missing={group.totals.turnsWithoutCacheWriteTokens}
                    label="cache write"
                  />
                  <CostCell totals={group.totals} />
                  <EstimateCell
                    usd={estimate?.usd}
                    turnsNotEstimated={estimate?.turnsNotEstimated ?? group.totals.turns}
                    title={harnessEstimateTitle(group, estimate)}
                  />
                  <td className="px-4 py-2 text-right text-fg-muted tabular">{group.totals.turns}</td>
                </tr>
                {group.models.map((model) => {
                  const row = estimateUsd(group.harness, model.model, model.totals);
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
                        count={model.totals.cached}
                        missing={model.totals.turnsWithoutCachedTokens}
                        label="cached"
                      />
                      <NumberCell
                        count={model.totals.cacheWrite}
                        missing={model.totals.turnsWithoutCacheWriteTokens}
                        label="cache write"
                      />
                      <CostCell totals={model.totals} />
                      <EstimateCell
                        usd={row}
                        turnsNotEstimated={row === undefined ? model.totals.turns : 0}
                        title={modelEstimateTitle(group.harness, model.model, model.totals)}
                      />
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
}: {
  count: number | undefined;
  missing: number;
  label: string;
}) {
  const partial = count !== undefined && missing > 0;
  return (
    <td className="px-2 py-2 text-right tabular" title={countTitle(count, missing, label)}>
      {tokensLabel(count)}
      {partial && <MissingMark label={`${missing} turns reported no ${label} tokens`} />}
    </td>
  );
}

const countTitle = (count: number | undefined, missing: number, label: string) =>
  count === undefined
    ? `No turn in this row reported ${label} tokens`
    : `${exact(count)}${missing ? `; ${missing} turns reported no ${label} tokens` : ""}`;

function CostCell({ totals }: { totals: UsageTotals }) {
  const partial = totals.cost !== undefined && totals.turnsWithoutCost > 0;
  return (
    <td className="px-2 py-2 text-right tabular" title={costTitle(totals)}>
      {formatCost(totals.cost)}
      {partial && <span className="text-fg-faint">†</span>}
    </td>
  );
}

const costTitle = (totals: UsageTotals) =>
  totals.cost === undefined
    ? "No turn in this row reported a cost"
    : `${totals.cost} USD reported${
        totals.turnsWithoutCost ? `; ${totals.turnsWithoutCost} turns reported none` : ""
      }`;

/**
 * What a row's tokens would cost at public list prices: its own column, never
 * merged into Reported, and blank (with a * for the turns left out) when the
 * row has no price or its harness's counts cannot be normalized.
 */
function EstimateCell({
  usd,
  turnsNotEstimated,
  title,
}: {
  usd: number | undefined;
  turnsNotEstimated: number;
  title: string;
}) {
  return (
    <td className="px-2 py-2 text-right text-fg-muted tabular" title={title}>
      {formatCost(usd)}
      {usd !== undefined && turnsNotEstimated > 0 && (
        <MissingMark label={`${turnsNotEstimated} turns left out of the estimate`} />
      )}
    </td>
  );
}

/** A model row's estimate in words: why it is blank, or the rates it used. */
function modelEstimateTitle(harness: string, model: string | undefined, totals: UsageTotals): string {
  const usd = estimateUsd(harness, model, totals);
  const price = priceOf(model);
  if (usd === undefined || !price) {
    if (!price) {
      return `No list price for ${model || "a model the store did not record"}; this row is not estimated`;
    }
    return `${harness} records token counts this page cannot split into input, cache read and cache write; not estimated`;
  }
  const perMillion = `$${price.input} input, $${price.cacheRead} cache read, $${price.cacheWrite} cache write, $${price.output} output per million tokens`;
  return `${usd.toFixed(4)} USD at list prices on ${PRICE_DATE}: ${perMillion}`;
}

/** A harness row's estimate in words: the sum of its priced model rows, and what it leaves out. */
function harnessEstimateTitle(group: HarnessTotal, estimate: GroupEstimate | undefined): string {
  if (estimate?.usd === undefined) {
    return `No model ${group.harness} ran has both a list price and token counts this page can split; not estimated`;
  }
  const left =
    estimate.turnsNotEstimated > 0
      ? `; ${estimate.turnsNotEstimated} turns in rows without a price are left out`
      : "";
  return `${estimate.usd.toFixed(4)} USD at list prices on ${PRICE_DATE}${left}`;
}

/** The table's foot: how much of it could be priced, and what the * means. */
function UsageTableFootnote({ estimates }: { estimates: ReportEstimate }) {
  const priced = estimates.usd !== undefined;
  return (
    <>
      <span className="block">
        A harness row totals its models. A token or cost no turn in a row reported is blank, never a made-up
        zero; a * after one means not every turn in that row reported it. Estimated is what the same tokens
        would cost at public list prices on {PRICE_DATE}, in a column of its own: it is never merged into
        Reported, and each harness's counts are normalized first, since some count cached input inside input
        tokens and others report it separately.
      </span>
      {priced && estimates.turnsNotEstimated > 0 && (
        <span className="block">
          {estimates.turnsNotEstimated} of these turns are not estimated: their model has no list price here,
          or their harness's token counts cannot be split. A * after an estimate marks a row it leaves turns
          out of.
        </span>
      )}
      {!priced && <span className="block">No row here could be priced; the column stays blank.</span>}
    </>
  );
}

/** A small mark after a number that covers only the turns that reported it. */
function MissingMark({ label }: { label: string }) {
  return (
    <span role="img" aria-label={label} title={label} className="shrink-0 text-fg-faint">
      *
    </span>
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
    thread.total.cost !== undefined && thread.total.turnsWithoutCost > 0
      ? `${thread.total.turnsWithoutCost} turns without a reported cost`
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
        <div
          className="text-caption text-fg-faint tabular"
          title={
            thread.total.cost === undefined
              ? "No turn in this thread reported a cost"
              : costTitle(thread.total)
          }
        >
          {thread.total.cost === undefined ? "no cost reported" : `${formatCost(thread.total.cost)} reported`}
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

function UsageFootnote({
  totals,
  truncated,
  error,
}: {
  totals: UsageTotals;
  truncated: boolean;
  error: string | null;
}) {
  return (
    <>
      <span className="block">
        Every turn in this Mac's local data store. Tokens are what each harness reported; a blank count means
        no turn in that row reported one. Each column sums only the counts that were reported, so a turn that
        reported one count and not another is included as it was reported. A * after a number means not every
        turn in that row reported that count; a † after a cost means not every turn reported one. Reported
        cost is the USD a harness reported; Estimated is what the same tokens would cost at public list prices
        on {PRICE_DATE}, from the app's own table and never a network read. The two are kept apart and are
        never added together. The CLI's `rustcode usage` has no estimated column yet.
      </span>
      {totals.turnsWithoutAnyTokens > 0 && (
        <span className="block">
          {totals.turnsWithoutAnyTokens} of {totals.turns} turns are missing at least one token count
          {missingColumns(totals)}.
        </span>
      )}
      {totals.turnsWithoutCost > 0 && (
        <span className="block">
          {totals.turnsWithoutCost} of {totals.turns} turns reported no cost.
        </span>
      )}
      {truncated && (
        <span className="block text-warning">
          The harness and model read stopped at 1,000 groups; these totals cover only what it returned.
        </span>
      )}
      {error && <span className="block text-warning">Could not refresh: {error}</span>}
    </>
  );
}

function ThreadFootnote({ report }: { report: ThreadReport }) {
  return (
    <>
      <span className="block">
        Each thread is counted once: under the thread that started it, or on its own when that thread has no
        recorded turns. A parent's total includes its delegates; the number on the right is input and output
        tokens, and cached tokens are not added to it.
      </span>
      {report.unlinked > 0 && (
        <span className="block">
          {report.unlinked} {report.unlinked === 1 ? "thread records its" : "threads record their"} parent by
          title only, so {report.unlinked === 1 ? "it is" : "they are"} counted on{" "}
          {report.unlinked === 1 ? "its" : "their"} own.
        </span>
      )}
      {report.brokenParents > 0 && (
        <span className="block">
          {report.brokenParents} parent {report.brokenParents === 1 ? "record forms" : "records form"} a loop;
          one edge was ignored so every thread is counted once.
        </span>
      )}
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
    </>
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
