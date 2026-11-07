// The Trace chip under a finished turn: where its time and money went
// (docs/design.md › Turn trace). One row per model call, tool call and
// delegate, bars on the turn's own timeline, the slowest marked, and each row
// opening to the call's arguments and output. The numbers come from what the
// engine reported while the turn ran (state/thread.ts `TurnUsage`); a replayed
// turn has no per-call timing, so it has no chip rather than invented bars.

import { Activity, Brain, CircleX, Users, Wrench } from "lucide-react";
import { Popover } from "radix-ui";
import { useMemo, useState } from "react";
import {
  barSegments,
  buildTrace,
  hasTimeline,
  middleClip,
  rowValue,
  sortTrace,
  type Trace,
  type TraceCall,
  type TraceMeasure,
  type TraceRow,
  type TraceSort,
  type TraceText,
  waitedMs,
  workDuration,
} from "../lib/trace";
import { formatDuration, formatTokens, subagentOf, type TranscriptItem } from "../state/thread";
import { displayTool, orchestrationTool, usefulSummary } from "../state/toolNames";
import { CopyButton } from "./Markdown";
import { cx, Segments, Tooltip } from "./primitives";
import { ToolDetail } from "./Rows";

type ToolItem = Extract<TranscriptItem, { kind: "tool" }>;
type TurnEnd = Extract<TranscriptItem, { kind: "turnEnd" }>;

/** Calls that reach another thread, or start a subagent: one folded row. */
function isDelegate(item: ToolItem): boolean {
  if (subagentOf(item)) return true;
  const called = orchestrationTool(item.title);
  return called === "start_session" || called === "fan_out";
}

/** A tool call as the trace reads it, in the tool row's own words. */
function traceCall(item: ToolItem): TraceCall {
  const agent = subagentOf(item);
  const shown = displayTool(item.title, item.input, item.status);
  return {
    id: item.id,
    label: agent?.name ?? shown.title,
    detail: agent?.task ?? shown.summary ?? usefulSummary(item.summary) ?? item.detail,
    status: item.status === "failed" ? "failed" : item.status === "completed" ? "completed" : "running",
    startedAt: item.startedAt ?? 0,
    endedAt: item.endedAt,
    delegate: isDelegate(item),
    waits: item.waits,
  };
}

/** A finished turn's Trace chip: the waterfall opens above it. */
export function TurnTrace({
  end,
  items,
  model,
}: {
  end: TurnEnd;
  /** The turn's transcript rows, from the message that started it to its footer. */
  items?: TranscriptItem[];
  /** The thread's model, which names its model calls. */
  model?: string;
}) {
  if (!hasTimeline(end)) return null;
  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <button
          type="button"
          className="flex h-[20px] items-center gap-1 rounded-full bg-fill-subtle px-2 font-medium text-caption text-fg-muted hover:bg-fill hover:text-fg data-[state=open]:bg-fill-strong"
        >
          <Activity size={11} />
          Trace
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          side="top"
          align="start"
          sideOffset={6}
          aria-label="Turn trace"
          className="elevated z-40 flex max-h-[min(560px,75vh)] w-[min(640px,85vw)] flex-col gap-2 p-3 outline-none"
        >
          <TracePanel end={end} items={items ?? []} model={model} />
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

function TracePanel({ end, items, model }: { end: TurnEnd; items: TranscriptItem[]; model?: string }) {
  const [sort, setSort] = useState<TraceSort>("timeline");
  // Each tool call by its id, for a row's expanded detail. A delegate's own
  // steps stay out of the timeline and live in its card, as in the transcript.
  const tools = useMemo(() => {
    const map = new Map<string, ToolItem>();
    for (const item of items) if (item.kind === "tool") map.set(item.id, item);
    return map;
  }, [items]);
  const trace = useMemo(() => {
    const calls: TraceCall[] = [];
    const text: TraceText[] = [];
    for (const item of items) {
      if (item.kind === "tool" && !item.parent && item.startedAt !== undefined) {
        calls.push(traceCall(item));
      } else if (item.kind === "assistant" && item.at !== undefined) {
        text.push({ at: item.at, kind: "reply", text: item.text });
      } else if (item.kind === "thought") {
        text.push({ at: item.startedAt, kind: "thought", text: item.text });
      }
    }
    return buildTrace({
      start: end.startedAt!,
      end: end.at!,
      calls,
      text,
      before: end.usage?.before,
      samples: end.usage?.samples,
      model,
    });
  }, [items, end, model]);
  // What the rows show, and what the third sort orders by. Cost first: the
  // money is what the chip is for. A harness that reported none (Claude Code,
  // Pi) leaves tokens, so the panel opens on them instead of a column of dashes.
  const [measure, setMeasure] = useState<TraceMeasure>(() =>
    trace.total?.cost !== undefined || trace.rows.some((row) => row.usage?.cost !== undefined)
      ? "cost"
      : "tokens",
  );
  const rows = useMemo(() => sortTrace(trace.rows, sort, measure), [trace, sort, measure]);
  const [open, setOpen] = useState<ReadonlySet<string>>(() => new Set());
  const toggle = (id: string) =>
    setOpen((was) => {
      const next = new Set(was);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const total =
    measure === "cost"
      ? trace.total?.cost !== undefined
        ? usd(trace.total.cost)
        : undefined
      : trace.total !== undefined
        ? `${formatTokens((trace.total.inputTokens ?? 0) + (trace.total.outputTokens ?? 0))} tokens`
        : undefined;
  return (
    <>
      <div className="flex items-center gap-2 px-1">
        <span className="font-semibold text-body">Trace</span>
        <span className="text-callout text-fg-muted tabular">{formatDuration(trace.durationMs)}</span>
        <span className="flex-1" />
        <span
          className={cx("text-callout tabular", total ? "text-fg-muted" : "text-fg-faint")}
          title={
            total
              ? "Reported by the harness for this turn"
              : "The harness reported no token counts or cost for this turn"
          }
        >
          {total ?? "No usage reported"}
        </span>
      </div>
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 px-1">
        <span className="text-caption text-fg-faint">Sort</span>
        <Segments
          value={sort}
          onChange={setSort}
          label="Order the turn's calls"
          options={[
            { value: "timeline", label: "Timeline" },
            { value: "duration", label: "Duration" },
            { value: "cost", label: measure === "cost" ? "Cost" : "Tokens" },
          ]}
        />
        <span className="flex-1" />
        <span className="text-caption text-fg-faint">Show</span>
        <Segments
          value={measure}
          onChange={setMeasure}
          label="Show cost or tokens"
          options={[
            { value: "cost", label: "Cost" },
            { value: "tokens", label: "Tokens" },
          ]}
        />
      </div>
      <div className="flex items-center gap-2 px-1" aria-hidden>
        <span className="w-[136px] shrink-0" />
        <span className="flex flex-1 justify-between text-caption text-fg-faint tabular">
          <span>0s</span>
          <span>{formatDuration(trace.durationMs)}</span>
        </span>
        <span className="w-[92px] shrink-0" />
      </div>
      <div className="flex min-h-0 flex-col overflow-y-auto">
        {rows.map((row) => (
          <TraceRowView
            key={row.id}
            row={row}
            trace={trace}
            measure={measure}
            slowest={row.id === trace.slowest}
            tools={tools}
            open={open}
            onToggle={toggle}
          />
        ))}
      </div>
      {trace.unpriced > 0 && (
        <p className="px-1 text-caption text-fg-faint">
          {trace.unpriced === 1
            ? "One model call reported no tokens or cost"
            : `${trace.unpriced} model calls reported no tokens or cost`}
          {trace.total ? "; the total is the turn's own." : "."}
        </p>
      )}
    </>
  );
}

function TraceRowView({
  row,
  trace,
  measure,
  slowest,
  tools,
  open,
  onToggle,
  nested,
}: {
  row: TraceRow;
  trace: Trace;
  measure: TraceMeasure;
  slowest?: boolean;
  tools: ReadonlyMap<string, ToolItem>;
  open: ReadonlySet<string>;
  onToggle: (id: string) => void;
  /** A delegate's own call, under the folded row. */
  nested?: boolean;
}) {
  const isOpen = open.has(row.id);
  const bars = barSegments(row, trace);
  const duration = workDuration(row);
  const waiting = waitedMs(row);
  const value = rowValue(row, measure);
  const Icon = row.kind === "delegates" ? Users : row.kind === "model" ? Brain : Wrench;
  const tool = row.toolId ? tools.get(row.toolId) : undefined;
  const detail = row.kind === "delegates" ? row.children : undefined;
  const expandable = !!detail?.length || !!tool || !!row.text;
  const content = (
    <>
      <span className="flex w-[184px] min-w-0 shrink-0 items-center gap-1.5">
        <span className="flex w-[14px] shrink-0 justify-center">
          {row.status === "failed" ? (
            <CircleX size={13} className="text-danger" />
          ) : (
            <Icon size={13} className="text-fg-muted" />
          )}
        </span>
        {/* The bar keeps its room: a long path gives way in its middle, not its end. */}
        <span className="truncate text-callout text-fg" title={row.label}>
          {middleClip(row.label, 30)}
        </span>
        {slowest && (
          <span className="shrink-0 rounded-full bg-fill px-1 text-caption text-fg-muted">Slowest</span>
        )}
      </span>
      <span className="relative h-[18px] flex-1">
        <span className="absolute inset-0 rounded-xs bg-fill-subtle" />
        {/* Hovering the bar, not the row, names the call: the row's label and
            numbers are already on screen, and the bar is what is being read. A
            wait on the user is hatched, so it is not read as the call's work. */}
        <Tooltip label={rowTip(row, trace, measure)}>
          <span className="absolute inset-0">
            {bars.map((bar, i) => (
              <span
                key={i}
                className={cx(
                  "absolute inset-y-[4px] rounded-xs",
                  bar.kind === "wait"
                    ? "rc-wait-hatch"
                    : row.kind === "model"
                      ? "bg-accent/55 group-hover:bg-accent/80"
                      : row.kind === "delegates"
                        ? "border border-accent/50 border-dashed bg-accent/10"
                        : "bg-fill-strong group-hover:bg-fg-faint",
                )}
                style={{ left: `${bar.left}%`, width: `${bar.width}%`, minWidth: 3 }}
              />
            ))}
          </span>
        </Tooltip>
      </span>
      <span className="flex w-[122px] shrink-0 items-center justify-end gap-1 whitespace-nowrap text-caption text-fg-muted tabular">
        {value !== undefined && <span>{measure === "cost" ? usd(value) : formatTokens(value)}</span>}
        {value !== undefined && <span className="text-fg-faint">·</span>}
        <span className={value === undefined ? "text-fg-faint" : undefined}>{formatDuration(duration)}</span>
        {waiting > 0 && <span className="text-fg-faint">{`+${formatDuration(waiting)} waiting`}</span>}
      </span>
    </>
  );
  const rowClass = cx(
    "group flex w-full items-center gap-2 rounded-md px-1 py-1 text-left",
    expandable && "hover:bg-fill-subtle",
    nested && "pl-6",
  );
  return (
    <div>
      {expandable ? (
        <button type="button" aria-expanded={isOpen} onClick={() => onToggle(row.id)} className={rowClass}>
          {content}
        </button>
      ) : (
        <div className={rowClass}>{content}</div>
      )}
      {isOpen && (
        <div className="rc-expand mt-1 mb-1.5 ml-[22px] flex flex-col gap-1">
          {detail?.map((child) => (
            <TraceRowView
              key={child.id}
              row={child}
              trace={trace}
              measure={measure}
              tools={tools}
              open={open}
              onToggle={onToggle}
              nested
            />
          ))}
          {tool && <ToolDetail item={tool} className="pl-1" />}
          {!tool && row.text && (
            <div className="flex items-start gap-2">
              <pre className="selectable m-0 max-h-[240px] min-w-0 flex-1 overflow-auto rounded-md bg-fill-subtle p-2.5 font-mono text-caption leading-[15px] whitespace-pre-wrap break-words">
                {row.text}
              </pre>
              <CopyButton text={row.text} iconOnly label="Copy the call's text" />
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/** Hovering a row names the call, when it ran, and what it used. */
function rowTip(row: TraceRow, trace: Trace, measure: TraceMeasure) {
  const tokens =
    row.usage && (row.usage.inputTokens !== undefined || row.usage.outputTokens !== undefined)
      ? `${formatTokens(row.usage.inputTokens ?? 0)} in · ${formatTokens(row.usage.outputTokens ?? 0)} out${
          row.usage.cachedTokens ? ` · ${formatTokens(row.usage.cachedTokens)} cached` : ""
        }`
      : undefined;
  return (
    <span className="flex flex-col gap-0.5">
      <span className="font-medium">{row.label}</span>
      {row.detail && <span className="max-w-72 truncate text-fg-muted">{row.detail}</span>}
      <span className="tabular">
        {formatDuration(row.startedAt - trace.start)} in · {formatDuration(workDuration(row))} work
        {waitedMs(row) > 0 ? ` + ${formatDuration(waitedMs(row))} waiting on you` : ""}
        {row.status === "running" ? " · running" : ""}
        {row.status === "failed" ? " · failed" : ""}
      </span>
      {tokens && <span className="tabular">{tokens}</span>}
      {row.usage?.cost !== undefined && <span className="tabular">{usd(row.usage.cost)}</span>}
      {row.kind === "delegates" && <span className="text-fg-muted">Click to open each one</span>}
      {measure === "cost" && row.usage?.cost === undefined && row.kind === "model" && (
        <span className="text-fg-muted">No cost reported for this call</span>
      )}
    </span>
  );
}

/** A USD figure, four places: a single round costs cents, not dollars. */
function usd(value: number): string {
  if (value === 0) return "$0";
  return value < 0.0001 ? "<$0.0001" : `$${value.toFixed(4)}`;
}
