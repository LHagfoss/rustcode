// Pull requests, after PullRequestsView.swift: the project's pull requests
// (`_rustcode/pulls`), one in full (`/view`), and a review the user writes
// and submits (`/review`), and a merge the user confirms (state/pullMerge.ts).
// Nothing here checks out, merges or comments on its own. Picked review feedback goes into the
// thread's composer as a draft; it is never sent from here.

import { Dialog, DropdownMenu } from "radix-ui";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { JsonObject } from "../acp/jsonrpc";
import {
  type PullDetail,
  type PullError,
  type PullRequest,
  parsePullError,
  pullErrorTitle,
  repositoryOf,
} from "../acp/rustcode";
import { Method } from "../acp/types";
import { useIntentRest } from "../lib/intent";
import { openExternal } from "../lib/native";
import { opRunning, useOp } from "../lib/op";
import { count } from "../lib/plural";
import { type FeedbackItem, feedbackItems, feedbackLocation, keepSelected } from "../lib/reviewFeedback";
import {
  advisoryCheck,
  cancelMerge,
  checkName,
  checksHeadline,
  mergeControl,
  mergePull,
  useMerging,
} from "../state/pullMerge";
import {
  PULL_LIMIT,
  type PullListState,
  prefetchPullDetail,
  readPullList,
  usePullDetail,
  usePullList,
} from "../state/pullQueries";
import { pullState } from "../state/pulls";
import {
  DEFAULT_PULL_INSTRUCTION,
  failingChecks,
  linkedPullThread,
  openComments,
  pullThreadMessage,
} from "../state/pullThread";
import {
  openSettings,
  request,
  selectThread,
  setDraft,
  setPanel,
  startNewThread,
  startPullThread,
  toast,
  useApp,
} from "../state/store";
import { relativeAge } from "../state/thread";
import { splitThreadKey, threadKey } from "../state/threadKeys";
import {
  ArrowLeft,
  Bot,
  ChevronDown,
  ChevronRight,
  CircleCheck,
  CircleDashed,
  CirclePlus,
  CircleX,
  ExternalLink,
  GitBranchPlus,
  GitMerge,
  MessageSquare,
  MoreHorizontal,
  RefreshCw,
} from "./icons";
import { Markdown } from "./Markdown";
import { addFeedbackToThread, PullDiff } from "./PullDiff";
import {
  Callout,
  ClusterButton,
  cx,
  MenuContent,
  MenuQuickActions,
  noTextHelpers,
  Pill,
  Segments,
  Skeleton,
  Spinner,
  Tooltip,
} from "./primitives";

/**
 * Reads a row's detail when the pointer or focus rests on it, so the click has
 * it already. Passing over rows reads nothing, and the read is under the same
 * key the click reads, so a rest that ends in a click is never wasted.
 */
function useRowIntent(cwd: string | undefined) {
  return useIntentRest((number: number) => prefetchPullDetail(cwd, number));
}

function ChecksMark({ pull }: { pull: PullRequest }) {
  const c = pull.checks;
  if (c.failed)
    return (
      <span className="flex items-center gap-1 text-danger">
        <CircleX size={12} />
        {c.failed} failing
      </span>
    );
  if (c.pending)
    return (
      <span className="flex items-center gap-1 text-warning">
        <CircleDashed size={12} />
        {c.pending} running
      </span>
    );
  if (c.passed)
    return (
      <span className="flex items-center gap-1 text-success">
        <CircleCheck size={12} />
        {c.passed} passed
      </span>
    );
  return null;
}

const REVIEW_LABEL: Record<string, string> = {
  approved: "Approved",
  changes_requested: "Changes requested",
  review_required: "Review required",
};

/** A pull's own state, in its mark's tone, on the detail panel's header. */
const STATE_LABEL = { open: "Open", draft: "Draft", merged: "Merged", closed: "Closed" } as const;
const STATE_TONE = {
  open: "text-success",
  draft: "text-fg-muted",
  merged: "text-accent",
  closed: "text-danger",
} as const;

function ErrorView({ error }: { error: PullError }) {
  return (
    <div className="p-3">
      <p className="font-medium text-body">{pullErrorTitle(error.code)}</p>
      <p className="mt-1 text-callout text-fg-muted">{error.message}</p>
      {(error.code === "gh_missing" || error.code === "not_connected") && (
        <Pill className="mt-2" onClick={() => openSettings("connections")}>
          Connections…
        </Pill>
      )}
    </div>
  );
}

function ReviewForm({ cwd, pull, onDone }: { cwd: string; pull: PullRequest; onDone: () => void }) {
  const [event, setEvent] = useState<"COMMENT" | "APPROVE" | "REQUEST_CHANGES">("COMMENT");
  const [body, setBody] = useState("");
  const [posting, runPost] = useOp();
  const busy = opRunning(posting);
  const repository = repositoryOf(pull.url);
  const submit = async () => {
    if (!repository) return;
    await runPost(async () => {
      try {
        const result = (await request(
          Method.pullReview,
          { cwd, repository, number: pull.number, commitId: pull.headSha, event, body, comments: [] },
          60_000,
        )) as JsonObject;
        const error = parsePullError(result);
        if (error) toast(`${pullErrorTitle(error.code)}: ${error.message}`, "error");
        else {
          toast("Review posted", "success");
          onDone();
        }
      } catch (e) {
        toast(`Could not post the review: ${e instanceof Error ? e.message : String(e)}`, "error");
      }
    });
  };
  return (
    <div className="mt-3 rounded-lg bg-fill-subtle p-2">
      <div className="mb-2 flex items-center justify-between">
        <span className="font-semibold text-callout">Your review</span>
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
      </div>
      <textarea
        value={body}
        onChange={(e) => setBody(e.target.value)}
        rows={4}
        placeholder="What you want to say about this pull request"
        aria-label="Review"
        {...noTextHelpers}
        className="w-full resize-y rounded-md bg-fill-subtle p-2 text-body outline-none"
      />
      <div className="mt-2 flex items-center gap-2">
        <span className="flex-1 text-caption text-fg-faint">
          Posted as you on {repository ?? "GitHub"} at {pull.headSha.slice(0, 7)}.
        </span>
        <Pill onClick={onDone}>Cancel</Pill>
        <Pill
          prominent
          disabled={busy || !repository || (event !== "APPROVE" && !body.trim())}
          onClick={() => void submit()}
        >
          {busy ? <Spinner size={12} /> : null} Submit review
        </Pill>
      </div>
    </div>
  );
}

function FeedbackCard({
  item,
  checked,
  onToggle,
  onAdd,
}: {
  item: FeedbackItem;
  checked: boolean;
  onToggle: (next: boolean) => void;
  onAdd: () => void;
}) {
  const where = feedbackLocation(item);
  return (
    <div className="group rounded-md bg-fill-subtle p-2">
      <div className="mb-1 flex items-center gap-2 text-caption text-fg-muted">
        <input
          type="checkbox"
          className="shrink-0 accent-[var(--rc-accent)]"
          checked={checked}
          onChange={(e) => onToggle(e.target.checked)}
          aria-label={`Pick ${item.author || "this"} comment`}
        />
        <span className="min-w-0 flex-1 truncate">
          <b className="text-fg">{item.author || "Deleted user"}</b>{" "}
          {item.kind === "line" ? (item.reply ? "replied" : "commented") : item.kind.replace(/_/g, " ")}
          {where && (
            <>
              {" on "}
              <span className="font-mono">{where}</span>
            </>
          )}
          {item.at !== undefined && <> · {relativeAge(item.at)}</>}
        </span>
        {item.outdated && (
          <Tooltip label="The code has changed since this comment">
            <span className="shrink-0 text-warning">Outdated</span>
          </Tooltip>
        )}
        <Tooltip label="Add to the thread's composer">
          <Pill size="sm" quiet onClick={onAdd}>
            <CirclePlus size={12} /> Add to thread
          </Pill>
        </Tooltip>
      </div>
      <Markdown text={item.body} className="text-callout" />
    </div>
  );
}

/** The review feedback the user can pick: line comments, then reviews and comments that say something. */
function FeedbackSection({ detail, pull }: { detail: PullDetail; pull: PullRequest }) {
  const items = useMemo(() => feedbackItems(detail), [detail]);
  const [picked, setPicked] = useState<Set<string>>(() => new Set());
  // A refresh drops picks whose comment is gone (deleted, or a different pull request).
  useEffect(() => setPicked((now) => (now.size ? keepSelected(now, items) : now)), [items]);
  const silent = detail.activity.filter((a) => !a.body.trim());
  const toggle = (key: string, on: boolean) =>
    setPicked((now) => {
      const next = new Set(now);
      if (on) next.add(key);
      else next.delete(key);
      return next;
    });
  const chosen = items.filter((item) => picked.has(item.key));
  return (
    <section className="mt-3 flex flex-col gap-2">
      <h3 className="font-semibold text-caption text-fg-muted">Review feedback</h3>
      {detail.reviewCommentsError && (
        <Callout tone="warning">Line comments could not be read: {detail.reviewCommentsError}</Callout>
      )}
      {items.length === 0 && !detail.reviewCommentsError && (
        <p className="text-callout text-fg-muted">No review comments yet.</p>
      )}
      {items.map((item) => (
        <FeedbackCard
          key={item.key}
          item={item}
          checked={picked.has(item.key)}
          onToggle={(on) => toggle(item.key, on)}
          onAdd={() => addFeedbackToThread([item], pull)}
        />
      ))}
      {silent.map((a, i) => (
        <div key={i} className="px-2 text-caption text-fg-muted">
          <b className="text-fg">{a.author}</b> {a.kind.replace(/_/g, " ").toLowerCase()}
          {a.at !== undefined && <> · {relativeAge(a.at)}</>}
        </div>
      ))}
      {chosen.length > 0 && (
        <div className="sticky bottom-0 flex items-center gap-2 rounded-lg bg-surface p-2 shadow-1">
          <span className="flex-1 text-callout text-fg-muted">{count(chosen.length, "comment")} picked</span>
          <Pill quiet onClick={() => setPicked(new Set())}>
            Clear
          </Pill>
          <Pill
            prominent
            onClick={() => {
              addFeedbackToThread(chosen, pull);
              setPicked(new Set());
            }}
          >
            Add to thread
          </Pill>
        </div>
      )}
    </section>
  );
}

/** How long a pressed merge waits for the second press that confirms it. */
const CONFIRM_MS = 4000;

/**
 * The view's one prominent control (state/pullMerge.ts): Merge once the checks
 * passed, Merge when checks pass while they run, nothing for a draft or a
 * finished pull. The first press arms it ("Confirm merge?"), the second merges;
 * a waiting merge shows here, with a way to stop it, until it ends.
 */
export function MergeButton({
  cwd,
  pull,
  runs,
}: {
  cwd: string;
  pull: PullRequest;
  runs?: PullDetail["checkRuns"];
}) {
  const merging = useMerging(cwd, pull.number);
  const [armed, setArmed] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const control = mergeControl(pull, runs);
  if (merging)
    return (
      <span className="inline-flex items-center rounded-full bg-fill">
        <span className="flex h-[26px] items-center gap-1.5 pl-3 pr-2 font-medium text-callout">
          <Spinner size={12} />
          {control.kind === "merge_when_green" ? "Waiting for checks" : "Merging"}
        </span>
        <Pill quiet className="rounded-l-none" onClick={() => cancelMerge(cwd, pull.number)}>
          Cancel
        </Pill>
      </span>
    );
  if (control.kind === "none") return null;
  // The failing count is the Checks headline's to say; the button only says it cannot merge.
  if (control.kind === "blocked")
    return (
      <Tooltip label="A failing check stops the merge. Fix it, or merge on GitHub.">
        <span className="inline-flex">
          <Pill prominent disabled>
            <GitMerge size={12} /> Merge
          </Pill>
        </span>
      </Tooltip>
    );
  const label = control.kind === "merge" ? "Merge" : "Merge when checks pass";
  // Armed, the button changes colour and keeps saying what the second press does.
  const confirm = control.kind === "merge" ? "Press again to merge" : "Press again to merge when green";
  const press = () => {
    clearTimeout(timer.current);
    if (!armed) {
      setArmed(true);
      timer.current = setTimeout(() => setArmed(false), CONFIRM_MS);
      return;
    }
    setArmed(false);
    void mergePull(cwd, pull);
  };
  return (
    <Tooltip
      label={`Squash-merges into ${pull.baseRef} and deletes ${pull.headRef}${
        control.kind === "merge_when_green" ? " once every check passes" : ""
      }`}
    >
      <Pill prominent={!armed} danger={armed} onClick={press} onBlur={() => setArmed(false)}>
        <GitMerge size={12} /> {armed ? confirm : label}
      </Pill>
    </Tooltip>
  );
}

/**
 * The pull's checks under one headline. While something failed or runs, those
 * rows show and the passed ones fold behind a line; when all passed, the list
 * folds whole behind the headline.
 */
function ChecksSection({ runs }: { runs: PullDetail["checkRuns"] }) {
  const [open, setOpen] = useState(false);
  const attention = runs.filter((c) => c.status !== "passed" && c.status !== "skipped");
  const rest = runs.filter((c) => c.status === "passed" || c.status === "skipped");
  const shown = open ? [...attention, ...rest] : attention;
  return (
    <section className="mt-3">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className="mb-1 flex items-center gap-1 font-semibold text-caption text-fg-muted hover:text-fg"
      >
        <ChevronRight size={12} className={cx("transition-transform", open && "rotate-90")} />
        {checksHeadline(runs)}
        {!open && rest.length > 0 && attention.length > 0 && (
          <span className="font-normal text-fg-faint">· {rest.length} passed</span>
        )}
      </button>
      {shown.map((c, i) => (
        <div key={i} className="flex h-[22px] items-center gap-2 text-callout">
          {c.status === "passed" ? (
            <CircleCheck size={12} className="shrink-0 text-success" />
          ) : c.status === "failed" ? (
            <CircleX size={12} className="shrink-0 text-danger" />
          ) : (
            <CircleDashed size={12} className="shrink-0 text-fg-faint" />
          )}
          {c.url ? (
            <button
              type="button"
              className="truncate text-left hover:underline"
              onClick={() => openExternal(c.url!)}
            >
              {checkName(c.name)}
            </button>
          ) : (
            <span className="truncate">{checkName(c.name)}</span>
          )}
          {advisoryCheck(c.name) && (
            <Tooltip label="Advisory: a merge does not wait for this check">
              <span className="shrink-0 text-caption text-fg-faint">advisory</span>
            </Tooltip>
          )}
        </div>
      ))}
    </section>
  );
}

function PullView({ cwd, host, number }: { cwd: string; host: string; number: number }) {
  // The row the list already read draws the header at once; the body, the
  // files and the activity come from the detail read behind it.
  const { detail, summary, error, live } = usePullDetail(cwd, number);
  const [reviewing, setReviewing] = useState(false);
  const [working, setWorking] = useState(false);
  const [tab, setTab] = useState<"details" | "diff">("details");
  const merges = !!summary && !host && mergeControl(summary, detail?.checkRuns).kind !== "none";
  const askAgent = (pull: PullRequest) => {
    startNewThread(cwd, host || undefined);
    setDraft(
      "new",
      `Review pull request #${pull.number} (${pull.title}) at commit ${pull.headSha.slice(0, 7)}: ${pull.url ?? ""}`,
    );
    setPanel(null);
  };
  return (
    <div className="min-h-0 flex-1 overflow-y-auto p-3">
      {summary ? (
        <>
          <h2 className="selectable font-semibold text-title3">
            {summary.title} <span className="font-normal text-fg-muted">#{summary.number}</span>
          </h2>
          <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-caption">
            <span className={STATE_TONE[pullState(summary)]}>{STATE_LABEL[pullState(summary)]}</span>
            <Tooltip label={`${summary.author} wants to merge ${summary.headRef} into ${summary.baseRef}`}>
              <span className="text-fg-muted">
                {count(summary.changedFiles, "file")}{" "}
                <span className="text-success">+{summary.additions}</span>{" "}
                <span className="text-danger">−{summary.deletions}</span>
              </span>
            </Tooltip>
          </div>
          <div className="mt-2 flex flex-wrap items-center gap-1.5">
            {/* A thread on another machine names a path this engine does not have: it merges there. */}
            {!host && <MergeButton cwd={cwd} pull={summary} runs={detail?.checkRuns} />}
            <DropdownMenu.Root modal={false}>
              <DropdownMenu.Trigger asChild>
                {merges ? (
                  <Pill aria-label="More pull request actions" className="px-2">
                    <MoreHorizontal size={13} />
                  </Pill>
                ) : (
                  // With no merge beside it, a bare "···" reads as nothing: it says what it holds.
                  <Pill>
                    Actions <ChevronDown size={12} />
                  </Pill>
                )}
              </DropdownMenu.Trigger>
              <MenuContent align="start" className="w-[280px]">
                <MenuQuickActions
                  Item={DropdownMenu.Item}
                  actions={[
                    ...(summary.url
                      ? [
                          {
                            label: "GitHub",
                            description: "Open on GitHub",
                            icon: <ExternalLink />,
                            onSelect: () => openExternal(summary.url!),
                          },
                        ]
                      : []),
                    {
                      label: "Ask",
                      description: "Ask an agent to review",
                      icon: <Bot />,
                      onSelect: () => askAgent(summary),
                    },
                    ...(!host && summary.state === "open"
                      ? [
                          {
                            label: "Work",
                            description: "Work on this in a thread",
                            icon: <GitBranchPlus />,
                            onSelect: () => setWorking(true),
                          },
                        ]
                      : []),
                    {
                      label: "Review",
                      description: "Write a review",
                      icon: <MessageSquare />,
                      // A review names the head commit: it waits for a read this session.
                      disabled:
                        reviewing || !live || !repositoryOf(summary.url) || pullState(summary) !== "open",
                      onSelect: () => setReviewing(true),
                    },
                  ]}
                />
              </MenuContent>
            </DropdownMenu.Root>
          </div>
          {reviewing && <ReviewForm cwd={cwd} pull={summary} onDone={() => setReviewing(false)} />}
          {working && (
            <PullThreadSheet cwd={cwd} pull={summary} detail={detail} onClose={() => setWorking(false)} />
          )}
        </>
      ) : error ? (
        <ErrorView error={error} />
      ) : (
        <PullSkeleton />
      )}
      {error && summary && <ErrorView error={error} />}
      {summary && (
        <div className="mt-3">
          <Segments
            label="Pull request view"
            value={tab}
            onChange={setTab}
            options={[
              { value: "details", label: "Details" },
              { value: "diff", label: `Diff · ${summary.changedFiles}` },
            ]}
          />
        </div>
      )}
      {tab === "diff" && summary && (
        <div className="mt-3">
          <PullDiff cwd={cwd} local={!host} pull={summary} detail={detail} />
        </div>
      )}
      {tab === "details" && detail && (
        <>
          {detail.body && (
            <div className="mt-3 rounded-lg bg-fill-subtle p-3">
              <Markdown text={detail.body} className="text-callout" />
            </div>
          )}
          {detail.checkRuns.length > 0 && <ChecksSection runs={detail.checkRuns} />}
          <FeedbackSection detail={detail} pull={summary ?? detail} />
        </>
      )}
    </div>
  );
}

/**
 * "Work on this in a thread": what the new thread starts with (the instruction and the parts
 * ticked), then a worktree on the pull request's branch with that message in its composer. A thread
 * already linked to the pull request is offered first, so it is not opened twice.
 */
function PullThreadSheet({
  cwd,
  pull,
  detail,
  onClose,
}: {
  cwd: string;
  pull: PullRequest;
  detail?: PullDetail;
  onClose: () => void;
}) {
  const comments = openComments(detail);
  const checks = failingChecks(detail);
  const [parts, setParts] = useState({ body: true, comments: true, checks: true });
  const [instruction, setInstruction] = useState(DEFAULT_PULL_INSTRUCTION);
  const [startOp, runStart] = useOp<string | undefined>();
  const starting = opRunning(startOp);
  const [linked, setLinked] = useState<string>();
  useEffect(() => {
    let live = true;
    request("_rustcode/pulls/watches", {}, 15_000)
      .then((answer) => {
        if (live) setLinked(linkedPullThread(answer, pull.number, repositoryOf(pull.url)));
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [pull.number, pull.url]);
  const start = async () => {
    const started = await runStart(() =>
      startPullThread(cwd, pull, (prepared) => pullThreadMessage(pull, detail, parts, instruction, prepared)),
    );
    if (started.status !== "done" || !started.value) return;
    setPanel(null);
    onClose();
  };
  const tick = (key: keyof typeof parts, label: string, n: number | null) => (
    <label className="flex items-center gap-2 text-callout">
      <input
        type="checkbox"
        className="shrink-0 accent-[var(--rc-accent)]"
        checked={parts[key] && n !== 0}
        disabled={n === 0}
        onChange={(e) => setParts((p) => ({ ...p, [key]: e.target.checked }))}
      />
      <span className={cx(n === 0 && "text-fg-faint")}>
        {label}
        {n !== null && <span className="text-fg-muted"> · {n}</span>}
      </span>
    </label>
  );
  return (
    <Dialog.Root open onOpenChange={(open) => !open && !starting && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center">
          <Dialog.Content
            aria-describedby={undefined}
            className="elevated pointer-events-auto flex max-h-[80%] w-[min(520px,calc(100vw-48px))] flex-col gap-3 p-5"
          >
            <Dialog.Title className="font-semibold text-body">
              Work on #{pull.number} in a thread
            </Dialog.Title>
            <p className="text-callout text-fg-muted">
              {pull.title} · a worktree on <span className="font-mono">{pull.headRef}</span>. Pushing from the
              thread updates the pull request, and its checks and comments wake the thread.
            </p>
            {linked && (
              <Callout tone="warning">
                A thread is already linked to this pull request.{" "}
                <button
                  type="button"
                  className="text-accent hover:underline"
                  onClick={() => {
                    selectThread(threadKey("", linked));
                    setPanel(null);
                    onClose();
                  }}
                >
                  Open it
                </button>
              </Callout>
            )}
            <div className="flex flex-col gap-1.5">
              {tick("body", "Description", detail ? (detail.body.trim() ? null : 0) : null)}
              {tick("comments", "Review comments", detail ? comments.length : null)}
              {tick("checks", "Failing checks", detail ? checks.length : null)}
            </div>
            <textarea
              value={instruction}
              onChange={(e) => setInstruction(e.target.value)}
              rows={2}
              aria-label="Instruction"
              {...noTextHelpers}
              className="w-full resize-y rounded-md bg-fill-subtle p-2 text-body outline-none"
            />
            <div className="flex items-center gap-2">
              <span className="flex-1 text-caption text-fg-muted">
                The message waits in the composer for you to send.
              </span>
              <Pill disabled={starting} onClick={onClose}>
                Cancel
              </Pill>
              <Pill prominent disabled={starting} onClick={() => void start()}>
                {starting && <Spinner size={12} />} {linked ? "Start another" : "Start"}
              </Pill>
            </div>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

export function PullsPanel() {
  const selectedId = useApp((s) => s.selectedId);
  const cwd = useApp((s) => (s.selectedId ? s.threads[s.selectedId]?.cwd : undefined) ?? s.projectCwd);
  const host = selectedId ? splitThreadKey(selectedId).host : "";
  const [state, setState] = useState<PullListState>("open");
  // An open pull belongs to the project (and machine) it was opened in: another
  // project's #N is a different pull request, so a switch goes back to the list.
  const scope = `${host}\u0000${cwd ?? ""}`;
  const [opened, setOpened] = useState<{ scope: string; number: number } | null>(null);
  const viewing = opened?.scope === scope ? opened.number : null;
  const setViewing = (number: number | null) => setOpened(number === null ? null : { scope, number });
  // The panel paints the last answer for this state at once and refreshes
  // behind it; the skeleton is only for a first read with nothing saved.
  const { data: pulls, error } = usePullList(cwd, state, { limit: PULL_LIMIT });
  // Only a reload the user asked for spins: a background refresh shows nothing.
  const [asked, setAsked] = useState(false);
  const intent = useRowIntent(cwd);

  const reload = useCallback(() => {
    if (!cwd) return;
    setAsked(true);
    void readPullList(cwd, state, { limit: PULL_LIMIT, force: true }).finally(() => setAsked(false));
  }, [cwd, state]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* The panel's tab already says "Pull requests": the bar holds the state filter or the way back. */}
      <div className="flex h-9 shrink-0 items-center gap-2 px-3">
        {viewing !== null && cwd ? (
          <button
            type="button"
            onClick={() => setViewing(null)}
            className="flex items-center gap-1 text-callout text-fg-muted hover:text-fg"
          >
            <ArrowLeft size={12} /> All pull requests
          </button>
        ) : (
          <Segments
            label="State"
            value={state}
            onChange={setState}
            // A rest on a state reads it: choosing it then draws rows at once.
            onIntent={(next) => cwd && void readPullList(cwd, next, { limit: PULL_LIMIT })}
            options={[
              { value: "open", label: "Open" },
              { value: "merged", label: "Merged" },
              { value: "closed", label: "Closed" },
              { value: "all", label: "All" },
            ]}
          />
        )}
        <span className="flex-1" />
        <ClusterButton label="Reload" onClick={reload}>
          {asked ? <Spinner size={12} /> : <RefreshCw size={13} />}
        </ClusterButton>
      </div>
      {viewing !== null && cwd ? (
        <PullView key={`${scope}#${viewing}`} cwd={cwd} host={host} number={viewing} />
      ) : (
        <div className="min-h-0 flex-1 overflow-y-auto p-2">
          {error && <ErrorView error={error} />}
          {!error && pulls?.length === 0 && (
            <p className="px-2 py-2 text-callout text-fg-muted">No pull requests.</p>
          )}
          {!pulls && !error && <PullListSkeleton />}
          {pulls?.map((pull) => (
            <button
              key={pull.number}
              type="button"
              onClick={() => setViewing(pull.number)}
              onPointerEnter={() => intent.start(pull.number)}
              onPointerLeave={intent.cancel}
              onFocus={() => intent.start(pull.number)}
              onBlur={intent.cancel}
              className="flex w-full flex-col gap-0.5 rounded-md px-2 py-2 text-left hover:bg-fill-subtle"
            >
              <span className="flex w-full items-baseline gap-1.5">
                <span
                  className={cx(
                    "min-w-0 flex-1 truncate font-medium text-body",
                    pull.draft && "text-fg-muted",
                  )}
                >
                  {pull.title}
                </span>
                <span className="shrink-0 text-caption text-fg-faint tabular">#{pull.number}</span>
              </span>
              <span className="flex items-center gap-2 text-caption text-fg-muted">
                <span className="truncate">
                  {[
                    pull.author,
                    pull.draft ? "Draft" : undefined,
                    pull.review ? REVIEW_LABEL[pull.review] : undefined,
                    relativeAge(pull.updatedAt),
                  ]
                    .filter(Boolean)
                    .join(" · ")}
                </span>
                <ChecksMark pull={pull} />
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** Where one pull request will be: its title and meta line, then the body's lines. */
function PullSkeleton() {
  return (
    <div role="status" aria-label="Loading" className="flex flex-col gap-2">
      <Skeleton className="h-4 w-3/5" />
      <Skeleton className="h-2.5 w-4/5" />
      <div className="mt-1 flex gap-1.5">
        <Skeleton className="h-[22px] w-20 rounded-full" />
        <Skeleton className="h-[22px] w-32 rounded-full" />
      </div>
      <div className="mt-2 flex flex-col gap-[5px] rounded-lg bg-fill-subtle p-3">
        {[92, 78, 84, 64, 88, 40].map((w, i) => (
          <Skeleton key={i} className="h-2.5" style={{ width: `${w}%` }} />
        ))}
      </div>
    </div>
  );
}

/** Where the list will be: rows the height of a pull request, its title and meta line. */
function PullListSkeleton() {
  return (
    <div role="status" aria-label="Loading" className="flex flex-col gap-1">
      {[76, 58, 84, 66].map((w, i) => (
        <div key={i} className="flex flex-col gap-1.5 px-2 py-2">
          <Skeleton className="h-3" style={{ width: `${w}%` }} />
          <Skeleton className="h-2.5 w-2/5" />
        </div>
      ))}
    </div>
  );
}
