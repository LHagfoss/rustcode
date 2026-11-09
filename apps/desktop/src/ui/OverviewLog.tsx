// The overview's Execution log: what the thread spent and how long it worked,
// a bar for each turn, then the last few turns themselves — and, behind the
// header's expand, the whole log with each turn's own waterfall (ui/TurnTrace.tsx)
// where the app watched one. The turns come from the transcript the thread
// already holds, so the panel reads no transcript again (ui/overview.ts).

import { CircleCheck, CircleSlash, CircleX, Maximize2, Minimize2 } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";
import type { SessionUsage } from "../acp/types";
import { turnItemsByEnd } from "../state/fold";
import { formatDuration, formatTokens, type TranscriptItem } from "../state/thread";
import { HarnessIcon } from "./HarnessIcon";
import { OverviewSection, SectionButton } from "./OverviewSection";
import { logTotals, type TurnEntry, turnEntries, turnHasTrace } from "./overview";
import { cx, Tooltip } from "./primitives";
import { TurnTrace } from "./TurnTrace";

/** How many turns the section lists before the header's expand shows them all. */
export const LOG_ROWS = 3;
/** How many bars the strip draws: a thread with a hundred turns keeps its shape, not every bar. */
export const LOG_BARS = 24;
/** The turns' rows, before the full log is opened and needs them. */
const NO_TURNS: Record<string, TranscriptItem[]> = {};

const OUTCOME = {
  completed: { icon: CircleCheck, tone: "text-success", label: "Completed" },
  cancelled: { icon: CircleSlash, tone: "text-fg-muted", label: "Cancelled" },
  failed: { icon: CircleX, tone: "text-danger", label: "Failed" },
} as const;

/** One of the three numbers the log leads with. */
function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex min-w-0 flex-col">
      <span className="text-caption text-fg-faint">{label}</span>
      <span className="truncate font-medium text-body tabular">{value}</span>
    </div>
  );
}

/** A turn's own numbers, in the one measure the strip shows: money when it was priced, else tokens. */
function turnValue(turn: TurnEntry, measure: "cost" | "tokens"): number {
  return (measure === "cost" ? turn.cost : turn.tokens) ?? 0;
}

function barLabel(turn: TurnEntry, index: number, measure: "cost" | "tokens"): string {
  const parts = [`Turn ${index + 1}`];
  if (turn.elapsedMs !== undefined) parts.push(formatDuration(turn.elapsedMs));
  if (measure === "cost" && turn.cost !== undefined) parts.push(`$${turn.cost.toFixed(2)}`);
  else if (turn.tokens !== undefined) parts.push(`${formatTokens(turn.tokens)} tokens`);
  return parts.join(" · ");
}

/** A bar for each of the last turns: its height is its cost or its tokens, and its tip says which. */
function TurnBars({ turns }: { turns: readonly TurnEntry[] }) {
  const bars = turns.slice(-LOG_BARS);
  const measure: "cost" | "tokens" = bars.some((turn) => turn.cost !== undefined) ? "cost" : "tokens";
  const values = bars.map((turn) => turnValue(turn, measure));
  const most = Math.max(...values, 0);
  return (
    <div
      className="flex h-12 items-end gap-0.5 px-2"
      role="img"
      aria-label={`${measure === "cost" ? "Cost" : "Tokens"} per turn`}
    >
      {bars.map((turn, i) => (
        // A turn with nothing reported keeps a hairline, so the strip still counts every turn.
        <Tooltip key={turn.id} label={barLabel(turn, turns.length - bars.length + i, measure)}>
          <span className="flex h-full min-w-0 flex-1 items-end">
            <span
              style={{ height: `${most > 0 ? Math.max(2, (values[i]! / most) * 100) : 2}%` }}
              className={cx("w-full rounded-xs", turn.outcome === "failed" ? "bg-danger/60" : "bg-accent/60")}
            />
          </span>
        </Tooltip>
      ))}
    </div>
  );
}

/** One turn of the log: the agent that ran it, its first line, how it ended and what it cost. */
function TurnRow({
  turn,
  harness,
  trace,
}: {
  turn: TurnEntry;
  harness?: string;
  /** The turn's own waterfall, when the app watched one and the log is expanded. */
  trace?: ReactNode;
}) {
  const outcome = OUTCOME[turn.outcome];
  const Icon = outcome.icon;
  return (
    <div className="flex h-8 w-full items-center gap-2 rounded-md px-2">
      <span className="flex size-4 shrink-0 items-center justify-center">
        {harness && <HarnessIcon id={harness} size={14} />}
      </span>
      <span className="min-w-0 flex-1 truncate text-body">{turn.prompt}</span>
      {trace}
      <span className="shrink-0 text-caption text-fg-muted tabular">
        {turn.cost !== undefined ? `$${turn.cost.toFixed(2)}` : undefined}
      </span>
      <Tooltip label={outcome.label}>
        <span className={cx("flex size-4 shrink-0 items-center justify-center", outcome.tone)}>
          <Icon size={12} />
        </span>
      </Tooltip>
    </div>
  );
}

export function ExecutionLogSection({
  items,
  usage,
  harness,
  model,
  folded,
  onToggle,
}: {
  items: readonly TranscriptItem[];
  /** The thread's own totals, which are what the store says it spent. */
  usage?: SessionUsage;
  harness?: string;
  /** The thread's model, which names the model calls in a turn's waterfall. */
  model?: string;
  folded: boolean;
  onToggle: () => void;
}) {
  const turns = useMemo(() => turnEntries(items), [items]);
  const totals = useMemo(() => logTotals(turns, usage), [turns, usage]);
  const [all, setAll] = useState(false);
  // Newest first: the turn a reader is waiting on is the one at the top of the list.
  const shown = useMemo(() => {
    const latestFirst = [...turns].reverse();
    return all ? latestFirst : latestFirst.slice(0, LOG_ROWS);
  }, [turns, all]);
  // Each finished turn with the rows it ran, for the waterfall its row can open:
  // read when the log is expanded and not on every chunk of the turn it watches.
  const turnItems = useMemo(() => (all ? turnItemsByEnd(items) : NO_TURNS), [items, all]);
  const ends = useMemo(() => {
    const map = new Map<string, Extract<TranscriptItem, { kind: "turnEnd" }>>();
    for (const item of items) if (item.kind === "turnEnd") map.set(item.id, item);
    return map;
  }, [items]);

  const more = turns.length > LOG_ROWS;

  return (
    <OverviewSection
      id="log"
      title="Execution log"
      count={turns.length}
      actions={
        more ? (
          <SectionButton
            label={all ? `Show the last ${LOG_ROWS} turns` : `Show all ${turns.length} turns`}
            onClick={() => setAll(!all)}
          >
            {all ? <Minimize2 size={13} /> : <Maximize2 size={13} />}
          </SectionButton>
        ) : undefined
      }
      folded={folded}
      onToggle={onToggle}
    >
      {turns.length > 0 ? (
        <>
          <div className="grid grid-cols-3 gap-2 px-2 pb-1">
            <Stat
              label="Spent"
              value={totals.spent === undefined ? "—" : `${totals.spent.toFixed(2)} ${totals.currency}`}
            />
            <Stat label="Tokens" value={totals.tokens === undefined ? "—" : formatTokens(totals.tokens)} />
            <Stat label="Agent time" value={totals.agentMs > 0 ? formatDuration(totals.agentMs) : "—"} />
          </div>
          <TurnBars turns={turns} />
          <div className="pt-1">
            {shown.map((turn) => {
              const end = ends.get(turn.id);
              return (
                <TurnRow
                  key={turn.id}
                  turn={turn}
                  harness={harness}
                  trace={
                    all && end && turnHasTrace(turn) ? (
                      <TurnTrace end={end} items={turnItems[turn.id]} model={model} />
                    ) : undefined
                  }
                />
              );
            })}
          </div>
          {!all && more && (
            <p className="px-2 pt-0.5 text-caption text-fg-faint">
              {turns.length - shown.length} more behind the expand.
            </p>
          )}
        </>
      ) : (
        <p className="px-2 py-1 text-callout text-fg-muted">No turns yet.</p>
      )}
    </OverviewSection>
  );
}
