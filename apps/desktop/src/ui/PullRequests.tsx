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
import { useCallback, useEffect, useState } from "react";
import type { JsonObject } from "../acp/jsonrpc";
import {
  type PullDetail,
  type PullError,
  type PullRequest,
  parsePullDetail,
  parsePullError,
  parsePullList,
  pullErrorTitle,
  repositoryOf,
} from "../acp/rustcode";
import { Method } from "../acp/types";
import { openExternal } from "../lib/native";
import { openSettings, request, setDraft, setPanel, startNewThread, toast, useApp } from "../state/store";
import { relativeAge } from "../state/thread";
import { Markdown } from "./Markdown";
import { Cluster, ClusterButton, cx, Pill, Segments, Skeleton, Spinner } from "./primitives";

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

function ReviewForm({ cwd, pull, onDone }: { cwd: string; pull: PullDetail; onDone: () => void }) {
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
  const [detail, setDetail] = useState<PullDetail | null>(null);
  const [error, setError] = useState<PullError | null>(null);
  const [reviewing, setReviewing] = useState(false);
  useEffect(() => {
    let live = true;
    void request(Method.pullView, { cwd, number }, 60_000).then(
      (result) => {
        if (!live) return;
        setError(parsePullError(result));
        setDetail(parsePullDetail(result));
      },
      (e) => live && setError({ code: "failed", message: e instanceof Error ? e.message : String(e) }),
    );
    return () => {
      live = false;
    };
  }, [cwd, number]);
  const askAgent = (pull: PullDetail) => {
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
      {error && <ErrorView error={error} />}
      {!detail && !error && <PullSkeleton />}
      {detail && (
        <>
          <h2 className="selectable font-semibold text-title3">
            {detail.title} <span className="font-normal text-fg-muted">#{detail.number}</span>
          </h2>
          <p className="mt-1 text-caption text-fg-muted">
            {detail.author} wants to merge <span className="font-mono">{detail.headRef}</span> into{" "}
            <span className="font-mono">{detail.baseRef}</span> · {detail.changedFiles} files{" "}
            <span className="text-success">+{detail.additions}</span>{" "}
            <span className="text-danger">−{detail.deletions}</span>
          </p>
          <div className="mt-2 flex flex-wrap gap-1.5">
            {detail.url && (
              <Pill onClick={() => openExternal(detail.url!)}>
                <ExternalLink size={11} /> GitHub
              </Pill>
            )}
            <Pill onClick={() => askAgent(detail)}>Ask the agent to review</Pill>
            {!reviewing && (
              <Pill
                prominent
                onClick={() => setReviewing(true)}
                disabled={!repositoryOf(detail.url) || detail.state !== "open"}
              >
                Review…
              </Pill>
            )}
          </div>
          {reviewing && <ReviewForm cwd={cwd} pull={detail} onDone={() => setReviewing(false)} />}
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
  const [state, setState] = useState<"open" | "closed" | "merged" | "all">("open");
  const [pulls, setPulls] = useState<PullRequest[] | null>(null);
  const [error, setError] = useState<PullError | null>(null);
  const [loading, setLoading] = useState(false);
  const [viewing, setViewing] = useState<number | null>(null);

  const load = useCallback(async () => {
    if (!cwd) return;
    setLoading(true);
    try {
      const result = await request(Method.pulls, { cwd, state, limit: 30 }, 60_000);
      setError(parsePullError(result));
      setPulls(parsePullList(result).pulls);
    } catch (e) {
      setError({ code: "failed", message: e instanceof Error ? e.message : String(e) });
    } finally {
      setLoading(false);
    }
  }, [cwd, state]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 items-center gap-2 px-3">
        <span className="flex items-center gap-1.5 font-semibold text-body">
          <GitPullRequest size={14} /> Pull requests
        </span>
        <span className="flex-1" />
        <Cluster>
          <ClusterButton label="Reload" onClick={() => void load()}>
            {loading ? <Spinner size={12} /> : <RefreshCw size={13} />}
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
