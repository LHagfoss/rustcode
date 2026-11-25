// A pull request's diff in the right panel, with its review conversations in
// place: each under the line it was left on, resolved ones folded to a marker,
// outdated ones above their file, and the review summaries and plain comments
// in a list above the files. "Add to chat" puts a conversation into the open
// thread's composer as a draft. Reply, Resolve and the "+" on a diff line's
// number write to GitHub through the engine (issue 3768); a failure keeps the text.

import { createContext, memo, useContext, useEffect, useMemo, useRef, useState } from "react";
import type { PullDetail, PullRequest } from "../acp/rustcode";
import { pullErrorTitle } from "../acp/rustcode";
import { formatBinding } from "../lib/keys";
import { opError, opRunning, useOp } from "../lib/op";
import { primaryModifier } from "../lib/platform";
import { count } from "../lib/plural";
import { type FeedbackItem, feedbackItems, formatReviewFeedback } from "../lib/reviewFeedback";
import { runWordMarks } from "../lib/wordDiff";
import { appendToComposer } from "../state/composerInsert";
import type { PatchLine } from "../state/diff";
import {
  applyCompletion,
  type CompletionItem,
  completionToken,
  type PullReference,
  pullPeople,
  rankPeople,
  rankReferences,
} from "../state/pullCompletion";
import {
  actOnThread,
  type LineTarget,
  lineTarget,
  loadReferences,
  type ThreadAction,
  targetLabel,
} from "../state/pullConversations";
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
import { toast } from "../state/store";
import { relativeAge } from "../state/thread";
import { StatusMark } from "./ChangesPanel";
import { Fragments, useMarkedFragments } from "./DiffView";
import {
  ChevronDown,
  ChevronRight,
  CircleCheck,
  CirclePlus,
  MessageSquare,
  MessageSquarePlus,
} from "./icons";
import { Markdown } from "./Markdown";
import { Callout, cx, noTextHelpers, Pill, Skeleton } from "./primitives";
import { DiffCount } from "./Rows";

/**
 * Puts `items` into the open thread's composer, after what it already holds,
 * and puts the caret at its end. The user reads, edits and sends it.
 */
export function addFeedbackToThread(items: readonly FeedbackItem[], pull: PullRequest) {
  if (items.length === 0) return;
  appendToComposer(formatReviewFeedback(items, pull));
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

/**
 * A comment the user writes to GitHub: a reply or a new line comment. It sends with the button or
 * mod+Enter, and on a failure keeps the text and shows GitHub's message under it.
 */
/** What a comment field completes from: the folder whose repository `#` reads, and the pull's people. */
const CompletionSource = createContext<{ cwd: string; people: readonly string[] }>({ cwd: "", people: [] });

export function CommentField({
  label,
  placeholder,
  hint,
  send,
  onCancel,
}: {
  label: string;
  placeholder: string;
  /** A line under the field, such as how to pick a range. */
  hint?: string;
  /** Sends the text; an error string keeps the field open with it. */
  send: (body: string) => Promise<string | null>;
  onCancel: () => void;
}) {
  const [body, setBody] = useState("");
  const [sending, runSend] = useOp();
  const busy = opRunning(sending);
  const error = opError(sending);
  // Completion (issue 3769): `#` lists issues and pull requests, `@` the pull's people.
  const source = useContext(CompletionSource);
  const field = useRef<HTMLTextAreaElement>(null);
  const [caret, setCaret] = useState(0);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [active, setActive] = useState(0);
  const [references, setReferences] = useState<PullReference[]>([]);
  const token = dismissed === body ? null : completionToken(body, caret);
  const wantsReferences = token?.trigger === "#" && !!source.cwd;
  useEffect(() => {
    if (!wantsReferences) return;
    let live = true;
    void loadReferences(source.cwd).then((list) => {
      if (live) setReferences(list);
    });
    return () => {
      live = false;
    };
  }, [wantsReferences, source.cwd]);
  const items: CompletionItem[] = !token
    ? []
    : token.trigger === "#"
      ? rankReferences(references, token.query)
      : rankPeople(source.people, token.query);
  const pickAt = Math.min(active, Math.max(items.length - 1, 0));
  const complete = (item: CompletionItem) => {
    if (!token) return;
    const next = applyCompletion(body, token, item.insert);
    setBody(next.text);
    setCaret(next.caret);
    setActive(0);
    requestAnimationFrame(() => {
      field.current?.focus();
      field.current?.setSelectionRange(next.caret, next.caret);
    });
  };
  const submit = async () => {
    if (!body.trim() || busy) return;
    await runSend(async () => {
      const failed = await send(body);
      if (failed) throw new Error(failed);
    });
  };
  return (
    <div className="flex flex-col gap-1.5 rounded-lg bg-fill-subtle p-2 font-sans">
      <textarea
        ref={field}
        autoFocus
        rows={2}
        value={body}
        disabled={busy}
        onChange={(e) => {
          setBody(e.target.value);
          setCaret(e.target.selectionStart);
          setActive(0);
        }}
        onSelect={(e) => setCaret(e.currentTarget.selectionStart)}
        aria-autocomplete="list"
        onKeyDown={(e) => {
          if (items.length > 0 && !primaryModifier(e)) {
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              const step = e.key === "ArrowDown" ? 1 : -1;
              setActive((pickAt + step + items.length) % items.length);
              return;
            }
            if (e.key === "Enter" || e.key === "Tab") {
              e.preventDefault();
              complete(items[pickAt]!);
              return;
            }
            if (e.key === "Escape") {
              e.preventDefault();
              setDismissed(body);
              return;
            }
          }
          if (e.key === "Enter" && primaryModifier(e)) {
            e.preventDefault();
            void submit();
          }
          if (e.key === "Escape" && !busy) onCancel();
        }}
        placeholder={`${placeholder} (${formatBinding("mod+enter")} to send)`}
        aria-label={label}
        {...noTextHelpers}
        className="w-full resize-y rounded-md bg-fill-subtle px-2 py-1.5 text-callout outline-none hover:bg-fill focus:bg-fill"
      />
      {items.length > 0 && (
        <div
          role="listbox"
          aria-label={token?.trigger === "#" ? "Issues and pull requests" : "People"}
          className="flex flex-col"
        >
          {items.map((item, i) => (
            <button
              key={item.key}
              type="button"
              role="option"
              aria-selected={i === pickAt}
              // Keep the caret in the field: a click picks without blurring it first.
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => complete(item)}
              onMouseEnter={() => setActive(i)}
              className={cx(
                "flex min-w-0 items-center gap-2 rounded-md px-2 py-1 text-left text-callout",
                i === pickAt ? "bg-fill-strong text-fg" : "text-fg-muted",
              )}
            >
              <span className="shrink-0 font-mono">{item.label}</span>
              {item.detail && <span className="min-w-0 flex-1 truncate">{item.detail}</span>}
            </button>
          ))}
        </div>
      )}
      {error && <Callout tone="danger">{error}</Callout>}
      <div className="flex items-center justify-end gap-1.5">
        {hint && <span className="min-w-0 flex-1 text-caption text-fg-muted">{hint}</span>}
        <Pill size="sm" disabled={busy} onClick={onCancel}>
          Cancel
        </Pill>
        <Pill size="sm" prominent disabled={busy || !body.trim()} onClick={() => void submit()}>
          {busy ? "Sending…" : "Send"}
        </Pill>
      </div>
    </div>
  );
}

/** One conversation: its first comment and replies, or a marker when resolved, until opened. */
export function ConversationCard({
  conversation,
  pull,
  cwd,
}: {
  conversation: Conversation;
  pull: PullRequest;
  cwd: string;
}) {
  // The resolved state the user just set, shown until the read after it agrees or it fails.
  const [resolvedNow, setResolvedNow] = useState<boolean | null>(null);
  const resolved = resolvedNow ?? conversation.resolved;
  const [open, setOpen] = useState(!conversation.resolved);
  const [replying, setReplying] = useState(false);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (resolvedNow === conversation.resolved) setResolvedNow(null);
  }, [conversation.resolved, resolvedNow]);
  const [first, ...replies] = conversation.comments;
  if (!first) return null;
  const threadId = conversation.threadId;
  const toggleResolved = async () => {
    if (!threadId || busy) return;
    const next = !resolved;
    setBusy(true);
    setResolvedNow(next);
    if (next) setOpen(false);
    const done = await actOnThread(cwd, pull, { kind: next ? "resolve" : "unresolve", threadId });
    setBusy(false);
    if (!done.ok) {
      setResolvedNow(null);
      setOpen(true);
      toast(done.error, "error");
    }
  };
  const reply = async (body: string) => {
    if (!threadId) return "The conversation has no id; reload the pull request.";
    const done = await actOnThread(cwd, pull, { kind: "reply", threadId, body });
    if (!done.ok) return done.error;
    setReplying(false);
    return null;
  };
  const range = conversationRange(conversation);
  if (!open && resolved) {
    return (
      <div className="my-1 font-sans">
        <Pill size="sm" quiet onClick={() => setOpen(true)} aria-expanded={false}>
          <CircleCheck size={12} className="text-success" />
          Resolved · {first.author || "Deleted user"}
          {replies.length > 0 && <> · {count(conversation.comments.length, "comment")}</>}
          <ChevronRight size={12} />
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
        <MessageSquare size={12} className="shrink-0" />
        <span className="min-w-0 flex-1 truncate font-mono">
          {range ? `${conversation.side === "LEFT" ? "base " : ""}line ${range}` : conversation.path}
        </span>
        {conversation.outdated && <span className="shrink-0 text-warning">Outdated</span>}
        {resolved && (
          <button
            type="button"
            onClick={() => setOpen(false)}
            aria-expanded
            className="flex shrink-0 items-center gap-1 text-success hover:underline"
          >
            <CircleCheck size={12} /> Resolved <ChevronDown size={12} />
          </button>
        )}
        {threadId && (
          <Pill size="sm" quiet disabled={busy} onClick={() => void toggleResolved()}>
            {resolved ? "Unresolve" : "Resolve"}
          </Pill>
        )}
        <Pill size="sm" quiet onClick={() => addFeedbackToThread(conversationFeedback(conversation), pull)}>
          <CirclePlus size={12} /> Add to chat
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
      {threadId &&
        (replying ? (
          <CommentField label="Reply" placeholder="Reply" send={reply} onCancel={() => setReplying(false)} />
        ) : (
          <div>
            <Pill size="sm" quiet onClick={() => setReplying(true)}>
              Reply…
            </Pill>
          </div>
        ))}
    </article>
  );
}

/** One file of the diff: its header, then its lines with conversations under theirs. */
const PullFile = memo(function PullFile({
  file,
  conversations,
  pull,
  cwd,
}: {
  file: PullDiffFile;
  conversations: readonly Conversation[];
  pull: PullRequest;
  cwd: string;
}) {
  const [open, setOpen] = useState(!file.large && file.status !== "D");
  // The lines a new comment is on: clicked on one line's number, shift-clicked on another for a range.
  const [picked, setPicked] = useState<{ start: number; end: number } | null>(null);
  const target: LineTarget | null = picked
    ? lineTarget(file.path, file.lines, picked.end, picked.start)
    : null;
  // GitHub takes a line comment on a draft or a closed pull request too; it needs the head commit.
  const canComment = Boolean(pull.headSha);
  const pick = (i: number, extend: boolean) =>
    setPicked((now) => (extend && now ? { start: now.start, end: i } : { start: i, end: i }));
  const comment = async (body: string) => {
    if (!target || !pull.headSha) return "Pick a line of the diff first.";
    const action: ThreadAction = { kind: "comment", commitId: pull.headSha, body, ...target };
    const done = await actOnThread(cwd, pull, action);
    if (!done.ok) return done.error;
    setPicked(null);
    return null;
  };
  const composerAt = picked ? Math.max(picked.start, picked.end) : -1;
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
        <ConversationCard key={c.id} conversation={c} pull={pull} cwd={cwd} />
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
                  <div
                    className={cx(
                      "flex pr-3",
                      tint(line.kind),
                      picked &&
                        i >= Math.min(picked.start, picked.end) &&
                        i <= composerAt &&
                        "shadow-[inset_2px_0_0_var(--rc-accent)]",
                    )}
                  >
                    <button
                      type="button"
                      disabled={!canComment}
                      onClick={(e) => pick(i, e.shiftKey)}
                      aria-label={`Comment on ${line.kind === "del" ? `base line ${line.old}` : `line ${line.new}`}`}
                      className="group/g relative flex shrink-0 enabled:hover:bg-accent/20 enabled:hover:text-accent"
                    >
                      <span
                        className="shrink-0 pr-1.5 text-right text-fg-muted/60 tabular"
                        style={{ width: `${width + 1}ch` }}
                      >
                        {line.old ?? ""}
                      </span>
                      <span
                        className={cx(
                          "shrink-0 pr-1.5 text-right text-fg-muted/60 tabular",
                          canComment && "group-hover/g:invisible",
                        )}
                        style={{ width: `${width + 1}ch` }}
                      >
                        {line.new ?? ""}
                      </span>
                      {canComment && (
                        <MessageSquarePlus
                          size={12}
                          className="invisible absolute top-[3px] right-1.5 group-hover/g:visible"
                        />
                      )}
                    </button>
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
                      <ConversationCard conversation={c} pull={pull} cwd={cwd} />
                    </div>
                  ))}
                  {i === composerAt && target && (
                    <div className="px-2 py-1 whitespace-normal">
                      <CommentField
                        label={`Comment on ${targetLabel(target)}`}
                        placeholder={`Comment on ${targetLabel(target)}`}
                        hint="Shift-click another line number to comment on a range."
                        send={comment}
                        onCancel={() => setPicked(null)}
                      />
                    </div>
                  )}
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
  // A diff for a pull request already left is dropped, not drawn over the one opened after it.
  const [diff, runLoad] = useOp<PullDiffFile[]>();
  const files = diff.data ?? null;
  const error = opError(diff);
  useEffect(() => {
    void runLoad(async () => {
      const answer = await loadPullDiff(cwd, pull.number);
      if (answer.error) throw new Error(`${pullErrorTitle(answer.error.code)}: ${answer.error.message}`);
      return pullDiffFiles(answer.diff ?? "");
    });
  }, [cwd, pull.number, runLoad]);
  const conversations = useMemo(() => (detail ? reviewConversations(detail) : []), [detail]);
  const general = useMemo(
    () => (detail ? feedbackItems(detail).filter((item) => item.kind !== "line") : []),
    [detail],
  );
  const open = conversations.filter((c) => !c.resolved);
  const people = useMemo(() => pullPeople(detail), [detail]);
  const completion = useMemo(() => ({ cwd, people }), [cwd, people]);
  // A conversation on a file the diff does not show still has a place: above the files.
  const shown = new Set(files?.map((f) => f.path));
  const elsewhere = files ? conversations.filter((c) => !shown.has(c.path)) : [];
  return (
    <CompletionSource.Provider value={completion}>
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
            <CirclePlus size={12} /> Add all unresolved
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
                    <CirclePlus size={12} /> Add to chat
                  </Pill>
                </div>
                <Markdown text={item.body} className="selectable text-callout" />
              </div>
            ))}
          </section>
        )}
        {elsewhere.map((c) => (
          <ConversationCard key={c.id} conversation={c} pull={pull} cwd={cwd} />
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
          <PullFile key={file.path} file={file} conversations={conversations} pull={pull} cwd={cwd} />
        ))}
      </div>
    </CompletionSource.Provider>
  );
}
