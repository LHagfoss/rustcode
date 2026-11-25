import { formatBinding } from "../lib/keys";
import { primaryModifier } from "../lib/platform";

// The Review screen for one thread: its changes as a file tree and a diff
// (unified or side by side, highlighted, with the words that changed marked
// inside each line), from the working tree, the worktree's branch, one turn's
// checkpoint, or the branch's pull request. The tree takes the left column on
// a wide window and folds into a picker below that. Click a line number to
// draft a comment; the drafts go to the agent as one message, or to the pull
// request as one review with line comments (`_rustcode/pulls/review`).

import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import type { JsonObject } from "../acp/jsonrpc";
import { type PullRequest, parsePullError, pullErrorTitle, repositoryOf } from "../acp/rustcode";
import { Method } from "../acp/types";
import { clip } from "../lib/clip";
import { setDiffMode, useDiffLayout, useDiffWidth } from "../lib/diffMode";
import { comparisonBase, shortRef } from "../lib/gitRef";
import { gitChanges } from "../lib/native";
import { opError, opRunning, useOp } from "../lib/op";
import { type Piece, runWordMarks } from "../lib/wordDiff";
import {
  fileTree,
  type PatchLine,
  parsePatch,
  type ReviewComment,
  rangeLines,
  reviewMessage,
  splitRows,
} from "../state/diff";
import { loadPullDiff, prefetchPullDiff, usePullList } from "../state/pullQueries";
import { pullDiffFiles } from "../state/pullReview";
import {
  draftCounts,
  EMPTY_DRAFT,
  type ReviewDraft,
  reviewDraftKey,
  setReviewDraft,
  useReviewDrafts,
} from "../state/reviewDrafts";
import {
  patchHash,
  setViewed,
  stepFile,
  useReviewViewed,
  viewedKey,
  viewedState,
} from "../state/reviewViewed";
import {
  featureMissing,
  refreshCheckpoints,
  request,
  selectThread,
  submit,
  toast,
  turnDiff,
  useApp,
  worktreeOf,
} from "../state/store";
import { StatusMark, splitUnifiedDiff } from "./ChangesPanel";
import { Fragments, useMarkedFragments } from "./DiffView";
import {
  Check,
  ChevronDown,
  ChevronRight,
  FileDiff,
  Folder,
  MessageSquarePlus,
  RefreshCw,
  Send,
  X,
} from "./icons";
import { ViewHeader } from "./MainHeader";
import { cx, noTextHelpers, Pill, Segments, Select, Skeleton, Spinner, Tooltip } from "./primitives";
import { DiffCount } from "./Rows";

interface ReviewFile {
  path: string;
  /** The path before a rename. */
  oldPath?: string;
  status: string;
  added: number;
  removed: number;
  lines: PatchLine[];
  binary?: boolean;
}

type Source =
  | { kind: "uncommitted" }
  | { kind: "branch" }
  | { kind: "turn"; turn: number }
  | { kind: "pull"; pull: PullRequest };

const sourceKey = (s: Source) =>
  s.kind === "turn" ? `turn:${s.turn}` : s.kind === "pull" ? `pull:${s.pull.number}` : s.kind;

function sourceLabel(s: Source, branch?: string): string {
  switch (s.kind) {
    case "uncommitted":
      return "the uncommitted changes";
    case "branch":
      return `branch ${branch ?? ""}`.trim();
    case "turn":
      return `turn ${s.turn}`;
    case "pull":
      return `pull request #${s.pull.number}`;
  }
}

const STATUS_LETTER: Record<string, string> = { added: "A", deleted: "D", renamed: "R", modified: "M" };

function CommentBox({
  onSave,
  onCancel,
  initial = "",
}: {
  onSave: (body: string) => void;
  onCancel: () => void;
  initial?: string;
}) {
  const [body, setBody] = useState(initial);
  return (
    <div className="my-1 ml-[8ch] mr-3 flex flex-col gap-1.5 rounded-lg bg-fill-subtle p-2 font-sans rc-in">
      <textarea
        autoFocus
        rows={2}
        value={body}
        onChange={(e) => setBody(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && primaryModifier(e) && body.trim()) onSave(body);
          if (e.key === "Escape") onCancel();
        }}
        placeholder={`Comment on this line (${formatBinding("mod+enter")} to save)`}
        aria-label="Comment"
        {...noTextHelpers}
        className="w-full resize-y rounded-md bg-fill-subtle px-2 py-1.5 text-callout outline-none hover:bg-fill focus:bg-fill"
      />
      <div className="flex justify-end gap-1.5">
        <Pill onClick={onCancel}>Cancel</Pill>
        <Pill prominent disabled={!body.trim()} onClick={() => onSave(body)}>
          Save draft
        </Pill>
      </div>
    </div>
  );
}

interface LineProps {
  line: PatchLine;
  html: string | null | undefined;
  side: "LEFT" | "RIGHT";
  onComment: (extend: boolean) => void;
  commented: boolean;
  /** Inside the range a comment is being written on. */
  picked?: boolean;
}

function Gutter({ line, side, onComment, commented, picked }: Omit<LineProps, "html">) {
  const n = side === "LEFT" ? line.old : line.new;
  return (
    <button
      type="button"
      onClick={(e) => onComment(e.shiftKey)}
      disabled={n === undefined}
      aria-label={n === undefined ? undefined : `Comment on line ${n}`}
      className={cx(
        "group/g relative w-[6ch] shrink-0 pr-1.5 text-right text-fg-faint tabular enabled:hover:bg-accent/20 enabled:hover:text-accent",
        commented && "text-accent",
        picked && "bg-accent/20 text-accent",
      )}
    >
      <span className="group-hover/g:invisible">{n ?? ""}</span>
      {n !== undefined && (
        <MessageSquarePlus
          size={12}
          className="invisible absolute top-[3px] right-1.5 group-hover/g:visible"
        />
      )}
    </button>
  );
}

function Code({
  pieces,
  html,
  start,
  tint,
}: {
  pieces: Piece[];
  html: (string | null)[];
  start: number;
  tint?: "add" | "del";
}) {
  return (
    <span className="min-w-0 flex-1">
      <Fragments pieces={pieces} html={html} start={start} tint={tint} />
    </span>
  );
}

const wordTint = (kind: PatchLine["kind"]) =>
  kind === "add" ? ("add" as const) : kind === "del" ? ("del" as const) : undefined;

const tint = (kind: PatchLine["kind"] | undefined) =>
  kind === "add" ? "bg-[var(--rc-diff-add)]" : kind === "del" ? "bg-[var(--rc-diff-del)]" : "";

const FileDiffView = memo(function FileDiffView({
  file,
  mode,
  comments,
  onAdd,
  onRemove,
}: {
  file: ReviewFile;
  mode: "unified" | "split";
  comments: ReviewComment[];
  onAdd: (c: ReviewComment) => void;
  onRemove: (c: ReviewComment) => void;
}) {
  const [editing, setEditing] = useState<{
    side: "LEFT" | "RIGHT";
    line: number;
    startLine?: number;
    code: string;
  } | null>(null);
  // Every line highlighted once, split around the words that changed on it.
  const texts = useMemo(() => file.lines.map((line) => line.text), [file.lines]);
  const marks = useMemo(() => runWordMarks(file.lines), [file.lines]);
  const { pieces, html, starts } = useMarkedFragments(texts, marks, file.path);
  const where = useMemo(() => new Map(file.lines.map((line, i) => [line, i])), [file.lines]);
  const code = (line: PatchLine) => {
    const i = where.get(line) ?? 0;
    return <Code pieces={pieces[i]!} html={html} start={starts[i]!} tint={wordTint(line.kind)} />;
  };
  const at = (side: "LEFT" | "RIGHT", n?: number) =>
    n === undefined ? [] : comments.filter((c) => c.side === side && c.line === n);
  const start = (line: PatchLine, side: "LEFT" | "RIGHT", extend = false) => {
    const n = side === "LEFT" ? line.old : line.new;
    if (n === undefined) return;
    const anchor = editing?.side === side ? (editing.startLine ?? editing.line) : undefined;
    const range = extend && anchor !== undefined ? rangeLines(file.lines, side, anchor, n) : null;
    if (range && range.length > 1) {
      const first = Math.min(anchor!, n);
      const last = Math.max(anchor!, n);
      setEditing({ side, line: last, startLine: first, code: range.map((l) => l.text).join("\n") });
    } else setEditing({ side, line: n, code: line.text });
  };
  const picked = (side: "LEFT" | "RIGHT", n?: number) =>
    n !== undefined &&
    editing?.side === side &&
    editing.startLine !== undefined &&
    n >= editing.startLine &&
    n <= editing.line;
  const after = (line: PatchLine | undefined, side: "LEFT" | "RIGHT") => {
    if (!line) return null;
    const n = side === "LEFT" ? line.old : line.new;
    const saved = at(side, n);
    return (
      <>
        {saved.map((c, i) => (
          <div
            key={i}
            className="my-1 mr-3 ml-[8ch] flex items-start gap-2 rounded-lg bg-accent/10 px-2 py-1.5 font-sans text-callout rc-in"
          >
            <span className="selectable flex-1 whitespace-pre-wrap">{c.body}</span>
            <button
              type="button"
              aria-label="Remove comment"
              onClick={() => onRemove(c)}
              className="text-fg-faint hover:text-fg"
            >
              <X size={12} />
            </button>
          </div>
        ))}
        {editing && editing.side === side && editing.line === n && (
          <CommentBox
            onCancel={() => setEditing(null)}
            onSave={(body) => {
              onAdd({
                path: file.path,
                line: editing.line,
                ...(editing.startLine !== undefined ? { startLine: editing.startLine } : {}),
                side,
                body,
                code: editing.code,
              });
              setEditing(null);
            }}
          />
        )}
      </>
    );
  };
  return (
    <div className="selectable overflow-x-auto rounded-lg bg-fill-subtle py-1 font-mono text-caption leading-[18px]">
      <div className="min-w-fit">
        {mode === "unified"
          ? file.lines.map((line, i) =>
              line.kind === "hunk" ? (
                <div key={i} className="bg-fill-subtle px-3 text-fg-faint">
                  {line.text}
                </div>
              ) : (
                <div key={i}>
                  <div className={cx("flex", tint(line.kind))}>
                    <Gutter
                      line={line}
                      side="LEFT"
                      onComment={(extend) => start(line, "LEFT", extend)}
                      commented={at("LEFT", line.old).length > 0}
                      picked={picked("LEFT", line.old)}
                    />
                    <Gutter
                      line={line}
                      side="RIGHT"
                      onComment={(extend) => start(line, line.kind === "del" ? "LEFT" : "RIGHT", extend)}
                      commented={at("RIGHT", line.new).length > 0}
                      picked={picked("RIGHT", line.new)}
                    />
                    <span
                      className={cx(
                        "w-4 shrink-0 text-center",
                        line.kind === "add" ? "text-success" : "text-danger",
                      )}
                    >
                      {line.kind === "add" ? "+" : line.kind === "del" ? "−" : " "}
                    </span>
                    {code(line)}
                  </div>
                  {after(line, line.kind === "del" ? "LEFT" : "RIGHT")}
                  {line.kind === "ctx" && after(line, "LEFT")}
                </div>
              ),
            )
          : splitRows(file.lines).map((row, i) =>
              row.hunk ? (
                <div key={i} className="bg-fill-subtle px-3 text-fg-faint">
                  {row.hunk}
                </div>
              ) : (
                <div key={i} className="grid grid-cols-2">
                  {(["LEFT", "RIGHT"] as const).map((side) => {
                    const line = side === "LEFT" ? row.left : row.right;
                    const shown =
                      line && (side === "LEFT" ? line.kind !== "add" : line.kind !== "del")
                        ? line
                        : undefined;
                    return (
                      <div key={side} className={cx("min-w-0", side === "RIGHT" && "border-border border-l")}>
                        <div className={cx("flex", shown && shown.kind !== "ctx" && tint(shown.kind))}>
                          {shown ? (
                            <>
                              <Gutter
                                line={shown}
                                side={side}
                                onComment={(extend) => start(shown, side, extend)}
                                commented={at(side, side === "LEFT" ? shown.old : shown.new).length > 0}
                                picked={picked(side, side === "LEFT" ? shown.old : shown.new)}
                              />
                              {code(shown)}
                            </>
                          ) : (
                            <span className="h-[18px] flex-1 bg-fill-subtle/50" />
                          )}
                        </div>
                        {shown && (shown.kind !== "ctx" || side === "RIGHT") && after(shown, side)}
                      </div>
                    );
                  })}
                </div>
              ),
            )}
      </div>
    </div>
  );
});

export function Review({ threadId }: { threadId: string }) {
  const thread = useApp((s) => s.threads[threadId]);
  const summary = useApp((s) => s.sessions.find((x) => x.id === threadId));
  const wt = useApp((s) => worktreeOf(s, threadId));
  const checkpoints = useApp((s) => s.checkpoints[threadId]);
  const checkpointsMissing = useApp((s) => (s.features.checkpoints ? null : featureMissing("checkpoints")));
  const pullsOn = useApp((s) => !!s.agent?.pullRequests);
  const cwd = thread?.cwd ?? summary?.cwd ?? "";
  const title = thread?.title ?? summary?.title ?? "Thread";
  const [source, setSource] = useState<Source>(wt ? { kind: "branch" } : { kind: "uncommitted" });
  // The diff column's own width decides: the file tree takes its share of the window.
  const [diffRef, diffWidth] = useDiffWidth<HTMLElement>();
  const { mode, wide } = useDiffLayout(diffWidth);
  const [collapsed, setCollapsed] = useState<boolean | null>(null);
  // A diff for a source already left is dropped, not drawn over the one chosen after it.
  const [diff, runLoad] = useOp<ReviewFile[]>();
  const files = diff.data ?? null;
  const error = opError(diff);
  const loading = opRunning(diff);
  const [selected, setSelected] = useState<string | null>(null);
  const draftKey = reviewDraftKey(threadId, sourceKey(source));
  const draft = useReviewDrafts((s) => s.book[draftKey]) ?? EMPTY_DRAFT;
  const counts = useReviewDrafts(useShallow((s) => draftCounts(s.book, threadId)));
  const { comments, note } = draft;
  const writeDraft = (next: Partial<ReviewDraft>) => setReviewDraft(draftKey, { ...draft, ...next });
  const setComments = (update: (all: ReviewComment[]) => ReviewComment[]) =>
    writeDraft({ comments: update(comments) });
  const setNote = (text: string) => writeDraft({ note: text });
  const discard = () => {
    const kept = draft;
    setReviewDraft(draftKey, EMPTY_DRAFT);
    toast("Review draft discarded", "info", {
      action: { label: "Undo", run: () => setReviewDraft(draftKey, kept) },
    });
  };
  const [event, setEvent] = useState<"COMMENT" | "APPROVE" | "REQUEST_CHANGES">("COMMENT");
  const [post, runPost] = useOp();
  const posting = opRunning(post);

  useEffect(() => {
    void refreshCheckpoints(threadId);
  }, [threadId]);

  // A folded file opens again for the next one, and for another source.
  // biome-ignore lint/correctness/useExhaustiveDependencies: a fold belongs to one file of one source
  useEffect(() => {
    setCollapsed(null);
  }, [selected, sourceKey(source)]);

  // The branch's pull request, from the pull store the panel and the sidebar
  // also read: a list one of them read a moment ago answers this at once.
  const openPulls = usePullList(pullsOn && cwd ? cwd : undefined, "open", { enabled: pullsOn });
  const pull = useMemo(
    () => (wt?.branch ? openPulls.data?.find((p) => p.headRef === wt.branch) : undefined) ?? null,
    [openPulls.data, wt?.branch],
  );
  const load = useCallback(
    async (force = false) => {
      const loaded = await runLoad(
        async () => {
          let out: ReviewFile[] = [];
          if (source.kind === "turn") {
            out = (await turnDiff(threadId, source.turn)).map((f) => ({
              path: f.path,
              status: STATUS_LETTER[f.status] ?? "M",
              added: f.insertions,
              removed: f.deletions,
              lines: parsePatch(f.patch),
            }));
          } else if (source.kind === "pull") {
            // Through the pull store: a diff read on intent (the pointer resting on
            // the pull segment) is the one this draws, so the click waits for nothing.
            const answer = await loadPullDiff(cwd, source.pull.number, { force });
            if (answer.error) {
              throw new Error(`${pullErrorTitle(answer.error.code)}: ${answer.error.message}`);
            }
            out = pullDiffFiles(answer.diff ?? "").map((f) => ({
              path: f.path,
              ...(f.oldPath ? { oldPath: f.oldPath } : {}),
              status: f.status,
              added: f.added,
              removed: f.removed,
              lines: f.lines,
              binary: f.binary,
            }));
          } else {
            const folder = source.kind === "branch" && wt ? wt.path : cwd;
            const changes = await gitChanges(folder, source.kind === "branch" ? wt?.base : undefined);
            if (!changes)
              throw new Error("Reading changes needs the desktop app: git runs in its native shell.");
            const chunks = splitUnifiedDiff(changes.diff);
            out = changes.files.map((f) => ({
              path: f.path,
              ...(f.oldPath ? { oldPath: f.oldPath } : {}),
              status: f.status,
              binary: f.binary,
              added: f.added,
              removed: f.removed,
              lines: parsePatch(chunks[f.path] ?? []),
            }));
          }
          return out;
        },
        // A failed diff is not drawn again under the next try's skeleton.
        { dropDataOnFailure: true },
      );
      if (loaded.status !== "done") return;
      const out = loaded.value;
      setSelected((cur) => (cur && out.some((f) => f.path === cur) ? cur : (out[0]?.path ?? null)));
    },
    [source, threadId, cwd, wt, runLoad],
  );

  useEffect(() => {
    void load();
  }, [load]);
  const tree = useMemo(() => fileTree((files ?? []).map((f) => f.path)), [files]);
  const order = useMemo(() => tree.flatMap((group) => group.files), [tree]);
  const viewedBook = useReviewViewed((s) => s.book);
  const hashes = useMemo(() => new Map((files ?? []).map((f) => [f.path, patchHash(f.lines)])), [files]);
  const viewedOf = (path: string) =>
    viewedState(viewedBook, viewedKey(threadId, sourceKey(source), path), hashes.get(path) ?? "");
  const viewedCount = order.filter((path) => viewedOf(path) === "viewed").length;
  const toggleViewed = (path: string) => {
    const hash = hashes.get(path);
    if (hash === undefined) return;
    const marking = viewedOf(path) !== "viewed";
    setViewed(viewedKey(threadId, sourceKey(source), path), marking ? hash : null);
    setCollapsed(null);
    if (!marking) return;
    const next = stepFile(order, path, 1, (p) => p === path || viewedOf(p) === "viewed", true);
    if (next) setSelected(next);
  };
  const keys = useRef<(e: KeyboardEvent) => void>(() => undefined);
  keys.current = (e) => {
    if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey || !order.length) return;
    const target = e.target as HTMLElement | null;
    if (target?.closest("input, textarea, select, [contenteditable='true'], [role='dialog'], [role='menu']"))
      return;
    if (e.key === "j" || e.key === "k") {
      const next = stepFile(order, selected, e.key === "j" ? 1 : -1);
      if (next) setSelected(next);
    } else if (e.key === "v" && selected) toggleViewed(selected);
    else return;
    e.preventDefault();
  };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => keys.current(e);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  const file = files?.find((f) => f.path === selected);
  const folded = collapsed ?? (file ? viewedOf(file.path) === "viewed" : false);
  const label = sourceLabel(source, wt?.branch);
  // The branch's counts compare against the base its worktree was made from; the
  // other scopes name their comparison themselves (HEAD, a turn, a pull request).
  const base = source.kind === "branch" ? comparisonBase(wt) : undefined;
  const counted = (key: string, label: string) => (counts[key] ? `${label} · ${counts[key]}` : label);
  const sources: { value: string; label: string }[] = [
    { value: "uncommitted", label: counted("uncommitted", "Uncommitted") },
    ...(wt ? [{ value: "branch", label: counted("branch", "Branch") }] : []),
    ...(pull
      ? [{ value: `pull:${pull.number}`, label: counted(`pull:${pull.number}`, `PR #${pull.number}`) }]
      : []),
  ];

  const sendToAgent = () => {
    if (!comments.length && !note.trim()) return;
    selectThread(threadId);
    const key = draftKey;
    void submit(reviewMessage(label, comments, note)).then((sent) => {
      if (sent) setReviewDraft(key, EMPTY_DRAFT);
    });
  };

  const postReview = async () => {
    if (source.kind !== "pull") return;
    const repository = repositoryOf(source.pull.url);
    if (!repository) return;
    const key = draftKey;
    await runPost(async () => {
      try {
        const r = (await request(
          Method.pullReview,
          {
            cwd,
            repository,
            number: source.pull.number,
            commitId: source.pull.headSha,
            event,
            body: note,
            comments: comments.map((c) => ({
              path: c.path,
              line: c.line,
              side: c.side,
              body: c.body,
              ...(c.startLine !== undefined ? { start_line: c.startLine, start_side: c.side } : {}),
            })),
          },
          60_000,
        )) as JsonObject;
        const err = parsePullError(r);
        if (err) toast(`${pullErrorTitle(err.code)}: ${err.message}`, "error");
        else {
          toast("Review posted", "success");
          setReviewDraft(key, EMPTY_DRAFT);
        }
      } catch (e) {
        toast(`Could not post the review: ${e instanceof Error ? e.message : String(e)}`, "error");
      }
    });
  };

  return (
    <main className="flex min-w-0 flex-1 flex-col bg-bg" aria-label="Review" data-focus-area="main">
      <ViewHeader title={`Review · ${title}`}>
        <Segments
          label="Changes"
          value={source.kind === "turn" ? "turn" : sourceKey(source)}
          onChange={(v) => {
            if (v === "uncommitted" || v === "branch") setSource({ kind: v });
            else if (v.startsWith("pull:") && pull) setSource({ kind: "pull", pull });
          }}
          // The pull segment rests before it is chosen: read its diff now, so the
          // click draws the files instead of waiting for `gh`.
          onIntent={(v) => v.startsWith("pull:") && prefetchPullDiff(cwd, Number(v.slice(5)))}
          options={
            source.kind === "turn"
              ? [...sources, { value: "turn", label: counted(`turn:${source.turn}`, `Turn ${source.turn}`) }]
              : sources
          }
        />
        {checkpoints && checkpoints.length > 0 ? (
          <Select
            label="A turn's changes"
            size="sm"
            className="max-w-[180px]"
            value={source.kind === "turn" ? String(source.turn) : ""}
            onChange={(turn) => turn && setSource({ kind: "turn", turn: Number(turn) })}
            options={[
              { value: "", label: "Turn…" },
              ...checkpoints.map((c) => ({
                value: String(c.turn),
                label: counted(`turn:${c.turn}`, `${c.turn}. ${clip(c.prompt, 40)}`),
              })),
            ]}
          />
        ) : (
          checkpointsMissing && (
            <Tooltip label={checkpointsMissing}>
              <span className="text-caption text-fg-faint">No turn diffs</span>
            </Tooltip>
          )
        )}
        <Tooltip label={wide ? "Unified or side by side" : "Side by side needs a wider view"}>
          <span>
            <Segments
              label="Layout"
              value={mode}
              onChange={(v) => setDiffMode(v)}
              options={[
                { value: "unified", label: "Unified" },
                { value: "split", label: "Split", disabled: !wide },
              ]}
            />
          </span>
        </Tooltip>
        <Tooltip label="Reload">
          <button
            type="button"
            aria-label="Reload"
            onClick={() => void load(true)}
            className="flex size-7 items-center justify-center rounded-full text-fg-muted hover:bg-fill"
          >
            {loading ? <Spinner size={12} /> : <RefreshCw size={13} />}
          </button>
        </Tooltip>
        <Pill onClick={() => selectThread(threadId)}>Open thread</Pill>
      </ViewHeader>
      <div className="flex min-h-0 flex-1">
        <nav
          aria-label="Files"
          className="w-[260px] shrink-0 overflow-y-auto border-border border-r px-2 pb-4 max-[1099px]:hidden"
        >
          <div className="flex h-9 items-center gap-2 px-2">
            <span className="shrink-0 font-semibold text-body">Files</span>
            <span className="shrink-0 text-caption text-fg-faint tabular">{files?.length ?? 0}</span>
            {files && files.length > 0 && (
              <Tooltip label="J and K move between files, V marks one viewed">
                <span className="shrink-0 text-caption text-fg-faint tabular">
                  · {viewedCount} of {files.length} viewed
                </span>
              </Tooltip>
            )}
            {base && (
              <Tooltip label={base}>
                <span className="min-w-0 truncate text-caption text-fg-faint">vs {shortRef(base)}</span>
              </Tooltip>
            )}
          </div>
          {error && <p className="px-2 py-3 text-callout text-fg-muted">{error}</p>}
          {files && !files.length && (
            <p className="px-2 py-3 text-callout text-fg-muted">No changes in {label}.</p>
          )}
          {!files && loading && <ReviewFilesSkeleton />}
          {tree.map((group) => (
            <div key={group.dir} className="mb-1">
              {group.dir && (
                <div className="flex items-center gap-1.5 px-2 pt-2 pb-0.5 text-caption text-fg-faint">
                  <Folder size={12} className="shrink-0" />
                  <span className="truncate font-mono">{group.dir}/</span>
                </div>
              )}
              {group.files.map((path) => {
                const f = files?.find((x) => x.path === path);
                const n = comments.filter((c) => c.path === path).length;
                const seen = viewedOf(path);
                return (
                  <Tooltip
                    key={path}
                    label={`${f?.oldPath ? `${f.oldPath} → ${path}` : path}${seen === "changed" ? " · changed since you viewed it" : ""}`}
                    side="right"
                  >
                    <button
                      type="button"
                      onClick={() => setSelected(path)}
                      aria-current={selected === path ? "true" : undefined}
                      className={cx(
                        "flex w-full items-center gap-1.5 rounded-md px-2 py-[5px] text-left",
                        selected === path ? "bg-fill" : "hover:bg-fill-subtle",
                        seen === "viewed" && selected !== path && "text-fg-muted",
                      )}
                    >
                      {f ? <StatusMark status={f.status} /> : <span className="size-4 shrink-0" />}
                      <span className="min-w-0 flex-1 truncate font-mono text-callout">
                        {path.split("/").pop()}
                      </span>
                      {n > 0 && (
                        <span className="rounded-full bg-accent/20 px-1.5 text-caption text-accent">{n}</span>
                      )}
                      {seen === "changed" && (
                        <span className="size-1.5 shrink-0 rounded-full bg-accent">
                          <span className="sr-only">Changed since viewed</span>
                        </span>
                      )}
                      {seen === "viewed" ? (
                        <Check size={12} aria-label="Viewed" className="shrink-0 text-success" />
                      ) : (
                        f && <DiffCount added={f.added} removed={f.removed} />
                      )}
                    </button>
                  </Tooltip>
                );
              })}
            </div>
          ))}
        </nav>
        <section ref={diffRef} className="flex min-w-0 flex-1 flex-col">
          {files && files.length > 0 && (
            <div className="shrink-0 px-4 pt-3 min-[1100px]:hidden">
              <Select
                label="File"
                size="sm"
                className="max-w-full"
                value={selected ?? ""}
                onChange={setSelected}
                options={files.map((f) => ({
                  value: f.path,
                  label: `${viewedOf(f.path) === "viewed" ? "✓ " : ""}${f.path} +${f.added} −${f.removed}`,
                }))}
              />
            </div>
          )}
          <div className="min-h-0 flex-1 overflow-y-auto">
            {file ? (
              <>
                {/* The file's own header holds its place while the diff scrolls under it. */}
                <div className="sticky top-0 z-10 flex items-center gap-2 bg-bg px-4 py-2 text-callout">
                  <button
                    type="button"
                    aria-expanded={!folded}
                    aria-label={folded ? `Expand ${file.path}` : `Collapse ${file.path}`}
                    onClick={() => setCollapsed(!folded)}
                    className="-ml-1.5 flex size-6 shrink-0 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg"
                  >
                    {folded ? (
                      <ChevronRight size={13} strokeWidth={2.5} />
                    ) : (
                      <ChevronDown size={13} strokeWidth={2.5} />
                    )}
                  </button>
                  <FileDiff size={13} className="shrink-0 text-fg-muted" />
                  <span className="selectable min-w-0 truncate font-mono">
                    {file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}
                  </span>
                  <DiffCount added={file.added} removed={file.removed} />
                  <span className="flex-1" />
                  <span className="shrink-0 text-caption text-fg-faint max-[1300px]:hidden">
                    Click a line number to comment, Shift-click another to cover a range
                  </span>
                  <Tooltip label="Mark viewed (V)">
                    <label className="flex shrink-0 cursor-pointer items-center gap-1.5 text-caption text-fg-muted hover:text-fg">
                      <input
                        type="checkbox"
                        className="shrink-0 accent-[var(--rc-accent)]"
                        checked={viewedOf(file.path) === "viewed"}
                        onChange={() => toggleViewed(file.path)}
                      />
                      Viewed
                    </label>
                  </Tooltip>
                </div>
                <div className="px-4 pb-4">
                  {folded ? null : file.lines.length ? (
                    <FileDiffView
                      key={`${sourceKey(source)}|${file.path}`}
                      file={file}
                      mode={mode}
                      comments={comments.filter((c) => c.path === file.path)}
                      onAdd={(c) => setComments((all) => [...all, c])}
                      onRemove={(c) => setComments((all) => all.filter((x) => x !== c))}
                    />
                  ) : (
                    <p className="text-callout text-fg-muted">
                      {file.binary ? "A binary file: no lines to show." : "No text diff to show."}
                    </p>
                  )}
                </div>
              </>
            ) : (
              !error && loading && <ReviewDiffSkeleton />
            )}
          </div>
          {(comments.length > 0 || note) && (
            <div className="elevated mx-4 mb-4 flex flex-col gap-2 p-2 rc-in">
              <div className="flex items-center gap-2 px-1 text-callout">
                <ChevronRight size={12} className="text-fg-faint" />
                <b>
                  {comments.length} comment{comments.length === 1 ? "" : "s"}
                </b>
                <span className="text-fg-muted">on {label}</span>
              </div>
              <textarea
                rows={2}
                value={note}
                onChange={(e) => setNote(e.target.value)}
                placeholder="A note for the whole review (optional)"
                aria-label="Review note"
                {...noTextHelpers}
                className="resize-y rounded-lg bg-fill-subtle px-2 py-1.5 text-callout outline-none hover:bg-fill focus:bg-fill"
              />
              <div className="flex flex-wrap items-center justify-end gap-2">
                <Pill onClick={discard}>Discard</Pill>
                {source.kind === "pull" && (
                  <>
                    <Segments
                      label="Review kind"
                      value={event}
                      onChange={setEvent}
                      options={[
                        { value: "COMMENT", label: "Comment" },
                        { value: "APPROVE", label: "Approve" },
                        { value: "REQUEST_CHANGES", label: "Request changes" },
                      ]}
                    />
                    <Pill disabled={posting} onClick={() => void postReview()}>
                      {posting ? <Spinner size={12} /> : null} Post to #{source.pull.number}
                    </Pill>
                  </>
                )}
                <Pill prominent onClick={sendToAgent}>
                  <Send size={12} /> Send to agent
                </Pill>
              </div>
            </div>
          )}
        </section>
      </div>
    </main>
  );
}

/** Where the diff will be: the file's header line, then lines with their gutter. */
function ReviewDiffSkeleton() {
  return (
    <div role="status" aria-label="Loading" className="flex flex-col gap-2">
      <div className="mb-2 flex items-center gap-2">
        <Skeleton className="h-3 w-3 rounded-xs" />
        <Skeleton className="h-3 w-52" />
        <Skeleton className="h-2.5 w-20" />
      </div>
      <div className="flex flex-col gap-[3px] rounded-lg bg-fill-subtle py-1">
        {[64, 42, 78, 30, 55, 70, 48].map((w, i) => (
          <div key={i} className="flex h-[18px] items-center gap-2 px-3">
            <Skeleton className="h-[10px] w-[3ch] rounded-xs" />
            <Skeleton className="h-[10px] w-[3ch] rounded-xs" />
            <Skeleton className="h-[10px]" style={{ width: `${w}%` }} />
          </div>
        ))}
      </div>
    </div>
  );
}

/** Where the file list will be: rows the height of a file, its status and counts. */
function ReviewFilesSkeleton() {
  return (
    <div role="status" aria-label="Loading" className="flex flex-col gap-[3px] px-1 pt-1">
      {[62, 84, 48, 72, 56].map((w, i) => (
        <div key={i} className="flex h-[26px] items-center gap-1.5 px-2">
          <Skeleton className="h-[9px] w-2.5 rounded-xs" />
          <Skeleton className="h-3" style={{ width: `${w}%` }} />
        </div>
      ))}
    </div>
  );
}
