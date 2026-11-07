// The turns a rewind left behind, below the live transcript: an "Earlier attempt" divider and the
// turns it abandoned, faded to half so they read as a record rather than as the thread. They stay
// readable and clickable (their messages select and copy, their Changes open, their replies copy),
// but nothing in them sends the thread on: the conversation went back to the cut
// (state/attempts.ts). The divider can put the files back where the attempt ended, and says so
// when the engine could not keep that state.
//
// Rendered as one block after the virtualized rows, so the attempt is never folded or measured as
// part of the thread: it is a record, not the transcript.

import { History, TriangleAlert } from "lucide-react";
import { type Checkpoint, checkpointsByTurn } from "../acp/v2";
import { type Attempt, attemptTurns, transcriptTurns } from "../state/attempts";
import { returnToAttempt, useApp } from "../state/store";
import type { TranscriptItem } from "../state/thread";
import { Pill } from "./primitives";
import {
  AssistantRow,
  HandoffRow,
  NoticeRow,
  QuestionRow,
  SwitchRow,
  ThoughtRow,
  ToolRow,
  TurnEndRow,
  UserRow,
} from "./Rows";
import { TurnChanges } from "./TurnChanges";

export function EarlierAttempts({
  threadId,
  cwd,
  attempts,
}: {
  threadId: string;
  /** The folder the abandoned turns ran in: their tool rows read paths against it. */
  cwd: string;
  /** Newest first, as the store keeps them. */
  attempts: readonly Attempt[];
}) {
  const checkpoints = useApp((s) => s.checkpoints[threadId]);
  if (!attempts.length) return null;
  return (
    <div className="mx-auto flex w-full max-w-[var(--rc-chat-content-max-width)] flex-col gap-7 pt-7 pb-4">
      {attempts.map((attempt) => (
        <EarlierAttempt
          key={attempt.id}
          threadId={threadId}
          cwd={cwd}
          attempt={attempt}
          checkpoints={checkpoints ?? []}
        />
      ))}
    </div>
  );
}

function EarlierAttempt({
  threadId,
  cwd,
  attempt,
  checkpoints,
}: {
  threadId: string;
  cwd: string;
  attempt: Attempt;
  checkpoints: readonly Checkpoint[];
}) {
  // The attempt's own turns and checkpoints pair among themselves: its turn checkpoints are still
  // in the list (a rewind never deletes them), and the state it ended in is the one kept.
  const byEnd = checkpointsByTurn(transcriptTurns(attempt.items), [...checkpoints]);
  const replyOf = (item: TranscriptItem) => lastReplyBefore(attempt.items, item.id);
  const turns = attemptTurns(attempt.items);
  return (
    <section aria-label="Earlier attempt" className="flex flex-col gap-3.5">
      <div className="flex items-center gap-2 text-caption text-fg-faint">
        <span className="h-px flex-1 bg-border" />
        <History size={11} />
        <span className="font-medium">Earlier attempt</span>
        <span className="tabular">
          {turns} turn{turns === 1 ? "" : "s"}
        </span>
        <span className="h-px flex-1 bg-border" />
        <AttemptAction threadId={threadId} attempt={attempt} />
      </div>
      <div className="flex flex-col gap-3.5 opacity-50">
        {attempt.items.map((item) => (
          <AttemptItem
            key={item.id}
            item={item}
            threadId={threadId}
            cwd={cwd}
            checkpoint={item.kind === "turnEnd" ? byEnd[item.id] : undefined}
            reply={item.kind === "turnEnd" ? replyOf(item) : undefined}
          />
        ))}
      </div>
    </section>
  );
}

/** Where going back is offered, or why it is not: an engine that could not keep the state it left. */
function AttemptAction({ threadId, attempt }: { threadId: string; attempt: Attempt }) {
  if (attempt.head === undefined)
    return (
      <span className="flex items-center gap-1">
        <TriangleAlert size={11} />
        The state it left was not kept
      </span>
    );
  return (
    <Pill
      size="sm"
      onClick={() => returnToAttempt(threadId, attempt.id)}
      title="Put the files back where this attempt ended"
    >
      Go back to this attempt
    </Pill>
  );
}

/** One row of an abandoned turn, with the components the live transcript uses. */
function AttemptItem({
  item,
  threadId,
  cwd,
  checkpoint,
  reply,
}: {
  item: TranscriptItem;
  threadId: string;
  cwd: string;
  checkpoint?: Checkpoint;
  reply?: string;
}) {
  if (item.kind === "user") return <UserRow item={item} />;
  if (item.kind === "assistant") return <AssistantRow item={item} threadId={threadId} />;
  if (item.kind === "thought") return <ThoughtRow item={item} live={false} />;
  if (item.kind === "tool") return <ToolRow item={item} cwd={cwd} />;
  if (item.kind === "notice") return <NoticeRow item={item} />;
  if (item.kind === "handoff") return <HandoffRow item={item} />;
  if (item.kind === "switch") return <SwitchRow item={item} />;
  if (item.kind === "question") return <QuestionRow item={item} />;
  return (
    <TurnEndRow
      item={item}
      reply={reply}
      changes={
        checkpoint?.filesChanged ? (
          <TurnChanges threadId={threadId} checkpoint={checkpoint} rewindable={false} />
        ) : undefined
      }
    />
  );
}

/** The turn's own last reply, for its footer to copy: the one written before its end. */
function lastReplyBefore(items: readonly TranscriptItem[], endId: string): string | undefined {
  let reply: string | undefined;
  for (const item of items) {
    if (item.kind === "turnEnd" && item.id === endId) break;
    if (item.kind === "assistant") reply = item.text;
    else if (item.kind === "user") reply = undefined;
  }
  return reply;
}
