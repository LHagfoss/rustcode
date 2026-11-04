// The plan a thread is working to, as a card at the foot of the transcript: what is left, what is
// done, and one way to carry on. The header's popover and the composer's meter keep their own
// compact view (ChatView.tsx, ComposerUsage.tsx) — this is the one you read in the chat.

import { ChevronDown, ChevronUp, ListChecks } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { PlanEntry } from "../acp/types";
import { runPrompt } from "../state/store";
import { CopyButton } from "./Markdown";
import { cx, Pill, Tooltip } from "./primitives";

/** How far the card shows before it fades: the rest is a click away. */
const SHOWN = 220;
/** The fade under the last line, so a cut-off list reads as continued, not as finished. */
const FADE = 24;

/** The plan as text, for the clipboard: `- [x] done`, `- [ ] to do`. */
export function planText(entries: PlanEntry[]): string {
  return entries.map((e) => `- [${e.status === "completed" ? "x" : " "}] ${e.content}`).join("\n");
}

/**
 * A thread's plan once it is waiting on you: every step with its state, Copy, and Implement,
 * which sends the one message a plan asks for. The card is only drawn while the thread is not
 * running and something is unfinished, so a finished plan leaves the transcript alone.
 */
export function PlanCard({ threadId, entries }: { threadId: string; entries: PlanEntry[] }) {
  const [open, setOpen] = useState(false);
  const [clipped, setClipped] = useState(false);
  const body = useRef<HTMLDivElement>(null);
  const text = useMemo(() => planText(entries), [entries]);
  const left = entries.filter((e) => e.status !== "completed").length;

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
          <Pill prominent size="sm" onClick={() => void runPrompt(threadId, "Implement the plan.")}>
            Implement
          </Pill>
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
        {entries.map((entry, i) => (
          <div key={`${i}-${entry.content}`} className="flex min-w-0 items-start gap-2 text-body">
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
                "min-w-0 break-words",
                entry.status === "completed" ? "text-fg-muted" : "text-fg",
              )}
            >
              {entry.content}
            </span>
          </div>
        ))}
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
