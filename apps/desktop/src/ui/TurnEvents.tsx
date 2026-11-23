import { Dialog } from "radix-ui";
import { Fragment, type ReactNode, useMemo, useRef, useState } from "react";
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
import { ChevronRight, CircleCheck, CircleSlash, CircleX, Copy, X } from "./icons";
import { cx, Pill, Segments, Tooltip } from "./primitives";

/** One turn of the log: the message that opened it, how it ended, and its rows. */
export interface LogTurn {
  id: string;
  prompt: string;
  outcome: "completed" | "cancelled" | "failed";
  elapsedMs?: number;
  items: readonly TranscriptItem[];
}

/** Each kind's badge and strip cell: commands in the accent, the agent's words in green. */
const TONE: Record<TurnEventKind, { badge: string; cell: string }> = {
  agent: { badge: "bg-success/15 text-success", cell: "bg-success/70" },
  thinking: { badge: "bg-fill text-fg-muted", cell: "bg-fill-strong" },
  run: { badge: "bg-accent/15 text-accent", cell: "bg-accent/70" },
  read: { badge: "bg-fill text-fg", cell: "bg-accent/35" },
  edit: { badge: "bg-warning/15 text-warning", cell: "bg-warning/70" },
  tool: { badge: "bg-fill text-fg", cell: "bg-fg-faint" },
  failed: { badge: "bg-danger/15 text-danger", cell: "bg-danger/70" },
};

const OUTCOME = {
  completed: { icon: CircleCheck, tone: "text-success", label: "Completed" },
  cancelled: { icon: CircleSlash, tone: "text-fg-muted", label: "Cancelled" },
  failed: { icon: CircleX, tone: "text-danger", label: "Failed" },
} as const;

function clock(at: number | undefined): string {
  return at
    ? new Date(at).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" })
    : "";
}

function plural(count: number, word: string): string {
  return `${count} ${word}${count === 1 ? "" : "s"}`;
}

/** A block of an opened row: what it is, quietly, over the text itself. */
function Block({ label, text }: { label?: string; text: string }) {
  return (
    <div className="flex min-w-0 flex-col gap-1">
      {label && <span className="font-medium text-caption text-fg-faint">{label}</span>}
      <pre className="max-h-72 overflow-auto whitespace-pre-wrap break-words rounded-md bg-code-bg px-3 py-2 font-mono text-caption text-fg">
        {text}
      </pre>
    </div>
  );
}

function EventRow({ event, index }: { event: TurnEvent; index: number }) {
  const [open, setOpen] = useState(false);
  const tool = event.input !== undefined || event.output !== undefined;
  return (
    <div id={`turn-event-${event.id}`}>
      <button
        type="button"
        aria-expanded={open}
        disabled={!event.body}
        onClick={() => setOpen(!open)}
        className="flex min-h-10 w-full items-center gap-3 px-4 text-left enabled:hover:bg-fill-subtle"
      >
        <span
          className={cx(
            "w-16 shrink-0 truncate rounded-sm px-1.5 py-0.5 text-center font-medium text-caption",
            TONE[event.kind].badge,
          )}
        >
          {event.badge}
        </span>
        <span className="min-w-0 flex-1 truncate text-body">{event.line}</span>
        <span className="shrink-0 text-caption text-fg-faint tabular">
          #{index + 1} · {clock(event.at)}
        </span>
        <ChevronRight
          size={12}
          className={cx(
            "shrink-0 text-fg-faint transition-transform",
            open && "rotate-90",
            !event.body && "invisible",
          )}
        />
      </button>
      {open && (
        <div className="flex flex-col gap-2 px-4 pb-3">
          {tool ? (
            <>
              {event.input && <Block label="Input" text={event.input} />}
              {event.output && <Block label="Output" text={event.output} />}
            </>
          ) : (
            <Block text={event.body} />
          )}
        </div>
      )}
    </div>
  );
}

/**
 * The event log: every message, thought and tool call of a turn (or of every turn), laid flat in
 * the order they happened, a reading of the transcript without its layout. A thin strip across the
 * top shows the run's shape at a glance (one cell per event, coloured by kind; a press scrolls to
 * it), and each row opens to the whole reply or the call's input and output. Opened from Turns in
 * the side panel (ui/ThreadDetails.tsx); the Trace chip in the header still says where the time
 * and money went (ui/TurnTrace.tsx).
 */
export function TurnEvents({
  open,
  onOpenChange,
  title,
  turns,
  trace,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  /** One turn, or every turn of the thread oldest first; more than one gets a header for each. */
  turns: readonly LogTurn[];
  /** The turn's Trace chip, in the header. */
  trace?: ReactNode;
}) {
  const [newest, setNewest] = useState(false);
  const [filter, setFilter] = useState<EventFilter>("all");
  const list = useRef<HTMLDivElement>(null);
  const single = turns.length === 1 ? turns[0] : undefined;

  const groups = useMemo(() => {
    let n = 0;
    return turns.map((turn, at) => ({
      turn,
      number: at + 1,
      events: turnEvents(turn.items).map((event) => ({ event, index: n++ })),
    }));
  }, [turns]);
  const all = useMemo(() => groups.flatMap((group) => group.events.map(({ event }) => event)), [groups]);
  const shown = useMemo(() => {
    const kept = groups
      .map((group) => {
        const events = group.events.filter(({ event }) => keepEvent(event, filter));
        return { ...group, events: newest ? events.reverse() : events };
      })
      .filter((group) => group.events.length > 0);
    return newest ? kept.reverse() : kept;
  }, [groups, filter, newest]);

  const tools = all.filter((event) => !keepEvent(event, "agent")).length;
  const elapsed = turns.reduce((sum, turn) => sum + (turn.elapsedMs ?? 0), 0);
  const firstMessage = turns[0]?.items.find((item) => item.kind === "user");
  const startedAt = firstMessage?.kind === "user" ? firstMessage.at : undefined;
  const outcome = single ? OUTCOME[single.outcome] : undefined;

  const jump = (id: string) => {
    setFilter("all");
    requestAnimationFrame(() =>
      list.current?.querySelector(`[id="turn-event-${id}"]`)?.scrollIntoView({ block: "center" }),
    );
  };

  const meta = [
    turns.length > 1 ? plural(turns.length, "turn") : undefined,
    elapsed > 0 ? formatDuration(elapsed) : undefined,
    plural(tools, "tool call"),
    plural(all.length, "event"),
    startedAt
      ? new Date(startedAt).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })
      : undefined,
  ].filter((part): part is string => !!part);

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center p-8">
          <Dialog.Content
            aria-describedby={undefined}
            className="elevated pointer-events-auto flex max-h-[min(760px,100%)] w-[min(880px,100%)] flex-col overflow-hidden"
          >
            <div className="flex flex-col gap-3 px-4 pt-4 pb-3">
              <div className="flex items-start gap-3">
                <div className="flex min-w-0 flex-1 flex-col gap-1">
                  <Dialog.Title className="truncate font-semibold text-title3">{title}</Dialog.Title>
                  <p className="flex min-w-0 items-center gap-1.5 text-callout text-fg-muted tabular">
                    {outcome && (
                      <span className={cx("flex shrink-0 items-center gap-1", outcome.tone)}>
                        <outcome.icon size={13} />
                        {outcome.label}
                      </span>
                    )}
                    {meta.map((part, i) => (
                      <Fragment key={part}>
                        {(i > 0 || outcome) && <span className="text-fg-faint">·</span>}
                        <span className="truncate">{part}</span>
                      </Fragment>
                    ))}
                  </p>
                </div>
                {trace}
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
                      .writeText(eventsText(all))
                      .then(() => toast("Event log copied", "info"))
                  }
                >
                  <Copy size={12} />
                  Copy all
                </Pill>
              </div>
              {all.length > 1 && (
                <div className="flex h-1.5 gap-px">
                  {all.map((event) => (
                    <Tooltip key={event.id} label={`${event.badge} · ${event.line}`}>
                      <button
                        type="button"
                        tabIndex={-1}
                        aria-label={`${event.badge}: ${event.line}`}
                        onClick={() => jump(event.id)}
                        className={cx("min-w-0 flex-1 rounded-full hover:opacity-70", TONE[event.kind].cell)}
                      />
                    </Tooltip>
                  ))}
                </div>
              )}
            </div>
            <div ref={list} className="min-h-0 flex-1 overflow-y-auto border-border border-t pb-2">
              {shown.map((group) => (
                <section key={group.turn.id} aria-label={single ? undefined : `Turn ${group.number}`}>
                  {!single && (
                    <div className="sticky top-0 z-10 flex h-8 items-center gap-2 bg-surface px-4 text-sub">
                      <span className="shrink-0 font-medium text-fg-muted">Turn {group.number}</span>
                      <span className="min-w-0 flex-1 truncate text-fg-faint">{group.turn.prompt}</span>
                      {group.turn.elapsedMs !== undefined && (
                        <span className="shrink-0 text-fg-faint tabular">
                          {formatDuration(group.turn.elapsedMs)}
                        </span>
                      )}
                    </div>
                  )}
                  <div className="rule-rows [--rc-rule-inset:1rem]">
                    {group.events.map(({ event, index }) => (
                      <EventRow key={event.id} event={event} index={index} />
                    ))}
                  </div>
                </section>
              ))}
              {shown.length === 0 && (
                <p className="px-4 py-10 text-center text-callout text-fg-muted">
                  {all.length ? "Nothing of this kind here." : "No events recorded for this turn."}
                </p>
              )}
            </div>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
