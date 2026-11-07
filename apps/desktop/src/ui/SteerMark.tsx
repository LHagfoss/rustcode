// A steering note, where it took effect: on the plan step the turn was working through when it read
// the note (`ui/PlanCard.tsx`), or — for a thread with no plan, or one rewritten past that step — a
// quiet line under the message the note was sent as (`ui/Transcript.tsx`). Compact at rest: one line
// with the note's first line. Hovering it, or a click, gives the whole note and when it was sent,
// which is also what keeps it readable once the turn it landed in has folded.

import { CornerDownRight } from "lucide-react";
import { useState } from "react";
import { type SteerNote, steerLine } from "../state/steer";
import { messageTime } from "../state/thread";
import { cx, Tooltip } from "./primitives";

/** The whole note and when it was sent, for a tip or an opened marker. */
function FullNote({ note, className }: { note: SteerNote; className?: string }) {
  return (
    <span className={cx("block whitespace-pre-wrap break-words", className)}>
      {note.text.trim()}
      <span className="mt-0.5 block text-fg-faint">Sent {messageTime(note.at)}</span>
    </span>
  );
}

/**
 * A note's marker: `Steered: first line`. With `inPlan` it sits under its plan step's text; without,
 * it is the transcript's own row — the same quiet divider a harness handoff draws — under the
 * message it was sent as. A click opens the whole note in place, so a reader on a touch screen (or
 * one who cannot hover) still gets it; Escape or a second click folds it back.
 */
export function SteerMark({ note, inPlan }: { note: SteerNote; inPlan?: boolean }) {
  const [open, setOpen] = useState(false);
  const line = steerLine(note.text);
  const tip = <FullNote note={note} />;
  const label = `Steering note: ${line}`;
  const mark = (
    <button
      type="button"
      aria-label={label}
      aria-expanded={open}
      onClick={() => setOpen(!open)}
      className={cx(
        "flex min-w-0 items-center gap-1 text-caption",
        // The accent says a steer is news, as the `Steered` badge under the message does.
        "text-accent hover:text-fg",
        inPlan ? "max-w-full" : "max-w-[min(420px,60vw)]",
      )}
    >
      <CornerDownRight size={11} className="shrink-0 text-fg-faint" aria-hidden />
      <span className="shrink-0 text-fg-muted">Steered:</span>
      <span className="min-w-0 truncate">{line}</span>
    </button>
  );
  return (
    <div className={cx("flex flex-col gap-1", inPlan ? "mt-1 min-w-0" : "py-1")}>
      {inPlan ? (
        <Tooltip label={open ? undefined : tip}>{mark}</Tooltip>
      ) : (
        // The row's own line says it belongs to the note, not to the next step of the turn.
        <div className="flex min-w-0 items-center gap-3">
          <span className="h-px flex-1 bg-border" aria-hidden />
          <Tooltip label={open ? undefined : tip}>{mark}</Tooltip>
          <span className="h-px flex-1 bg-border" aria-hidden />
        </div>
      )}
      {open && (
        <FullNote
          note={note}
          className={cx("selectable text-callout text-fg-muted", inPlan ? "pl-[15px]" : "px-1")}
        />
      )}
    </div>
  );
}
