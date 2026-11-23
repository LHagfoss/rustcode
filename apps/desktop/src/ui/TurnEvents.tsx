// A turn's event log: every message, thought and tool call of one turn, in a list that reads like
// the transcript laid flat. A strip of cells across the top shows the turn's shape at a glance
// (one cell per event, coloured by kind; a press scrolls to it), and each row opens to the whole
// reply or the call's input and output. Opened from a turn in the side panel (ui/ThreadDetails.tsx);
// the Trace chip beside it still says where the time and money went (ui/TurnTrace.tsx).

import { Dialog } from "radix-ui";
import { type ReactNode, useMemo, useRef, useState } from "react";
import { toast } from "../state/store";
import { formatDuration, type TranscriptItem } from "../state/thread";
import {
  EVENT_FILTERS,
  type EventFilter,
  eventsText,
  keepEvent,
  type TurnEvent,
  type TurnEventKind,
  turnEvents,
} from "../state/turnEvents";
import { ChevronRight, Copy, X } from "./icons";
import { cx, Pill, Segments, Tooltip } from "./primitives";

/** Each kind's badge and strip cell: tool calls in the accent, the agent's words in green. */
const TONE: Record<TurnEventKind, { badge: string; cell: string }> = {
  agent: { badge: "bg-success/15 text-success", cell: "bg-success/70" },
  thinking: { badge: "bg-fill text-fg-muted", cell: "bg-fill-strong" },
  run: { badge: "bg-accent/15 text-accent", cell: "bg-accent/70" },
  read: { badge: "bg-fill-strong text-fg", cell: "bg-accent/35" },
  edit: { badge: "bg-warning/15 text-warning", cell: "bg-warning/70" },
  tool: { badge: "bg-fill-strong text-fg", cell: "bg-fg-faint" },
  failed: { badge: "bg-danger/15 text-danger", cell: "bg-danger/70" },
};

function clock(at: number | undefined): string {
  return at
    ? new Date(at).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" })
    : "";
}

function Chip({ children }: { children: ReactNode }) {
  return (
    <span className="inline-flex h-6 shrink-0 items-center gap-1 rounded-md border border-border px-2 text-caption text-fg-muted tabular">
      {children}
    </span>
  );
}

function EventRow({ event, index }: { event: TurnEvent; index: number }) {
  const [open, setOpen] = useState(false);
  const tone = TONE[event.kind];
  return (
    <li id={`turn-event-${event.id}`} className="border-border border-b last:border-b-0">
      <button
        type="button"
        aria-expanded={open}
        disabled={!event.body}
        onClick={() => setOpen(!open)}
        className="flex min-h-9 w-full items-center gap-2 px-3 text-left enabled:hover:bg-fill-subtle"
      >
        <span
          className={cx(
            "w-20 shrink-0 truncate rounded-md px-1.5 py-0.5 text-center font-medium text-caption",
            tone.badge,
          )}
        >
          {event.badge}
        </span>
        <ChevronRight
          size={12}
          className={cx(
            "shrink-0 text-fg-faint transition-transform",
            open && "rotate-90",
            !event.body && "invisible",
          )}
        />
        <span className="min-w-0 flex-1 truncate text-callout">{event.line}</span>
        <span className="shrink-0 text-caption text-fg-faint tabular">#{index + 1}</span>
        <span className="w-20 shrink-0 text-right text-caption text-fg-faint tabular">{clock(event.at)}</span>
      </button>
      {open && (
        <pre className="mx-3 mb-2 max-h-80 overflow-auto whitespace-pre-wrap break-words rounded-md bg-code-bg p-2 font-mono text-caption text-fg">
          {event.body}
        </pre>
      )}
    </li>
  );
}

export function TurnEvents({
  open,
  onOpenChange,
  title,
  items,
  elapsedMs,
  outcome,
  trace,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The message that opened the turn. */
  title: string;
  /** The turn's own rows, from its message to its footer. */
  items: readonly TranscriptItem[];
  elapsedMs?: number;
  outcome: "completed" | "cancelled" | "failed";
  /** The turn's Trace chip, beside the log's own controls. */
  trace?: ReactNode;
}) {
  const events = useMemo(() => turnEvents(items), [items]);
  const [newest, setNewest] = useState(false);
  const [filter, setFilter] = useState<EventFilter>("all");
  const list = useRef<HTMLOListElement>(null);
  const numbered = useMemo(() => events.map((event, index) => ({ event, index })), [events]);
  const shown = useMemo(() => {
    const kept = numbered.filter(({ event }) => keepEvent(event, filter));
    return newest ? kept.reverse() : kept;
  }, [numbered, filter, newest]);
  const tools = events.filter((event) => event.kind !== "agent" && event.kind !== "thinking").length;
  const start = items.find((item) => item.kind === "user");
  const startedAt = start?.kind === "user" ? start.at : undefined;

  const jump = (id: string) => {
    setFilter("all");
    requestAnimationFrame(() =>
      list.current?.querySelector(`[id="turn-event-${id}"]`)?.scrollIntoView({ block: "center" }),
    );
  };

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center p-6">
          <Dialog.Content
            aria-describedby={undefined}
            className="elevated pointer-events-auto flex max-h-full w-[min(920px,100%)] flex-col overflow-hidden"
          >
            <div className="flex flex-col gap-2 border-border border-b p-3">
              <div className="flex items-center gap-2">
                <Dialog.Title className="min-w-0 flex-1 truncate font-semibold text-body">
                  {title}
                </Dialog.Title>
                <span
                  className={cx(
                    "shrink-0 rounded-full px-2 py-0.5 text-caption",
                    outcome === "completed"
                      ? "bg-success/15 text-success"
                      : outcome === "failed"
                        ? "bg-danger/15 text-danger"
                        : "bg-fill text-fg-muted",
                  )}
                >
                  {outcome === "completed" ? "Completed" : outcome === "failed" ? "Failed" : "Cancelled"}
                </span>
                <Dialog.Close asChild>
                  <button
                    type="button"
                    aria-label="Close"
                    className="flex size-7 shrink-0 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg"
                  >
                    <X size={14} />
                  </button>
                </Dialog.Close>
              </div>
              <div className="flex flex-wrap items-center gap-1.5">
                {elapsedMs !== undefined && <Chip>{formatDuration(elapsedMs)}</Chip>}
                <Chip>
                  {tools} tool call{tools === 1 ? "" : "s"}
                </Chip>
                <Chip>
                  {events.length} event{events.length === 1 ? "" : "s"}
                </Chip>
                {startedAt && <Chip>{new Date(startedAt).toLocaleString()}</Chip>}
                <span className="flex-1" />
                {trace}
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <Segments
                  label="Show"
                  value={filter}
                  onChange={setFilter}
                  options={EVENT_FILTERS.map((f) => ({ value: f.id, label: f.label }))}
                />
                <span className="flex-1" />
                <Segments
                  label="Order"
                  value={newest ? "newest" : "oldest"}
                  onChange={(v) => setNewest(v === "newest")}
                  options={[
                    { value: "oldest", label: "Oldest first" },
                    { value: "newest", label: "Newest first" },
                  ]}
                />
                <Pill
                  quiet
                  size="sm"
                  onClick={() =>
                    void navigator.clipboard
                      .writeText(eventsText(events))
                      .then(() => toast("Turn copied", "info"))
                  }
                >
                  <Copy size={12} />
                  Copy all
                </Pill>
              </div>
              {events.length > 1 && (
                <div className="flex h-4 gap-0.5" aria-hidden>
                  {events.map((event) => (
                    <Tooltip key={event.id} label={`${event.badge} · ${event.line}`}>
                      <button
                        type="button"
                        tabIndex={-1}
                        onClick={() => jump(event.id)}
                        className={cx("min-w-0 flex-1 rounded-sm hover:opacity-80", TONE[event.kind].cell)}
                      />
                    </Tooltip>
                  ))}
                </div>
              )}
            </div>
            <ol ref={list} className="min-h-0 flex-1 overflow-y-auto">
              {shown.map(({ event, index }) => (
                <EventRow key={event.id} event={event} index={index} />
              ))}
              {shown.length === 0 && (
                <li className="px-3 py-6 text-center text-callout text-fg-muted">
                  Nothing of this kind in the turn.
                </li>
              )}
            </ol>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
