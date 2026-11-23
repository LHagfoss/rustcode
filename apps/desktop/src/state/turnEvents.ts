import { subagentOf, type TranscriptItem } from "./thread";
import { stripHarnessMarkup } from "./threadFormat";
import { displayTool, usefulSummary } from "./toolNames";

/**
 * A turn as a list of events, for the turn's event log (ui/TurnEvents.tsx): what the agent said,
 * what it thought and each tool call, in the order they happened, each with a short line for its
 * row and the whole text for when the row is opened. The trace (lib/trace.ts) says where the time
 * went; this says what was done.
 */
/** What an event is, which picks its badge's colour and the filter it falls under. */
export type TurnEventKind = "agent" | "thinking" | "run" | "read" | "edit" | "tool" | "failed";

export interface TurnEvent {
  id: string;
  kind: TurnEventKind;
  /** The badge: the tool's own name (`Bash`, `Read`), `Agent` or `Thinking`. */
  badge: string;
  /** One line for the row. */
  line: string;
  /** Everything there is to read when the row opens: the reply, or the call's input and output. */
  body: string;
  /** A tool call's arguments and what came back, apart, for the opened row's two blocks. */
  input?: string;
  output?: string;
  at?: number;
}

/** The filters the log offers, in order, with what each keeps. */
export const EVENT_FILTERS: { id: "all" | "agent" | "tools" | "failed"; label: string }[] = [
  { id: "all", label: "All" },
  { id: "agent", label: "Messages" },
  { id: "tools", label: "Tool calls" },
  { id: "failed", label: "Failed" },
];
export type EventFilter = (typeof EVENT_FILTERS)[number]["id"];

export function keepEvent(event: TurnEvent, filter: EventFilter): boolean {
  if (filter === "all") return true;
  if (filter === "agent") return event.kind === "agent" || event.kind === "thinking";
  if (filter === "failed") return event.kind === "failed";
  return event.kind !== "agent" && event.kind !== "thinking";
}

type ToolItem = Extract<TranscriptItem, { kind: "tool" }>;

/** The tool's kind as the harness reports it, folded into the few the log colours apart. */
function toolKindOf(item: ToolItem): TurnEventKind {
  if (item.status === "failed") return "failed";
  switch (item.toolKind) {
    case "execute":
      return "run";
    case "read":
    case "search":
    case "fetch":
      return "read";
    case "edit":
    case "delete":
    case "move":
      return "edit";
    default:
      return "tool";
  }
}

/** The first line with text, without the markdown that only means something rendered. */
function firstLine(text: string): string {
  const line =
    text
      .split("\n")
      .find((part) => part.trim())
      ?.trim() ?? "";
  return line.replace(/\*\*|__|`/g, "").replace(/^#+\s+/, "");
}

function capitalized(word: string): string {
  return word ? word[0]!.toUpperCase() + word.slice(1) : word;
}

/** A call's arguments and what came back, each as text, or unset when there is none. */
function callParts(item: ToolItem): { input?: string; output?: string } {
  const input =
    item.input === undefined || item.input === null
      ? undefined
      : typeof item.input === "string"
        ? item.input
        : JSON.stringify(item.input, null, 2);
  const output = (item.output ?? item.detail)?.trim() || undefined;
  return { input: input?.trim() || undefined, output };
}

/** The rows of one turn (state/fold.ts `turnItemsByEnd`) as its events, oldest first. */
export function turnEvents(items: readonly TranscriptItem[]): TurnEvent[] {
  const events: TurnEvent[] = [];
  for (const item of items) {
    if (item.kind === "assistant") {
      const text = stripHarnessMarkup(item.text).trim();
      if (!text) continue;
      events.push({
        id: item.id,
        kind: "agent",
        badge: "Agent",
        line: firstLine(text),
        body: text,
        at: item.at,
      });
    } else if (item.kind === "thought") {
      const text = item.text.trim();
      if (!text) continue;
      events.push({
        id: item.id,
        kind: "thinking",
        badge: "Thinking",
        line: firstLine(text),
        body: text,
        at: item.startedAt,
      });
    } else if (item.kind === "tool") {
      const agent = subagentOf(item);
      const shown = displayTool(item.title, item.input, item.status);
      const badge = agent
        ? "Agent"
        : (item.toolName ?? (item.toolKind ? capitalized(item.toolKind) : "Tool"));
      const line =
        agent?.task ??
        shown.summary ??
        usefulSummary(item.summary) ??
        firstLine(item.detail ?? "") ??
        shown.title;
      const { input, output } = callParts(item);
      events.push({
        id: item.id,
        kind: toolKindOf(item),
        badge,
        line: line || shown.title,
        body: [input, output].filter(Boolean).join("\n\n"),
        input,
        output,
        at: item.startedAt,
      });
    }
  }
  return events;
}

/** The whole log as text, for Copy all: one block per event. */
export function eventsText(events: readonly TurnEvent[]): string {
  return events
    .map((event, i) => `#${i + 1} ${event.badge}: ${event.line}\n${event.body}`.trim())
    .join("\n\n");
}
