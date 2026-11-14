// A pull request's diff in the right panel, with its review conversations in
// place: each under the line it was left on, resolved ones folded to a marker,
// outdated ones above their file, and the review summaries and plain comments
// in a list above the files. "Add to chat" puts a conversation into the open
// thread's composer as a draft; nothing here sends or writes to GitHub.

import { ChevronDown, ChevronRight, CircleCheck, CirclePlus, MessageSquare } from "lucide-react";
import { memo, useEffect, useMemo, useState } from "react";
import type { PullDetail, PullRequest } from "../acp/rustcode";
import { pullErrorTitle } from "../acp/rustcode";
import { count } from "../lib/plural";
import { appendToDraft, type FeedbackItem, feedbackItems, formatReviewFeedback } from "../lib/reviewFeedback";
import { runWordMarks } from "../lib/wordDiff";
import type { PatchLine } from "../state/diff";
import { loadPullDiff } from "../state/pullQueries";
import {
  type Conversation,
  conversationFeedback,
  conversationRange,
  type PullDiffFile,
  placeConversations,
  pullDiffFiles,
  reviewConversations,
  unresolvedFeedback,
} from "../state/pullReview";
import { setDraft, toast, useApp } from "../state/store";
import { relativeAge } from "../state/thread";
import { StatusMark } from "./ChangesPanel";
import { Fragments, useMarkedFragments } from "./DiffView";
import { Markdown } from "./Markdown";
import { Callout, cx, Pill, Skeleton } from "./primitives";
import { DiffCount } from "./Rows";

/**
 * Puts `items` into the open thread's composer, after what it already holds,
 * and puts the caret at its end. The user reads, edits and sends it.
 */
export function addFeedbackToThread(items: readonly FeedbackItem[], pull: PullRequest) {
  if (items.length === 0) return;
  const key = useApp.getState().selectedId ?? "new";
  const draft = useApp.getState().drafts[key] ?? "";
  setDraft(key, appendToDraft(draft, formatReviewFeedback(items, pull)));
  requestAnimationFrame(() => {
    const field = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message"]');
    if (!field) return;
    field.focus();
    field.setSelectionRange(field.value.length, field.value.length);
  });
  toast(`Added ${count(items.length, "comment")} to the composer`, "success");
}

const tint = (kind: PatchLine["kind"]) =>
  kind === "add" ? "bg-[var(--rc-diff-add)]" : kind === "del" ? "bg-[var(--rc-diff-del)]" : undefined;

function CommentBody({ author, at, body }: { author: string; at?: number; body: string }) {
  return (
    <div className="flex flex-col gap-0.5">
      <div className="text-caption text-fg-muted">
        <b className="text-fg">{author || "Deleted user"}</b>
        {at !== undefined && <> · {relativeAge(at)}</>}
      </div>
      <Markdown text={body} className="selectable text-callout" />
    </div>
  );
}

/** One conversation: its first comment and replies, or a marker when resolved, until opened. */
export function ConversationCard({ conversation, pull }: { conversation: Conversation; pull: PullRequest }) {
  const [open, setOpen] = useState(!conversation.resolved);
  const [first, ...replies] = conversation.comments;
  if (!first) return null;
  const range = conversationRange(conversation);
  if (!open) {
    return (
      <div className="my-1 font-sans">
        <Pill size="sm" quiet onClick={() => setOpen(true)} aria-expanded={false}>
          <CircleCheck size={11} className="text-success" />
          Resolved · {first.author || "Deleted user"}
          {replies.length > 0 && <> · {count(conversation.comments.length, "comment")}</>}
          <ChevronRight size={11} />
        </Pill>
      </div>
    );
  }
  return (
    <article
      className="my-1 flex flex-col gap-2 rounded-lg bg-surface p-2 font-sans shadow-1"
      aria-label={`Conversation on ${conversation.path}${range ? `:${range}` : ""}`}
    >
      <div className="flex items-center gap-1.5 text-caption text-fg-muted">
        <MessageSquare size={11} className="shrink-0" />
        <span className="min-w-0 flex-1 truncate font-mono">
          {range ? `${conversation.side === "LEFT" ? "base " : ""}line ${range}` : conversation.path}
        </span>
        {conversation.outdated && <span className="shrink-0 text-warning">Outdated</span>}
        {conversation.resolved && (
          <button
            type="button"
            onClick={() => setOpen(false)}
            aria-expanded
            className="flex shrink-0 items-center gap-1 text-success hover:underline"
          >
            <CircleCheck size={11} /> Resolved <ChevronDown size={11} />
          </button>
        )}
        <Pill size="sm" quiet onClick={() => addFeedbackToThread(conversationFeedback(conversation), pull)}>
          <CirclePlus size={11} /> Add to chat
        </Pill>
      </div>
      <CommentBody author={first.author} at={first.at} body={first.body} />
      {replies.length > 0 && (
        <div className="ml-2 flex flex-col gap-2 border-border border-l pl-3">
          {replies.map((reply) => (
            <CommentBody key={reply.id} author={reply.author} at={reply.at} body={reply.body} />
          ))}
        </div>
      )}
    </article>
  );
}

/** One file of the diff: its header, then its lines with conversations under theirs. */
const PullFile = memo(function PullFile({
  file,
  conversations,
  pull,
}: {
  file: PullDiffFile;
  conversations: readonly Conversation[];
  pull: PullRequest;
}) {
  const [open, setOpen] = useState(!file.large && file.status !== "D");
  const placement = useMemo(() => placeConversations(file, conversations), [file, conversations]);
  const texts = useMemo(() => (open ? file.lines.map((line) => line.text) : []), [open, file.lines]);
  const marks = useMemo(() => (open ? runWordMarks(file.lines) : []), [open, file.lines]);
  const { pieces, html, starts } = useMarkedFragments(texts, marks, file.path);
  const width = String(Math.max(...file.lines.map((l) => l.new ?? l.old ?? 0), 1)).length;
  const talk = conversations.filter((c) => c.path === file.path).length;
  return (
    <section aria-label={file.path} className="flex flex-col">
      <div className="flex items-center gap-1.5 py-1.5 text-callout">
        <button
          type="button"
          aria-expanded={open}
          aria-label={open ? `Collapse ${file.path}` : `Expand ${file.path}`}
          onClick={() => setOpen(!open)}
          className="flex size-5 shrink-0 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg"
        >
          {open ? <ChevronDown size={12} strokeWidth={2.5} /> : <ChevronRight size={12} strokeWidth={2.5} />}
        </button>
        <StatusMark status={file.status} />
        <span className="min-w-0 flex-1 truncate font-mono text-caption" title={file.path}>
          {file.oldPath && <span className="text-fg-muted">{file.oldPath} → </span>}
          {file.path}
        </span>
        {talk > 0 && (
          <span className="flex shrink-0 items-center gap-0.5 text-caption text-fg-muted">
            <MessageSquare size={10} /> {talk}
          </span>
        )}
        <DiffCount added={file.added} removed={file.removed} />
      </div>
      {placement.unplaced.map((c) => (
        <ConversationCard key={c.id} conversation={c} pull={pull} />
      ))}
      {!open ? (
        file.large && (
          <div className="mb-1">
            <Pill size="sm" onClick={() => setOpen(true)}>
              Large diff, {count(file.lines.length, "line")}: show it
            </Pill>
          </div>
        )
      ) : file.binary || file.lines.length === 0 ? (
        <p className="mb-1 text-caption text-fg-muted">
          {file.binary ? "Binary file." : file.oldPath ? "Renamed without changes." : "No text diff."}
        </p>
      ) : (
        <div className="selectable overflow-x-auto rounded-md bg-fill-subtle py-1 font-mono text-caption leading-[18px]">
          <div className="min-w-fit">
            {file.lines.map((line, i) =>
              line.kind === "hunk" ? (
                <div key={i} className="px-2 text-fg-faint">
                  {line.text}
                </div>
              ) : (
                <div key={i}>
                  <div className={cx("flex pr-3", tint(line.kind))}>
                    <span
                      className="shrink-0 pr-1.5 text-right text-fg-muted/60 tabular"
                      style={{ width: `${width + 1}ch` }}
                    >
                      {line.old ?? ""}
                    </span>
                    <span
                      className="shrink-0 pr-1.5 text-right text-fg-muted/60 tabular"
                      style={{ width: `${width + 1}ch` }}
                    >
                      {line.new ?? ""}
                    </span>
                    <span
                      className={cx(
                        "w-3 shrink-0",
                        line.kind === "add" && "text-success",
                        line.kind === "del" && "text-danger",
                      )}
                    >
                      {line.kind === "add" ? "+" : line.kind === "del" ? "−" : " "}
                    </span>
                    <Fragments
                      pieces={pieces[i] ?? []}
                      html={html}
                      start={starts[i] ?? 0}
                      tint={line.kind === "add" ? "add" : line.kind === "del" ? "del" : undefined}
                    />
                  </div>
                  {placement.atLine.get(i)?.map((c) => (
                    <div key={c.id} className="px-2 whitespace-normal">
                      <ConversationCard conversation={c} pull={pull} />
                    </div>
                  ))}
                </div>
              ),
            )}
          </div>
        </div>
      )}
    </section>
  );
});

/** The pull request's diff and review conversations, read when the view opens. */
export function PullDiff({ cwd, pull, detail }: { cwd: string; pull: PullRequest; detail?: PullDetail }) {
  const [files, setFiles] = useState<PullDiffFile[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    setError(null);
    void loadPullDiff(cwd, pull.number).then((answer) => {
      if (!live) return;
      if (answer.error) setError(`${pullErrorTitle(answer.error.code)}: ${answer.error.message}`);
      else setFiles(pullDiffFiles(answer.diff ?? ""));
    });
    return () => {
      live = false;
    };
  }, [cwd, pull.number]);
  const conversations = useMemo(() => (detail ? reviewConversations(detail) : []), [detail]);
  const general = useMemo(
    () => (detail ? feedbackItems(detail).filter((item) => item.kind !== "line") : []),
    [detail],
  );
  const open = conversations.filter((c) => !c.resolved);
  // A conversation on a file the diff does not show still has a place: above the files.
  const shown = new Set(files?.map((f) => f.path));
  const elsewhere = files ? conversations.filter((c) => !shown.has(c.path)) : [];
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2 text-caption text-fg-muted">
        <span className="flex-1">
          {files ? `${count(files.length, "file")} · ` : ""}
          {count(conversations.length, "conversation")}
          {conversations.length > 0 && `, ${open.length} unresolved`}
        </span>
        <Pill
          size="sm"
          disabled={open.length === 0}
          onClick={() => addFeedbackToThread(unresolvedFeedback(conversations), pull)}
        >
          <CirclePlus size={11} /> Add all unresolved
        </Pill>
      </div>
      {detail?.reviewCommentsError && (
        <Callout tone="warning">Line comments could not be read: {detail.reviewCommentsError}</Callout>
      )}
      {general.length > 0 && (
        <section aria-label="Reviews and comments" className="flex flex-col gap-1.5">
          <h3 className="font-semibold text-caption text-fg-muted">Reviews and comments</h3>
          {general.map((item) => (
            <div key={item.key} className="rounded-md bg-fill-subtle p-2">
              <div className="mb-0.5 flex items-center gap-2 text-caption text-fg-muted">
                <span className="min-w-0 flex-1 truncate">
                  <b className="text-fg">{item.author || "Deleted user"}</b> {item.kind.replace(/_/g, " ")}
                  {item.at !== undefined && <> · {relativeAge(item.at)}</>}
                </span>
                <Pill size="sm" quiet onClick={() => addFeedbackToThread([item], pull)}>
                  <CirclePlus size={11} /> Add to chat
                </Pill>
              </div>
              <Markdown text={item.body} className="selectable text-callout" />
            </div>
          ))}
        </section>
      )}
      {elsewhere.map((c) => (
        <ConversationCard key={c.id} conversation={c} pull={pull} />
      ))}
      {error && <Callout tone="danger">{error}</Callout>}
      {!files && !error && (
        <div role="status" aria-label="Loading" className="flex flex-col gap-1.5">
          {[70, 50, 85].map((w) => (
            <Skeleton key={w} className="h-3" style={{ width: `${w}%` }} />
          ))}
        </div>
      )}
      {files?.length === 0 && <p className="text-callout text-fg-muted">The diff is empty.</p>}
      {files?.map((file) => (
        <PullFile key={file.path} file={file} conversations={conversations} pull={pull} />
      ))}
    </div>
  );
}
