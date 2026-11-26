// The git button on the side panel's Changes tab (issue 3724): one primary action that follows the repository's state
// (Commit, Push, Pull, Create Pull Request), a menu with the rest, and the commit sheet. The engine
// runs git in the thread's folder (state/gitActions.ts), never the agent, so hooks and signing apply.

import { Dialog, DropdownMenu } from "radix-ui";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { openExternal } from "../lib/native";
import { opError, opRunning, useOp } from "../lib/op";
import {
  acceptsGeneratedDraft,
  commitSummaryLabel,
  commitsStagedIndex,
  draftCommitMessage,
  draftPull,
  GIT_ACTION_LABEL,
  type GitAction,
  type GitFile,
  GitMethod,
  type GitRead,
  type GitState,
  gitActionDisplayLabel,
  gitErrorOf,
  gitReadFor,
  gitReads,
  initialCommitSelection,
  parseGitState,
  planGitActions,
  pullFromUrl,
  pullPresence,
} from "../state/gitActions";
import { readPullList, usePullList } from "../state/pullQueries";
import { request, toast } from "../state/store";
import type { ThreadState } from "../state/thread";
import { splitThreadKey } from "../state/threadKeys";
import { GitBranch, GitCommitHorizontal, GitPullRequest, RefreshCw } from "./icons";
import {
  ClusterButton,
  cx,
  inputClass,
  MenuContent,
  menuItem,
  menuLabel,
  menuSeparator,
  noTextHelpers,
  Pill,
  Spinner,
  Tooltip,
} from "./primitives";

/**
 * The folder's git state, read again when a turn ends and when the window comes back. Only the
 * newest read's answer lands, and only for the folder it was read for (issue 3744).
 */
export function useGitState(
  cwd: string | undefined,
  running: boolean,
): [GitRead, () => Promise<void>, (state: GitState) => void] {
  const [read, setRead] = useState<GitRead>({});
  const [reads] = useState(gitReads);
  const refresh = useCallback(async () => {
    if (!cwd) return;
    const land = reads.begin(cwd);
    let answer: GitRead | undefined;
    try {
      answer = land({ state: parseGitState(await request(GitMethod.state, { path: cwd }, 20_000)) });
    } catch (error) {
      answer = land({ error: gitErrorOf(error).message });
    }
    if (answer) setRead(answer);
  }, [cwd, reads]);
  const apply = useCallback(
    (state: GitState) => {
      if (!cwd) return;
      const land = reads.begin(cwd);
      const answer = land({ state });
      if (answer) setRead(answer);
    },
    [cwd, reads],
  );
  useEffect(() => {
    if (!running) void refresh();
  }, [refresh, running]);
  useEffect(() => {
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh]);
  return [gitReadFor(read, cwd), refresh, apply];
}

/** After a pull request is opened, the open list is read again so the button and the panel show it. */
function rereadOpenPulls(cwd: string) {
  void readPullList(cwd, "open", { force: true });
}

const ICON: Record<GitAction, typeof GitBranch> = {
  commit: GitCommitHorizontal,
  commit_push: GitCommitHorizontal,
  push: GitBranch,
  pull: GitBranch,
  create_pr: GitPullRequest,
};

/** Keyed by the thread, so a thread switch never carries the last thread's sheet or action over. */
export function GitActionsButton({
  thread,
  gitRead,
  onRefresh,
  onMutated,
}: {
  thread?: ThreadState;
  gitRead: GitRead;
  onRefresh: () => Promise<void>;
  onMutated?: () => void;
}) {
  return (
    <GitButton
      key={thread?.id ?? ""}
      thread={thread}
      read={gitRead}
      refresh={onRefresh}
      onMutated={onMutated}
    />
  );
}

function GitButton({
  thread,
  read,
  refresh,
  onMutated,
}: {
  thread?: ThreadState;
  read: GitRead;
  refresh: () => Promise<void>;
  onMutated?: () => void;
}) {
  const cwd = thread?.cwd;
  const [busy, setBusy] = useState<GitAction | null>(null);
  const [sheet, setSheet] = useState<GitAction | null>(null);
  const state = read.state;
  const branch = state?.branch;
  const stagedCount = useMemo(() => (state?.files ?? []).filter((f) => f.staged).length, [state]);
  const pulls = usePullList(cwd, "open", { enabled: !!state?.hasRemote && !state.onDefault && !!branch });
  const { presence, pull: openPull } = pullPresence(branch, pulls.data);
  const plan = useMemo(() => (state ? planGitActions(state, presence) : undefined), [state, presence]);
  if (!thread || !cwd) return null;
  if (read.error || !state || !plan) {
    return (
      <Tooltip label={read.error ? "Not a git repository" : "Reading git…"}>
        <span className="inline-flex">
          <button
            type="button"
            aria-label="Git"
            disabled
            className="inline-flex h-6 w-8 items-center justify-center rounded-full text-fg-muted opacity-40"
          >
            <GitBranch size={14} />
          </button>
        </span>
      </Tooltip>
    );
  }

  const run = async (action: GitAction) => {
    if (action === "commit" || action === "commit_push" || action === "create_pr") {
      setSheet(action);
      return;
    }
    setBusy(action);
    const id = toast(`${GIT_ACTION_LABEL[action]}…`, "info", { persist: true });
    try {
      const answer = (await request(GitMethod.run, { path: cwd, action }, 330_000)) as {
        url?: string;
      };
      const pull = pullFromUrl(answer?.url);
      if (pull) rereadOpenPulls(cwd);
      if (pull)
        void request(
          "_rustcode/pulls/watch/start",
          { sessionId: splitThreadKey(thread.id).id, cwd, ...pull, link: true },
          60_000,
        ).catch(() => {});
      toast(pull ? `Opened pull request #${pull.number}` : `${GIT_ACTION_LABEL[action]} done`, "success", {
        id,
      });
      onMutated?.();
    } catch (error) {
      toast(gitErrorOf(error).message, "error", { id, persist: true });
    } finally {
      setBusy(null);
      void refresh();
    }
  };

  const primary = plan.primary;
  const label = primary ? gitActionDisplayLabel(primary, stagedCount) : plan.note;
  const PrimaryIcon = primary ? ICON[primary] : GitBranch;
  return (
    <>
      <DropdownMenu.Root modal={false} onOpenChange={(open) => open && void refresh()}>
        <DropdownMenu.Trigger asChild>
          <ClusterButton
            label="Git actions"
            tooltip={primary ? `${branch}: ${label}` : `${branch ?? "Git"}: ${plan.note}`}
            className="relative data-[state=open]:bg-fill-strong data-[state=open]:text-fg"
          >
            {busy ? <Spinner size={12} /> : <PrimaryIcon size={14} />}
            {primary && !busy && (
              <span
                aria-hidden
                className="absolute top-[3px] right-[7px] size-[5px] rounded-full bg-accent"
              />
            )}
          </ClusterButton>
        </DropdownMenu.Trigger>
        <MenuContent side="bottom" align="end" className="min-w-[220px]">
          <DropdownMenu.Label className={menuLabel}>
            {branch}
            {state.upstream ? ` → ${state.upstream}` : state.hasRemote ? " · not pushed" : " · no remote"}
          </DropdownMenu.Label>
          {[primary, ...plan.others].flatMap((action) =>
            action ? (
              <DropdownMenu.Item
                key={action}
                className={menuItem}
                disabled={!!busy}
                onSelect={() => void run(action)}
              >
                {(() => {
                  const Icon = ICON[action];
                  return <Icon size={13} className="text-fg-muted" />;
                })()}
                {gitActionDisplayLabel(action, stagedCount)}
                {action === "commit" || action === "commit_push" ? "…" : ""}
              </DropdownMenu.Item>
            ) : (
              []
            ),
          )}
          {openPull && (
            <DropdownMenu.Item
              className={menuItem}
              onSelect={() => openPull.url && openExternal(openPull.url)}
            >
              <GitPullRequest size={13} className="text-fg-muted" /> Pull request #{openPull.number}
            </DropdownMenu.Item>
          )}
          <DropdownMenu.Separator className={menuSeparator} />
          <DropdownMenu.Item className={menuItem} onSelect={() => void refresh()}>
            <RefreshCw size={13} className="text-fg-muted" /> Refresh
          </DropdownMenu.Item>
        </MenuContent>
      </DropdownMenu.Root>
      {sheet === "create_pr" && (
        <PullSheet
          cwd={cwd}
          fallback={state.lastCommitMessage || thread.title || state.branch || "Update"}
          onClose={() => setSheet(null)}
          onDone={() => {
            void refresh();
            rereadOpenPulls(cwd);
          }}
        />
      )}
      {(sheet === "commit" || sheet === "commit_push") && (
        <CommitSheet
          cwd={cwd}
          title={thread.title}
          state={state}
          push={sheet === "commit_push"}
          onClose={() => setSheet(null)}
          onDone={() => {
            void refresh();
            onMutated?.();
          }}
        />
      )}
    </>
  );
}

function CommitSheet({
  cwd,
  title,
  state,
  push,
  onClose,
  onDone,
}: {
  cwd: string;
  title?: string;
  state: GitState;
  push: boolean;
  onClose: () => void;
  onDone: () => void;
}) {
  const [picked, setPicked] = useState<ReadonlySet<string>>(() => initialCommitSelection(state.files));
  const [message, setMessage] = useState(() => {
    const staged = state.files.filter((file) => file.staged);
    return draftCommitMessage(title, staged.length > 0 ? staged : state.files);
  });
  const edited = useRef(false);
  const [generationNote, setGenerationNote] = useState("");
  const [newBranch, setNewBranch] = useState("");
  const [committing, runCommit] = useOp();
  const running = opRunning(committing);
  const failure = opError(committing);
  const stagedCount = state.files.filter((file) => file.staged).length;
  const usingStagedIndex = commitsStagedIndex(picked, state.files);
  const files = state.files.filter((f) => picked.has(f.path)).map((f) => f.path);
  const fileKey = JSON.stringify(files);
  useEffect(() => {
    let cancelled = false;
    void request(
      "_rustcode/textGeneration/generate",
      { kind: "commit_message", path: cwd, files: JSON.parse(fileKey) },
      50_000,
    )
      .then((answer) => {
        const text = (answer as { text?: string }).text;
        if (acceptsGeneratedDraft(edited.current, cancelled, text)) setMessage(text);
      })
      .catch(() => {
        if (!cancelled) setGenerationNote("Using the original draft; text generation failed.");
      });
    return () => {
      cancelled = true;
    };
  }, [cwd, fileKey]);
  const ready = !!message.trim() && files.length > 0 && !running;
  const verb = push ? "Commit & Push" : "Commit";

  const commit = async (toDefault: boolean) => {
    const done = await runCommit(
      async () => {
        await request(
          GitMethod.run,
          {
            path: cwd,
            action: push ? "commit_push" : "commit",
            message: message.trim(),
            files,
            stagedOnly: usingStagedIndex,
            newBranch: toDefault ? null : newBranch.trim() || null,
            allowDefaultBranch: toDefault,
          },
          650_000,
        );
      },
      { describe: (error) => gitErrorOf(error).message },
    );
    if (done.status === "done") {
      toast(push ? "Committed and pushed" : "Committed", "success");
      onDone();
      onClose();
    } else if (done.status === "failed") onDone();
  };
  const toggle = (file: GitFile, on: boolean) =>
    setPicked((set) => {
      const next = new Set(set);
      if (on) next.add(file.path);
      else next.delete(file.path);
      return next;
    });

  return (
    <Dialog.Root open onOpenChange={(open) => !open && !running && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center">
          <Dialog.Content
            aria-describedby={undefined}
            className="elevated pointer-events-auto flex max-h-[80%] w-[min(560px,calc(100vw-48px))] flex-col gap-3 p-5"
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && ready) {
                e.preventDefault();
                void commit(false);
              }
            }}
          >
            <Dialog.Title className="font-semibold text-body">
              {verb} on {state.branch}
            </Dialog.Title>
            <textarea
              value={message}
              onChange={(e) => {
                edited.current = true;
                setMessage(e.target.value);
              }}
              rows={3}
              aria-label="Commit message"
              placeholder="What this commit does"
              {...noTextHelpers}
              className="w-full resize-y rounded-md bg-fill-subtle p-2 text-body outline-none"
            />
            {generationNote && (
              <p role="status" className="text-caption text-fg-muted">
                {generationNote}
              </p>
            )}
            <div className="flex min-h-0 flex-col gap-0.5 overflow-y-auto">
              {state.files.map((file) => (
                <label
                  key={file.path}
                  className="flex items-center gap-2 rounded-md px-1 py-0.5 text-callout"
                >
                  <input
                    type="checkbox"
                    className="shrink-0 accent-[var(--rc-accent)]"
                    checked={picked.has(file.path)}
                    onChange={(e) => toggle(file, e.target.checked)}
                  />
                  <span className="w-3 shrink-0 font-mono text-caption text-fg-faint">{file.status}</span>
                  <span className="min-w-0 flex-1 truncate font-mono text-caption">{file.path}</span>
                  {file.staged && (
                    <span className="shrink-0 rounded-sm bg-success/15 px-1.5 py-0.5 text-caption text-success">
                      {file.unstaged ? "partial" : "staged"}
                    </span>
                  )}
                </label>
              ))}
            </div>
            {state.onDefault && (
              <div className="flex flex-col gap-2 rounded-md bg-warning/10 p-3 text-callout">
                <span>
                  You are committing to <b>{state.branch}</b>, the default branch. Name a new branch to commit
                  there instead.
                </span>
                <input
                  aria-label="New branch"
                  className={cx(inputClass, "font-mono")}
                  placeholder="feat/my-change"
                  spellCheck={false}
                  value={newBranch}
                  onChange={(e) => setNewBranch(e.target.value)}
                />
              </div>
            )}
            {failure && (
              <pre className="max-h-40 overflow-auto whitespace-pre-wrap rounded-md bg-danger/10 p-2 font-mono text-caption text-danger">
                {failure}
              </pre>
            )}
            <div className="flex items-center gap-2">
              <span className="flex-1 text-caption text-fg-muted">
                {commitSummaryLabel(files.length, state.files.length, stagedCount, usingStagedIndex)} · hooks
                run as in your terminal
              </span>
              <Pill disabled={running} onClick={onClose}>
                Cancel
              </Pill>
              {state.onDefault ? (
                <>
                  <Pill disabled={!ready} onClick={() => void commit(true)}>
                    {verb} to {state.branch}
                  </Pill>
                  <Pill prominent disabled={!ready || !newBranch.trim()} onClick={() => void commit(false)}>
                    {running && <Spinner size={12} />} {verb} to new branch
                  </Pill>
                </>
              ) : (
                <Pill prominent disabled={!ready} onClick={() => void commit(false)}>
                  {running && <Spinner size={12} />} {verb}
                </Pill>
              )}
            </div>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function PullSheet({
  cwd,
  fallback,
  onClose,
  onDone,
}: {
  cwd: string;
  fallback: string;
  onClose: () => void;
  onDone: () => void;
}) {
  const [draft, setDraft] = useState(() => draftPull(fallback));
  const edited = useRef(false);
  const [note, setNote] = useState("");
  const [creating, runCreate] = useOp();
  const running = opRunning(creating);
  useEffect(() => {
    let cancelled = false;
    void request("_rustcode/textGeneration/generate", { kind: "pull_request", path: cwd }, 50_000)
      .then((answer) => {
        const text = (answer as { text?: string }).text;
        if (acceptsGeneratedDraft(edited.current, cancelled, text)) setDraft(draftPull(text));
      })
      .catch(() => {
        if (!cancelled) setNote("Using the original draft; text generation failed.");
      });
    return () => {
      cancelled = true;
    };
  }, [cwd]);
  async function create() {
    const done = await runCreate(
      async () => {
        const answer = (await request(
          GitMethod.run,
          { action: "create_pr", path: cwd, ...draft },
          330_000,
        )) as { url?: string };
        if (answer.url) void openExternal(answer.url);
      },
      { describe: (error) => gitErrorOf(error).message },
    );
    if (done.status === "done") {
      onDone();
      onClose();
    } else if (done.status === "failed") setNote(done.error);
  }
  return (
    <Dialog.Root open onOpenChange={(open) => !open && !running && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center">
          <Dialog.Content
            aria-describedby={undefined}
            className="elevated pointer-events-auto flex w-[min(560px,calc(100vw-48px))] flex-col gap-3 p-5"
          >
            <Dialog.Title className="font-semibold">Create pull request</Dialog.Title>
            <input
              aria-label="Pull request title"
              className={inputClass}
              value={draft.title}
              onChange={(e) => {
                edited.current = true;
                setDraft({ ...draft, title: e.target.value });
              }}
            />
            <textarea
              aria-label="Pull request body"
              rows={8}
              className={inputClass}
              value={draft.body}
              onChange={(e) => {
                edited.current = true;
                setDraft({ ...draft, body: e.target.value });
              }}
            />
            {note && (
              <p role="status" className="text-caption text-fg-muted">
                {note}
              </p>
            )}
            <div className="flex justify-end gap-2">
              <Pill disabled={running} onClick={onClose}>
                Cancel
              </Pill>
              <Pill prominent disabled={running || !draft.title.trim()} onClick={() => void create()}>
                Create pull request
              </Pill>
            </div>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
