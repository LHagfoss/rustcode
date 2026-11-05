// What waits at the end of a thread: the messages queued for it, drawn as the bubbles of the
// messages already sent but on a dashed edge, and one quiet line for a turn the app will wake it
// with. The dashed edge says it is waiting at rest; its state and what can be done with it appear
// on hover or keyboard focus, as a sent message's do. A queue a stopped or failed turn left reads
// Held at rest, with Resume under it. The composer edits a queued message in place (its pencil, or
// ↑), so a row never opens a field of its own.

import { ArrowUp, ChevronDown, ChevronUp, Clock, FileText, Pencil, RotateCw, X } from "lucide-react";
import { memo, useEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import type { Attachment } from "../state/compose";
import { isOwnQueued, queuedAnnouncement, queueView, STEER_REFUSAL, steerability } from "../state/queue";
import {
  cancelQueuedEdit,
  moveQueued,
  removeQueued,
  resumeQueue,
  sendQueuedNow,
  startQueuedEdit,
  useApp,
} from "../state/store";
import type { QueuedPrompt } from "../state/thread";
import { openLightbox } from "./Lightbox";
import { Markdown } from "./Markdown";
import { cx, Pill, Tooltip } from "./primitives";

const EMPTY_QUEUE: QueuedPrompt[] = [];

/** One shape for every icon button in the status row: quiet, and the row's own 22px. */
function RowAction({
  label,
  tip,
  onClick,
  disabled,
  children,
}: {
  label: string;
  tip?: string;
  onClick: () => void;
  disabled?: boolean;
  children: React.ReactNode;
}) {
  return (
    <Tooltip label={tip ?? label}>
      <button
        type="button"
        aria-label={label}
        disabled={disabled}
        onClick={onClick}
        className="flex size-[22px] items-center justify-center rounded-sm text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong disabled:opacity-40 disabled:hover:bg-transparent"
      >
        {children}
      </button>
    </Tooltip>
  );
}

/** The pictures and files that go with a queued message, above its text. */
const QueuedAttachments = memo(function QueuedAttachments({ list }: { list: Attachment[] }) {
  const pictures = list.flatMap((a) =>
    a.kind === "image" ? [{ src: `data:${a.mimeType};base64,${a.data}`, alt: a.name }] : [],
  );
  return (
    <div className="mb-1.5 flex max-w-[80%] flex-wrap justify-end gap-1.5">
      {list.map((a) =>
        a.kind === "image" ? (
          <button
            key={a.id}
            type="button"
            aria-label={`Open ${a.name}`}
            onClick={() =>
              openLightbox(
                pictures,
                pictures.findIndex((p) => p.alt === a.name),
              )
            }
            className="rounded-lg"
          >
            <img
              src={`data:${a.mimeType};base64,${a.data}`}
              alt={a.name}
              title={a.name}
              className="max-h-[120px] max-w-[180px] rounded-lg object-cover opacity-80 shadow-1"
            />
          </button>
        ) : (
          <Tooltip key={a.id} label={a.path ?? `${a.name} (sent inline)`}>
            <span className="flex h-[26px] items-center gap-1.5 rounded-full bg-fill-subtle px-2.5 text-callout text-fg-muted">
              <FileText size={12} />
              <span className="max-w-[200px] truncate">{a.name}</span>
            </span>
          </Tooltip>
        ),
      )}
    </div>
  );
});

/**
 * One queued message, at the end of the thread: right-aligned like a sent one, with a dashed edge
 * instead of a fill so it reads as waiting. Under it its state, and on hover or focus what can be
 * done with it: send it into the running turn, edit it in the composer, move it, remove it.
 */
export const QueuedMessageRow = memo(function QueuedMessageRow({
  threadId,
  item,
  index,
  count,
  held,
  steerable,
  images,
  builtin,
  from,
}: {
  threadId: string;
  item: QueuedPrompt;
  index: number;
  count: number;
  held: boolean;
  /** The harness-level half of the steer rule; `images` and `builtin` come from the same thread. */
  steerable: boolean;
  images: boolean;
  builtin: boolean;
  /** The thread that sent it, for a message delivered for another thread. */
  from?: string;
}) {
  // Only this one row listens for "this is the message the composer has open".
  const editing = useApp((s) => s.queuedEdit?.queuedId === item.id);
  const own = isOwnQueued(item);
  const attachments = item.extras?.attachments ?? [];
  const command = item.extras?.command;
  const links = item.extras?.links ?? [];
  const state = held ? "Held" : "Queued";
  const label = `${state} message ${index + 1} of ${count}, not sent`;
  // The same rule ⌘⏎ and `sendQueuedNow` follow: the arrow is active whenever it says yes — for
  // pictures too, on a harness that takes them mid-turn — and otherwise says exactly why it waits.
  const steer = steerability(item.extras, { live: steerable, images, builtin });
  return (
    <div className="group flex flex-col items-end" role="group" aria-label={label}>
      {/* A message another thread sent is that thread's, not the user's: it says whose it is, and
          it cannot be edited here. */}
      {from && <p className="mb-1 text-caption text-fg-faint">From “{from}”</p>}
      {attachments.length > 0 && <QueuedAttachments list={attachments} />}
      <div
        className={cx(
          // A bubble like a sent message's, told apart by its dashed edge: the text waits here, it was
          // not said. While the composer has it open, the edge takes the accent.
          "max-w-[80%] rounded-2xl border border-border-strong border-dashed bg-message/40 p-3 text-fg",
          editing && "border-accent/50",
        )}
      >
        {(command || links.length > 0) && (
          <div className="mb-1.5 flex flex-wrap items-center gap-1.5">
            {command && (
              <span className="flex h-[22px] items-center rounded-xs bg-accent/12 px-1.5 font-semibold text-accent text-callout">
                {command.name}
              </span>
            )}
            {links.map((url) => (
              <span
                key={url}
                className="flex h-[22px] max-w-[280px] items-center truncate rounded-xs bg-accent/12 px-1.5 text-accent text-callout"
              >
                {url}
              </span>
            ))}
          </div>
        )}
        {item.text.trim() ? (
          <Markdown
            text={item.text}
            className="text-fg [&_li]:whitespace-pre-wrap [&_p]:whitespace-pre-wrap"
          />
        ) : (
          <p className="text-callout text-fg-muted">Attachments only</p>
        )}
      </div>
      {/* Its state, and what can be done with it, in one reveal: hidden at rest (the dashed edge says
          it waits) and in together on hover of the row or keyboard focus inside it. A held queue says
          Held at rest, since that is why nothing runs; one being edited says so at rest too. A wake is
          never a row (it is one quiet line), and a message another thread sent is not the user's to
          edit. */}
      <div className="mt-1 flex h-[22px] items-center gap-1.5 text-caption text-fg-muted">
        <span className={cx("flex items-center gap-1", !held && !editing && "rc-meta-reveal")}>
          <Clock size={11} className="text-fg-faint" />
          {state}
        </span>
        {editing ? (
          <span className="flex items-center gap-1 text-accent">
            <Pencil size={11} />
            Editing in the composer
            <button
              type="button"
              aria-label="Cancel editing the queued message"
              onClick={() => cancelQueuedEdit()}
              className="ml-0.5 rounded-sm px-1 text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong"
            >
              Cancel
            </button>
          </span>
        ) : (
          <span className="rc-meta-reveal flex items-center gap-0.5">
            {!held &&
              (steer.ok ? (
                <RowAction
                  label="Send now"
                  tip="Send now: the turn reads it after its current step"
                  onClick={() => void sendQueuedNow(threadId, item.id)}
                >
                  <ArrowUp size={13} />
                </RowAction>
              ) : (
                <Tooltip label={STEER_REFUSAL[steer.reason]}>
                  <span
                    role="img"
                    aria-label={STEER_REFUSAL[steer.reason]}
                    className="flex size-[22px] items-center justify-center text-fg-faint"
                  >
                    <ArrowUp size={13} />
                  </span>
                </Tooltip>
              ))}
            {own && (
              <RowAction
                label="Edit in the composer"
                tip="Edit in the composer (⏎ saves, esc cancels)"
                onClick={() => startQueuedEdit(threadId, item.id)}
              >
                <Pencil size={12} />
              </RowAction>
            )}
            {count > 1 && index > 0 && (
              <RowAction label="Move up" onClick={() => moveQueued(threadId, item.id, -1)}>
                <ChevronUp size={13} />
              </RowAction>
            )}
            {count > 1 && index < count - 1 && (
              <RowAction label="Move down" onClick={() => moveQueued(threadId, item.id, 1)}>
                <ChevronDown size={13} />
              </RowAction>
            )}
            <RowAction
              label="Remove queued message"
              tip={item.link ? "Remove: the thread that sent it is told the message was withdrawn" : "Remove"}
              onClick={() => removeQueued(threadId, item.id)}
            >
              <X size={13} />
            </RowAction>
          </span>
        )}
      </div>
    </div>
  );
});

/** Resume starts what a stopped or failed turn left waiting; nothing runs until it is pressed. */
export const ResumeQueueRow = memo(function ResumeQueueRow({
  threadId,
  count,
}: {
  threadId: string;
  count: number;
}) {
  return (
    <div className="flex items-center justify-end gap-2.5">
      <span className="text-caption text-fg-muted">
        {count === 1 ? "1 held message" : count ? `${count} held messages` : "A wake-up held"} · not sent
      </span>
      <Pill onClick={() => void resumeQueue(threadId)}>
        <RotateCw size={12} />
        Resume
      </Pill>
    </div>
  );
});

/**
 * Says what changed in the queue, for a reader who cannot see the rows appear: polite, and only
 * after the first look (a queue that was already there when the thread opened is not news).
 */
export function QueuedAnnouncer({ threadId }: { threadId: string }) {
  const queue = useApp(useShallow((s) => s.threads[threadId]?.queue ?? EMPTY_QUEUE));
  const held = useApp((s) => !!s.threads[threadId]?.queueHeld);
  const editing = useApp((s) => s.queuedEdit?.threadId === threadId);
  const [line, setLine] = useState("");
  const last = useRef<string | null>(null);
  useEffect(() => {
    const view = queueView({ queue, running: false, queueHeld: held });
    const sig = `${queue.map((q) => q.id).join(",")}|${held}|${editing}`;
    if (last.current === sig) return;
    const first = last.current === null;
    last.current = sig;
    // The first render is where the queue already was, not a change to it.
    if (!first) setLine(queuedAnnouncement(view, editing));
  }, [queue, held, editing]);
  return (
    <p className="sr-only" aria-live="polite" aria-atomic="true">
      {line}
    </p>
  );
}
