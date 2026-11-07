// The plan a thread is working to, as a card at the foot of the transcript: what is left, what is
// done, and one way to carry on. The header's popover and the composer's meter keep their own
// compact view (ChatView.tsx, ComposerUsage.tsx) — this is the one you read in the chat.
//
// While the turn works through the plan the card is live: the step it is on carries a running
// clock, and a steering note the turn read on that step is marked under it (`ui/SteerMark.tsx`), so
// an instruction sent mid-turn says which step it changed. Each finished step keeps its own time.

import { ChevronDown, ChevronUp, ListChecks } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { PlanEntry } from "../acp/types";
import { type PlanClocks, planClock, type SteerNote, steerMarks, stepElapsed } from "../state/steer";
import { runPrompt } from "../state/store";
import { formatDuration } from "../state/thread";
import { useNow } from "./AgentViews";
import { CopyButton } from "./Markdown";
import { cx, Pill, Tooltip } from "./primitives";
import { SteerMark } from "./SteerMark";

/** How far the card shows before it fades: the rest is a click away. */
const SHOWN = 220;
/** The fade under the last line, so a cut-off list reads as continued, not as finished. */
const FADE = 24;

/** The plan as text, for the clipboard: `- [x] done`, `- [ ] to do`. */
export function planText(entries: PlanEntry[]): string {
  return entries.map((e) => `- [${e.status === "completed" ? "x" : " "}] ${e.content}`).join("\n");
}

/**
 * A thread's plan: every step with its state, the clock it ran by, Copy, and Implement — which
 * sends the one message a plan asks for, so it is only there while the plan waits on you. The card
 * is drawn while a turn works through the plan (live: the step on now carries a running clock and
 * any note read on it) and once the thread waits on you with something left. A plan that is all
 * done still shows where a steering note landed, and nothing else.
 */
export function PlanCard({
  threadId,
  entries,
  running,
  steers,
  clocks,
}: {
  threadId: string;
  entries: PlanEntry[];
  /** The turn is working through this plan now: the step it is on is clocked. */
  running: boolean;
  /** The steering notes this thread's turns read, marked on the steps they landed on. */
  steers: SteerNote[];
  clocks: PlanClocks;
}) {
  const [open, setOpen] = useState(false);
  const [clipped, setClipped] = useState(false);
  const body = useRef<HTMLDivElement>(null);
  const text = useMemo(() => planText(entries), [entries]);
  const left = entries.filter((e) => e.status !== "completed").length;
  const working = running && entries.some((e) => e.status === "in_progress");
  const now = useNow(working);
  const marks = useMemo(() => steerMarks(steers, entries), [steers, entries]);

  // Whether the list runs past what the card shows. Measured while it is clamped: unclamped, the
  // content is exactly what is shown, and the answer would always be no. A ResizeObserver keeps
  // the answer true as the plan changes under the reader.
  useEffect(() => {
    if (open) return;
    const el = body.current;
    if (!el) return;
    const measure = () => setClipped(el.scrollHeight > SHOWN + FADE);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [open]);

  return (
    <section aria-label="Plan" className="rounded-2xl bg-surface p-4">
      <div className="flex flex-wrap items-center gap-2">
        <span className="flex min-w-0 flex-1 items-center gap-2">
          <ListChecks size={15} className="shrink-0 text-fg-muted" />
          <span className="shrink-0 font-semibold text-body">Plan</span>
          <span className="min-w-0 truncate text-callout text-fg-muted tabular">
            {left ? `${left} of ${entries.length} left` : "all done"}
          </span>
        </span>
        <span className="flex shrink-0 items-center gap-1">
          <CopyButton text={text} iconOnly label="Copy plan" />
          {left > 0 && !running && (
            <Pill prominent size="sm" onClick={() => void runPrompt(threadId, "Implement the plan.")}>
              Implement
            </Pill>
          )}
        </span>
      </div>
      <div
        ref={body}
        className={cx("mt-3 flex flex-col gap-1 overflow-hidden", !open && clipped && "max-h-[220px]")}
        style={
          open || !clipped
            ? undefined
            : {
                // design-lint: allow (mask alpha, not a colour)
                maskImage: `linear-gradient(to bottom, #000 calc(100% - ${FADE}px), transparent)`,
                // design-lint: allow (mask alpha, not a colour)
                WebkitMaskImage: `linear-gradient(to bottom, #000 calc(100% - ${FADE}px), transparent)`,
              }
        }
      >
        {entries.map((entry, i) => {
          const elapsed = stepElapsed(entry, planClock(clocks, entry.content), now, running);
          const notes = marks.byStep.get(entry.content);
          return (
            <div key={`${i}-${entry.content}`} className="flex min-w-0 flex-col">
              <div className="flex min-w-0 items-start gap-2 text-body">
                <span
                  aria-hidden
                  className={cx(
                    "mt-[7px] size-1.5 shrink-0 rounded-full",
                    entry.status === "completed" && "bg-success",
                    entry.status === "in_progress" && "bg-accent",
                    entry.status === "pending" && "bg-fill-strong",
                  )}
                />
                <span
                  className={cx(
                    "min-w-0 flex-1 break-words",
                    entry.status === "completed" ? "text-fg-muted" : "text-fg",
                  )}
                >
                  {entry.content}
                </span>
                {elapsed !== undefined && (
                  <Tooltip
                    label={entry.status === "completed" ? "How long this step took" : "This step so far"}
                  >
                    <span className="shrink-0 text-caption text-fg-faint tabular">
                      {formatDuration(elapsed)}
                    </span>
                  </Tooltip>
                )}
              </div>
              {/* What the turn was told while it was on this step: the note it read here. */}
              {notes && (
                <div className="pl-[14px]">
                  {notes.map((note) => (
                    <SteerMark key={note.id} note={note} inPlan />
                  ))}
                </div>
              )}
            </div>
          );
        })}
      </div>
      {clipped && (
        <div className="mt-2 flex justify-center">
          <Tooltip label={open ? "Show the plan's start" : "Show the whole plan"}>
            <Pill size="sm" quiet onClick={() => setOpen(!open)}>
              {open ? "Collapse" : `Expand · ${entries.length} steps`}
              {open ? <ChevronUp size={11} /> : <ChevronDown size={11} />}
            </Pill>
          </Tooltip>
        </div>
      )}
    </section>
  );
}
