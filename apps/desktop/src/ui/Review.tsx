// The Review screen for one thread: its changes as a file tree and a diff
// (unified or side by side, highlighted, with the words that changed marked
// inside each line), from the working tree, the worktree's branch, one turn's
// checkpoint, or the branch's pull request. The tree takes the left column on
// a wide window and folds into a picker below that. Click a line number to
// draft a comment; the drafts go to the agent as one message, or to the pull
// request as one review with line comments (`_rustcode/pulls/review`).

import {
  ChevronDown,
  ChevronRight,
  FileDiff,
  Folder,
  MessageSquarePlus,
  RefreshCw,
  Send,
  X,
} from "lucide-react";
import { memo, useCallback, useEffect, useMemo, useState } from "react";
import type { JsonObject } from "../acp/jsonrpc";
import {
  type PullRequest,
  parsePullError,
  parsePullList,
  pullErrorTitle,
  repositoryOf,
} from "../acp/rustcode";
import { Method } from "../acp/types";
import { clip } from "../lib/clip";
import { setDiffMode, useDiffLayout, useDiffWidth } from "../lib/diffMode";
import { comparisonBase, shortRef } from "../lib/gitRef";
import { gitChanges } from "../lib/native";
import { type Piece, runWordMarks } from "../lib/wordDiff";
import {
  fileTree,
  type PatchLine,
  parsePatch,
  type ReviewComment,
  reviewMessage,
  splitRows,
} from "../state/diff";
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
import { ViewHeader } from "./Board";
import { StatusMark, splitUnifiedDiff } from "./ChangesPanel";
import { Fragments, useMarkedFragments } from "./DiffView";
import { cx, Pill, Segments, Select, Skeleton, Spinner, Tooltip } from "./primitives";
import { DiffCount } from "./Rows";

interface ReviewFile {
  path: string;
  status: string;
  added: number;
  removed: number;
  lines: PatchLine[];
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
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && body.trim()) onSave(body);
          if (e.key === "Escape") onCancel();
        }}
        placeholder="Comment on this line (⌘⏎ to save)"
        aria-label="Comment"
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
  onComment: () => void;
  commented: boolean;
}

function Gutter({ line, side, onComment, commented }: Omit<LineProps, "html">) {
  const n = side === "LEFT" ? line.old : line.new;
  return (
    <button
      type="button"
      onClick={onComment}
      disabled={n === undefined}
      aria-label={n === undefined ? undefined : `Comment on line ${n}`}
      className={cx(
        "group/g relative w-[6ch] shrink-0 pr-1.5 text-right text-fg-faint tabular enabled:hover:bg-accent/20 enabled:hover:text-accent",
        commented && "text-accent",
      )}
    >
      <span className="group-hover/g:invisible">{n ?? ""}</span>
      {n !== undefined && (
        <MessageSquarePlus
          size={11}
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
  const [editing, setEditing] = useState<{ side: "LEFT" | "RIGHT"; line: number; code: string } | null>(null);
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
  const start = (line: PatchLine, side: "LEFT" | "RIGHT") => {
    const n = side === "LEFT" ? line.old : line.new;
    if (n !== undefined) setEditing({ side, line: n, code: line.text });
  };
  const after = (line: PatchLine | undefined, side: "LEFT" | "RIGHT") => {
    if (!line) return null;
    const n = side === "LEFT" ? line.old : line.new;
    const saved = at(side, n);
    return (
      <>
        {saved.map((c, i) => (
          <div
            key={i}
            className="my-1 mr-3 ml-[8ch] flex items-start gap-2 rounded-lg bg-accent/10 px-2.5 py-1.5 font-sans text-callout rc-in"
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
              onAdd({ path: file.path, line: editing.line, side, body, code: editing.code });
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
                      onComment={() => start(line, "LEFT")}
                      commented={at("LEFT", line.old).length > 0}
                    />
                    <Gutter
                      line={line}
                      side="RIGHT"
                      onComment={() => start(line, line.kind === "del" ? "LEFT" : "RIGHT")}
                      commented={at("RIGHT", line.new).length > 0}
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
                                onComment={() => start(shown, side)}
                                commented={at(side, side === "LEFT" ? shown.old : shown.new).length > 0}
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
  const [collapsed, setCollapsed] = useState(false);
  const [files, setFiles] = useState<ReviewFile[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [comments, setComments] = useState<ReviewComment[]>([]);
  const [note, setNote] = useState("");
  const [pull, setPull] = useState<PullRequest | null>(null);
  const [event, setEvent] = useState<"COMMENT" | "APPROVE" | "REQUEST_CHANGES">("COMMENT");
  const [posting, setPosting] = useState(false);

  useEffect(() => {
    void refreshCheckpoints(threadId);
  }, [threadId]);

  // A folded file opens again for the next one, and for another source.
  // biome-ignore lint/correctness/useExhaustiveDependencies: a fold belongs to one file of one source
  useEffect(() => {
    setCollapsed(false);
  }, [selected, sourceKey(source)]);

  // The branch's pull request, when the project has one open for it.
  useEffect(() => {
    if (!pullsOn || !wt?.branch || !cwd) return;
    let live = true;
    void request(Method.pulls, { cwd, state: "open", limit: 30 }, 60_000)
      .then((r) => {
        const found = parsePullList((r as JsonObject).list ?? r).pulls.find((p) => p.headRef === wt.branch);
        if (live) setPull(found ?? null);
      })
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [pullsOn, wt?.branch, cwd]);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
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
        const r = (await request(Method.pullDiff, { cwd, number: source.pull.number }, 60_000)) as JsonObject;
        const err = parsePullError(r);
        if (err) throw new Error(`${pullErrorTitle(err.code)}: ${err.message}`);
        const chunks = splitUnifiedDiff(String(r.diff ?? ""));
        out = Object.entries(chunks).map(([path, lines]) => {
          const parsed = parsePatch(lines);
          return {
            path,
            status: "M",
            added: parsed.filter((l) => l.kind === "add").length,
            removed: parsed.filter((l) => l.kind === "del").length,
            lines: parsed,
          };
        });
      } else {
        const folder = source.kind === "branch" && wt ? wt.path : cwd;
        const changes = await gitChanges(folder, source.kind === "branch" ? wt?.base : undefined);
        if (!changes) throw new Error("Reading changes needs the desktop app: git runs in its native shell.");
        const chunks = splitUnifiedDiff(changes.diff);
        out = changes.files.map((f) => ({
          path: f.path,
          status: f.status,
          added: f.added,
          removed: f.removed,
          lines: parsePatch(chunks[f.path] ?? []),
        }));
      }
      setFiles(out);
      setSelected((cur) => (cur && out.some((f) => f.path === cur) ? cur : (out[0]?.path ?? null)));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setFiles(null);
    } finally {
      setLoading(false);
    }
  }, [source, threadId, cwd, wt]);

  useEffect(() => {
    void load();
  }, [load]);

  const tree = useMemo(() => fileTree((files ?? []).map((f) => f.path)), [files]);
  const file = files?.find((f) => f.path === selected);
  const label = sourceLabel(source, wt?.branch);
  // The branch's counts compare against the base its worktree was made from; the
  // other scopes name their comparison themselves (HEAD, a turn, a pull request).
  const base = source.kind === "branch" ? comparisonBase(wt) : undefined;
  const sources: { value: string; label: string }[] = [
    { value: "uncommitted", label: "Uncommitted" },
    ...(wt ? [{ value: "branch", label: "Branch" }] : []),
    ...(pull ? [{ value: `pull:${pull.number}`, label: `PR #${pull.number}` }] : []),
  ];

  const sendToAgent = () => {
    if (!comments.length && !note.trim()) return;
    selectThread(threadId);
    void submit(reviewMessage(label, comments, note)).then((sent) => {
      if (sent) {
        toast(thread?.running ? "Queued for the agent" : "Sent to the agent", "success");
        setComments([]);
        setNote("");
      }
    });
  };

  const postReview = async () => {
    if (source.kind !== "pull") return;
    const repository = repositoryOf(source.pull.url);
    if (!repository) return;
    setPosting(true);
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
          comments: comments.map((c) => ({ path: c.path, line: c.line, side: c.side, body: c.body })),
        },
        60_000,
      )) as JsonObject;
      const err = parsePullError(r);
      if (err) toast(`${pullErrorTitle(err.code)}: ${err.message}`, "error");
      else {
        toast("Review posted", "success");
        setComments([]);
        setNote("");
      }
    } catch (e) {
      toast(`Could not post the review: ${e instanceof Error ? e.message : String(e)}`, "error");
    } finally {
      setPosting(false);
    }
  };

  return (
    <main className="flex min-w-0 flex-1 flex-col bg-bg" aria-label="Review">
      <ViewHeader title={`Review · ${title}`}>
        <Segments
          label="Changes"
          value={source.kind === "turn" ? "turn" : sourceKey(source)}
          onChange={(v) => {
            if (v === "uncommitted" || v === "branch") setSource({ kind: v });
            else if (v.startsWith("pull:") && pull) setSource({ kind: "pull", pull });
          }}
          options={
            source.kind === "turn" ? [...sources, { value: "turn", label: `Turn ${source.turn}` }] : sources
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
                label: `${c.turn}. ${clip(c.prompt, 40)}`,
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
            onClick={() => void load()}
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
                  <Folder size={11} className="shrink-0" />
                  <span className="truncate font-mono">{group.dir}/</span>
                </div>
              )}
              {group.files.map((path) => {
                const f = files?.find((x) => x.path === path);
                const n = comments.filter((c) => c.path === path).length;
                return (
                  <button
                    key={path}
                    type="button"
                    onClick={() => setSelected(path)}
                    aria-current={selected === path ? "true" : undefined}
                    title={path}
                    className={cx(
                      "flex w-full items-center gap-1.5 rounded-md px-2 py-[5px] text-left",
                      selected === path ? "bg-fill" : "hover:bg-fill-subtle",
                    )}
                  >
                    {f ? <StatusMark status={f.status} /> : <span className="size-4 shrink-0" />}
                    <span className="min-w-0 flex-1 truncate font-mono text-callout">
                      {path.split("/").pop()}
                    </span>
                    {n > 0 && (
                      <span className="rounded-full bg-accent/20 px-1.5 text-caption text-accent">{n}</span>
                    )}
                    {f && <DiffCount added={f.added} removed={f.removed} />}
                  </button>
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
                  label: `${f.path} +${f.added} −${f.removed}`,
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
                    aria-expanded={!collapsed}
                    aria-label={collapsed ? `Expand ${file.path}` : `Collapse ${file.path}`}
                    onClick={() => setCollapsed(!collapsed)}
                    className="-ml-1.5 flex size-6 shrink-0 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg"
                  >
                    {collapsed ? (
                      <ChevronRight size={13} strokeWidth={2.5} />
                    ) : (
                      <ChevronDown size={13} strokeWidth={2.5} />
                    )}
                  </button>
                  <FileDiff size={13} className="shrink-0 text-fg-muted" />
                  <span className="selectable min-w-0 truncate font-mono">{file.path}</span>
                  <DiffCount added={file.added} removed={file.removed} />
                  <span className="flex-1" />
                  <span className="shrink-0 text-caption text-fg-faint max-[1099px]:hidden">
                    Click a line number to comment
                  </span>
                </div>
                <div className="px-4 pb-4">
                  {collapsed ? null : file.lines.length ? (
                    <FileDiffView
                      key={`${sourceKey(source)}|${file.path}`}
                      file={file}
                      mode={mode}
                      comments={comments.filter((c) => c.path === file.path)}
                      onAdd={(c) => setComments((all) => [...all, c])}
                      onRemove={(c) => setComments((all) => all.filter((x) => x !== c))}
                    />
                  ) : (
                    <p className="text-callout text-fg-muted">No text diff (binary or untracked file).</p>
                  )}
                </div>
              </>
            ) : (
              !error && loading && <ReviewDiffSkeleton />
            )}
          </div>
          {(comments.length > 0 || note) && (
            <div className="elevated mx-4 mb-4 flex flex-col gap-2 p-2.5 rc-in">
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
                className="resize-y rounded-lg bg-fill-subtle px-2.5 py-1.5 text-callout outline-none hover:bg-fill focus:bg-fill"
              />
              <div className="flex flex-wrap items-center justify-end gap-2">
                <Pill onClick={() => setComments([])}>Discard</Pill>
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
                      {posting ? <Spinner size={11} /> : null} Post to #{source.pull.number}
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
