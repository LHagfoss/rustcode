// Pull requests, after PullRequestsView.swift: the project's pull requests
// (`_rustcode/pulls`), one in full (`/view`), and a review the user writes
// and submits (`/review`). Read-only otherwise: nothing here checks out,
// merges or comments on its own.

import {
  ArrowLeft,
  CircleCheck,
  CircleDashed,
  CircleX,
  ExternalLink,
  GitPullRequest,
  RefreshCw,
} from "lucide-react";
import { useCallback, useState } from "react";
import type { JsonObject } from "../acp/jsonrpc";
import {
  type PullError,
  type PullRequest,
  parsePullError,
  pullErrorTitle,
  repositoryOf,
} from "../acp/rustcode";
import { Method } from "../acp/types";
import { useIntentRest } from "../lib/intent";
import { openExternal } from "../lib/native";
import {
  PULL_LIMIT,
  type PullListState,
  prefetchPullDetail,
  readPullList,
  usePullDetail,
  usePullList,
} from "../state/pullQueries";
import { pullState } from "../state/pulls";
import { openSettings, request, setDraft, setPanel, startNewThread, toast, useApp } from "../state/store";
import { relativeAge } from "../state/thread";
import { Markdown } from "./Markdown";
import { Cluster, ClusterButton, cx, Pill, Segments, Skeleton, Spinner } from "./primitives";

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
        <CircleX size={11} />
        {c.failed} failing
      </span>
    );
  if (c.pending)
    return (
      <span className="flex items-center gap-1 text-warning">
        <CircleDashed size={11} />
        {c.pending} running
      </span>
    );
  if (c.passed)
    return (
      <span className="flex items-center gap-1 text-success">
        <CircleCheck size={11} />
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
  const [busy, setBusy] = useState(false);
  const repository = repositoryOf(pull.url);
  const submit = async () => {
    if (!repository) return;
    setBusy(true);
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
    } finally {
      setBusy(false);
    }
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
          {busy ? <Spinner size={11} /> : null} Submit review
        </Pill>
      </div>
    </div>
  );
}

function PullView({ cwd, number, onBack }: { cwd: string; number: number; onBack: () => void }) {
  // The row the list already read draws the header at once; the body, the
  // files and the activity come from the detail read behind it.
  const { detail, summary, error, live } = usePullDetail(cwd, number);
  const [reviewing, setReviewing] = useState(false);
  const askAgent = (pull: PullRequest) => {
    startNewThread(cwd);
    setDraft(
      "new",
      `Review pull request #${pull.number} (${pull.title}) at commit ${pull.headSha.slice(0, 7)}: ${pull.url ?? ""}`,
    );
    setPanel(null);
  };
  return (
    <div className="min-h-0 flex-1 overflow-y-auto p-3">
      <button
        type="button"
        onClick={onBack}
        className="mb-2 flex items-center gap-1 text-callout text-fg-muted hover:text-fg"
      >
        <ArrowLeft size={12} /> All pull requests
      </button>
      {summary ? (
        <>
          <h2 className="selectable font-semibold text-title3">
            {summary.title} <span className="font-normal text-fg-muted">#{summary.number}</span>
          </h2>
          <div className="mt-1 flex flex-wrap items-center gap-2 text-caption">
            <span className={STATE_TONE[pullState(summary)]}>{STATE_LABEL[pullState(summary)]}</span>
            <ChecksMark pull={summary} />
          </div>
          <p className="mt-1 text-caption text-fg-muted">
            {summary.author} wants to merge <span className="font-mono">{summary.headRef}</span> into{" "}
            <span className="font-mono">{summary.baseRef}</span> · {summary.changedFiles} files{" "}
            <span className="text-success">+{summary.additions}</span>{" "}
            <span className="text-danger">−{summary.deletions}</span>
          </p>
          <div className="mt-2 flex flex-wrap gap-1.5">
            {summary.url && (
              <Pill onClick={() => openExternal(summary.url!)}>
                <ExternalLink size={11} /> GitHub
              </Pill>
            )}
            <Pill onClick={() => askAgent(summary)}>Ask the agent to review</Pill>
            {!reviewing && (
              <Pill
                prominent
                onClick={() => setReviewing(true)}
                // A review names the head commit: it waits for a read this session,
                // not for a row or a saved copy that may be behind the branch.
                disabled={!live || !repositoryOf(summary.url) || pullState(summary) !== "open"}
              >
                Review…
              </Pill>
            )}
          </div>
          {reviewing && <ReviewForm cwd={cwd} pull={summary} onDone={() => setReviewing(false)} />}
        </>
      ) : error ? (
        <ErrorView error={error} />
      ) : (
        <PullSkeleton />
      )}
      {error && summary && <ErrorView error={error} />}
      {detail && (
        <>
          {detail.body && (
            <div className="mt-3 rounded-lg bg-fill-subtle p-3">
              <Markdown text={detail.body} className="text-callout" />
            </div>
          )}
          {detail.checkRuns.length > 0 && (
            <section className="mt-3">
              <h3 className="mb-1 font-semibold text-caption text-fg-muted">Checks</h3>
              {detail.checkRuns.map((c, i) => (
                <div key={i} className="flex h-[22px] items-center gap-2 text-callout">
                  {c.status === "passed" ? (
                    <CircleCheck size={12} className="text-success" />
                  ) : c.status === "failed" ? (
                    <CircleX size={12} className="text-danger" />
                  ) : (
                    <CircleDashed size={12} className="text-fg-faint" />
                  )}
                  <span className="truncate">{c.name}</span>
                </div>
              ))}
            </section>
          )}
          {detail.files.length > 0 && (
            <section className="mt-3">
              <h3 className="mb-1 font-semibold text-caption text-fg-muted">Files</h3>
              {detail.files.map((f) => (
                <div key={f.path} className="flex h-[20px] items-center gap-2 text-caption">
                  <span className="min-w-0 flex-1 truncate text-left font-mono" dir="rtl">
                    <bdi>{f.path}</bdi>
                  </span>
                  <span className="text-success tabular">+{f.additions}</span>
                  <span className="text-danger tabular">−{f.deletions}</span>
                </div>
              ))}
            </section>
          )}
          {detail.activity.length > 0 && (
            <section className="mt-3 flex flex-col gap-2">
              <h3 className="font-semibold text-caption text-fg-muted">Activity</h3>
              {detail.activity.map((a, i) => (
                <div key={i} className="rounded-md bg-fill-subtle p-2">
                  <div className="mb-1 text-caption text-fg-muted">
                    <b className="text-fg">{a.author}</b> {a.kind.replace(/_/g, " ").toLowerCase()} ·{" "}
                    {relativeAge(a.at)}
                  </div>
                  {a.body && <Markdown text={a.body} className="text-callout" />}
                </div>
              ))}
            </section>
          )}
        </>
      )}
    </div>
  );
}

export function PullsPanel() {
  const cwd = useApp((s) => (s.selectedId ? s.threads[s.selectedId]?.cwd : undefined) ?? s.projectCwd);
  const [state, setState] = useState<PullListState>("open");
  const [viewing, setViewing] = useState<number | null>(null);
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
      <div className="flex h-9 shrink-0 items-center gap-2 px-3">
        <span className="flex items-center gap-1.5 font-semibold text-body">
          <GitPullRequest size={14} /> Pull requests
        </span>
        <span className="flex-1" />
        <Cluster>
          <ClusterButton label="Reload" onClick={reload}>
            {asked ? <Spinner size={12} /> : <RefreshCw size={13} />}
          </ClusterButton>
        </Cluster>
      </div>
      {viewing !== null && cwd ? (
        <PullView cwd={cwd} number={viewing} onBack={() => setViewing(null)} />
      ) : (
        <>
          <div className="px-3 pt-2">
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
          </div>
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
        </>
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
