// The messages written now and sent later, in a card over the composer: what the message says, the
// countdown to its time, and the two things it can do before then — send it now, or drop it. A card
// is not a queued row: a queued row waits behind a turn and reads at the end of the transcript, while
// this has a time of its own and never runs until then (state/schedule.ts).

import { Clock, X } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { countdown, type ScheduledMessage, scheduledSummary, whenText } from "../state/schedule";
import { removeScheduled, sendScheduledNow, useApp } from "../state/store";
import { useNow } from "./AgentViews";
import { Pill, Tooltip } from "./primitives";

const EMPTY: ScheduledMessage[] = [];

function ScheduledCard({
  threadId,
  item,
  now,
  ready,
}: {
  threadId: string;
  item: ScheduledMessage;
  now: number;
  ready: boolean;
}) {
  const summary = scheduledSummary(item.text, item.extras);
  return (
    <div
      role="group"
      aria-label={`Scheduled message: ${summary}`}
      className="flex items-center gap-2 rounded-xl bg-surface px-3 py-2"
    >
      <Clock size={12} className="shrink-0 text-fg-muted" />
      <div className="min-w-0 flex-1">
        <p className="truncate text-sub text-fg" title={summary}>
          {summary}
        </p>
        <p className="text-caption text-fg-muted">
          Sends {countdown(item.due, now)} · {whenText(item.due, now)}
        </p>
      </div>
      <Pill
        quiet
        size="sm"
        disabled={!ready}
        onClick={() => void sendScheduledNow(threadId, item.id)}
        title={ready ? undefined : "Not connected"}
      >
        Send now
      </Pill>
      <Tooltip label="Delete">
        <button
          type="button"
          aria-label="Delete the scheduled message"
          onClick={() => removeScheduled(threadId, item.id)}
          className="flex size-[22px] shrink-0 items-center justify-center rounded-sm text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong"
        >
          <X size={13} />
        </button>
      </Tooltip>
    </div>
  );
}

/** Every message this thread is holding for a time, soonest first; nothing at all when there are none. */
export function ScheduledMessages({ threadId }: { threadId: string }) {
  const rows = useApp(useShallow((s) => s.scheduled[threadId] ?? EMPTY));
  const ready = useApp((s) => s.status === "ready");
  // The countdown is the only thing that changes while the card waits: one tick a second.
  const now = useNow(rows.length > 0);
  if (!rows.length) return null;
  return (
    <div className="mb-2 flex flex-col gap-2">
      {rows.map((item) => (
        <ScheduledCard key={item.id} threadId={threadId} item={item} now={now} ready={ready} />
      ))}
    </div>
  );
}
