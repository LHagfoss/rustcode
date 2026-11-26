import { Dialog, DropdownMenu } from "radix-ui";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { PullError } from "../acp/rustcode";
import { openExternal } from "../lib/native";
import { opRunning, useOp } from "../lib/op";
import { count } from "../lib/plural";
import {
  collectIssueLabels,
  DEFAULT_ISSUE_INSTRUCTION,
  fanOutIssueThreads,
  fetchIssues,
  filterIssues,
  type GitHubIssue,
  ISSUE_LIMIT,
  type IssueListState,
  issueBranchSlug,
  issueErrorTitle,
  issuesCwd,
  startIssueThread,
} from "../state/githubIssues";
import {
  harnessesOn,
  newThreadChoice,
  openSettings,
  projectThreadChoice,
  setPanel,
  useApp,
} from "../state/store";
import { relativeAge } from "../state/thread";
import { LOCAL_HOST, splitThreadKey } from "../state/threadKeys";
import {
  ArrowLeft,
  ChevronDown,
  CircleDot,
  ExternalLink,
  GitBranchPlus,
  RefreshCw,
  Search,
  X,
} from "./icons";
import { Markdown } from "./Markdown";
import {
  Callout,
  ClusterButton,
  cx,
  MenuContent,
  menuItem,
  noTextHelpers,
  Pill,
  Segments,
  Skeleton,
  Spinner,
  Tooltip,
} from "./primitives";

const STATE_LABEL = { open: "Open", closed: "Closed" } as const;
const STATE_TONE = { open: "text-success", closed: "text-danger" } as const;

function IssueErrorView({ error }: { error: PullError }) {
  return (
    <div className="p-3">
      <p className="font-medium text-body">{issueErrorTitle(error.code)}</p>
      <p className="mt-1 text-callout text-fg-muted">{error.message}</p>
      {(error.code === "gh_missing" || error.code === "not_connected") && (
        <Pill className="mt-2" onClick={() => openSettings("connections")}>
          Connections…
        </Pill>
      )}
    </div>
  );
}

function IssueListSkeleton() {
  return (
    <div role="status" aria-label="Loading" className="flex flex-col gap-1">
      {[78, 62, 86, 68].map((w, i) => (
        <div key={i} className="flex flex-col gap-1.5 px-2 py-2">
          <Skeleton className="h-3" style={{ width: `${w}%` }} />
          <Skeleton className="h-2.5 w-2/5" />
        </div>
      ))}
    </div>
  );
}

/**
 * Dialog for launching either a single issue thread or parallel worktree threads
 * across multiple selected issues using the chosen harness, model, and instruction.
 */
function IssueLaunchSheet({
  cwd,
  issues,
  onClose,
  onLaunched,
}: {
  cwd: string;
  issues: readonly GitHubIssue[];
  onClose: () => void;
  onLaunched: () => void;
}) {
  const multi = issues.length > 1;
  const snapshot = useApp((s) => harnessesOn(s, LOCAL_HOST));
  const initialChoice = useMemo(() => {
    const s = useApp.getState();
    return (
      projectThreadChoice(s, cwd) ??
      newThreadChoice({
        newThread: s.newThread,
        harnesses: s.harnesses,
        remoteHarnesses: s.remoteHarnesses,
        newThreadHost: LOCAL_HOST,
        projectCwd: cwd,
      })
    );
  }, [cwd]);

  const availableHarnesses = useMemo(
    () => (snapshot?.harnesses ?? []).filter((h) => h.enabled && h.installed),
    [snapshot],
  );
  const [harnessId, setHarnessId] = useState<string>(
    initialChoice?.harness ?? availableHarnesses[0]?.id ?? "",
  );
  const selectedHarness = useMemo(
    () => availableHarnesses.find((h) => h.id === harnessId) ?? availableHarnesses[0],
    [availableHarnesses, harnessId],
  );
  const models = selectedHarness?.models ?? [];
  const [modelId, setModelId] = useState<string | null>(initialChoice?.model ?? null);
  const [worktree, setWorktree] = useState(true);
  const [sendNow, setSendNow] = useState(multi);
  const [instruction, setInstruction] = useState(DEFAULT_ISSUE_INSTRUCTION);
  const [launchOp, runLaunch] = useOp<boolean>();
  const starting = opRunning(launchOp);

  const pickHarness = (nextId: string) => {
    setHarnessId(nextId);
    const nextHarness = availableHarnesses.find((h) => h.id === nextId);
    const nextModel = nextHarness?.last?.model ?? nextHarness?.models[0]?.id ?? null;
    setModelId(nextModel);
  };

  const activeModel = models.find((m) => m.id === modelId);
  const harnessLabel = selectedHarness?.name ?? (harnessId || "Default harness");
  const modelLabel = activeModel?.name ?? modelId ?? "Default model";

  const launch = async () => {
    const result = await runLaunch(async () => {
      if (multi) {
        const outcome = await fanOutIssueThreads(cwd, issues, {
          instruction,
          harness: selectedHarness?.id ?? (harnessId || undefined),
          model: modelId,
          send: sendNow,
        });
        return outcome.created.length > 0;
      }
      const single = issues[0];
      if (!single) return false;
      const id = await startIssueThread(cwd, single, {
        instruction,
        worktree,
        harness: selectedHarness?.id ?? (harnessId || undefined),
        model: modelId,
        send: sendNow,
      });
      return !!id;
    });
    if (result.status !== "done" || !result.value) return;
    onLaunched();
    setPanel(null);
    onClose();
  };

  return (
    <Dialog.Root open onOpenChange={(open) => !open && !starting && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center">
          <Dialog.Content
            aria-describedby={undefined}
            className="elevated pointer-events-auto flex max-h-[82%] w-[min(540px,calc(100vw-48px))] flex-col gap-3 p-5"
          >
            <Dialog.Title className="font-semibold text-body">
              {multi
                ? `Fan out ${count(issues.length, "issue")} into worktree threads`
                : `Work on #${issues[0]?.number} in a thread`}
            </Dialog.Title>
            <p className="text-callout text-fg-muted">
              {multi
                ? "Each selected issue starts in its own isolated git worktree branch with a seeded prompt."
                : `${issues[0]?.title} · branch ${issueBranchSlug(issues[0]!)}`}
            </p>
            {multi && (
              <div className="flex max-h-36 flex-col gap-1 overflow-y-auto rounded-lg bg-fill-subtle p-2">
                {issues.map((issue) => (
                  <div key={issue.number} className="flex items-baseline gap-2 text-callout">
                    <span className="shrink-0 font-mono text-caption text-fg-muted">#{issue.number}</span>
                    <span className="min-w-0 flex-1 truncate">{issue.title}</span>
                  </div>
                ))}
              </div>
            )}
            {availableHarnesses.length > 0 && (
              <div className="flex flex-wrap items-center gap-2">
                <DropdownMenu.Root modal={false}>
                  <DropdownMenu.Trigger asChild>
                    <Pill disabled={starting}>
                      {harnessLabel} <ChevronDown size={12} />
                    </Pill>
                  </DropdownMenu.Trigger>
                  <MenuContent align="start" className="w-56">
                    {availableHarnesses.map((item) => (
                      <DropdownMenu.Item
                        key={item.id}
                        className={menuItem}
                        onSelect={() => pickHarness(item.id)}
                      >
                        <span className="truncate">{item.name}</span>
                      </DropdownMenu.Item>
                    ))}
                  </MenuContent>
                </DropdownMenu.Root>
                {models.length > 0 && (
                  <DropdownMenu.Root modal={false}>
                    <DropdownMenu.Trigger asChild>
                      <Pill disabled={starting}>
                        {modelLabel} <ChevronDown size={12} />
                      </Pill>
                    </DropdownMenu.Trigger>
                    <MenuContent align="start" className="max-h-64 w-64 overflow-y-auto">
                      <DropdownMenu.Item className={menuItem} onSelect={() => setModelId(null)}>
                        <span>Default model</span>
                      </DropdownMenu.Item>
                      {models.map((item) => (
                        <DropdownMenu.Item
                          key={item.id}
                          className={menuItem}
                          onSelect={() => setModelId(item.id)}
                        >
                          <span className="truncate">{item.name}</span>
                        </DropdownMenu.Item>
                      ))}
                    </MenuContent>
                  </DropdownMenu.Root>
                )}
              </div>
            )}
            <div className="flex flex-col gap-1.5">
              {!multi && (
                <label className="flex items-center gap-2 text-callout">
                  <input
                    type="checkbox"
                    className="shrink-0 accent-[var(--rc-accent)]"
                    checked={worktree}
                    disabled={starting}
                    onChange={(e) => setWorktree(e.target.checked)}
                  />
                  <span>Create an isolated git worktree</span>
                </label>
              )}
              <label className="flex items-center gap-2 text-callout">
                <input
                  type="checkbox"
                  className="shrink-0 accent-[var(--rc-accent)]"
                  checked={sendNow}
                  disabled={starting}
                  onChange={(e) => setSendNow(e.target.checked)}
                />
                <span>Start agent turn immediately</span>
              </label>
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
                {sendNow
                  ? "Each prompt is seeded with the issue title, URL, and description and sent right away."
                  : "Each seeded prompt waits in the thread's composer for you to review and send."}
              </span>
              <Pill disabled={starting} onClick={onClose}>
                Cancel
              </Pill>
              <Pill prominent disabled={starting} onClick={() => void launch()}>
                {starting && <Spinner size={12} />}
                {multi ? `Fan out ${issues.length}` : "Start thread"}
              </Pill>
            </div>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function IssueView({ issue, host, onStart }: { issue: GitHubIssue; host: string; onStart: () => void }) {
  return (
    <div className="min-h-0 flex-1 overflow-y-auto p-3">
      <h2 className="selectable font-semibold text-title3">
        {issue.title} <span className="font-normal text-fg-muted">#{issue.number}</span>
      </h2>
      <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-caption text-fg-muted">
        <span className={STATE_TONE[issue.state]}>{STATE_LABEL[issue.state]}</span>
        {issue.author && <span>{issue.author}</span>}
        {issue.updatedAt !== undefined && <span>{relativeAge(issue.updatedAt)}</span>}
        {issue.assignees.length > 0 && <span>Assigned: {issue.assignees.join(", ")}</span>}
      </div>
      {issue.labels.length > 0 && (
        <div className="mt-2 flex flex-wrap gap-1">
          {issue.labels.map((label) => (
            <span
              key={label.name}
              className="rounded-md bg-fill-subtle px-1.5 py-0.5 text-caption text-fg-muted"
            >
              {label.name}
            </span>
          ))}
        </div>
      )}
      <div className="mt-2 flex flex-wrap items-center gap-1.5">
        {!host && (
          <Pill prominent onClick={onStart}>
            <GitBranchPlus size={12} /> Start worktree thread
          </Pill>
        )}
        {issue.url && (
          <Pill onClick={() => openExternal(issue.url!)}>
            <ExternalLink size={12} /> GitHub
          </Pill>
        )}
      </div>
      {host && (
        <Callout className="mt-3">
          Switch to a local project thread to start a worktree thread from this issue.
        </Callout>
      )}
      {issue.body.trim() ? (
        <div className="mt-3 rounded-lg bg-fill-subtle p-3">
          <Markdown text={issue.body} className="text-callout" />
        </div>
      ) : (
        <p className="mt-3 text-callout text-fg-muted">No description provided.</p>
      )}
    </div>
  );
}

/** Browse open GitHub issues, inspect details, and start single or parallel worktree threads. */
export function IssuesPanel() {
  const selectedId = useApp((s) => s.selectedId);
  const cwd = useApp((s) => {
    const thread = s.selectedId ? s.threads[s.selectedId] : undefined;
    return issuesCwd(s.selectedId, thread?.worktree?.repo || thread?.cwd || s.projectCwd);
  });
  const host = selectedId ? splitThreadKey(selectedId).host : "";
  const [state, setState] = useState<IssueListState>("open");
  const [query, setQuery] = useState("");
  const [activeLabel, setActiveLabel] = useState<string | null>(null);
  const [issues, setIssues] = useState<GitHubIssue[] | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [error, setError] = useState<PullError | null>(null);
  const [loading, setLoading] = useState(false);
  const [selectedNumbers, setSelectedNumbers] = useState<Set<number>>(() => new Set());
  const [launching, setLaunching] = useState<GitHubIssue[] | null>(null);

  const scope = `${host}\u0000${cwd ?? ""}`;
  const [opened, setOpened] = useState<{ scope: string; number: number } | null>(null);
  const viewingNumber = opened?.scope === scope ? opened.number : null;
  const setViewingNumber = (number: number | null) => setOpened(number === null ? null : { scope, number });

  const load = useCallback(async (targetCwd: string, targetState: IssueListState) => {
    setLoading(true);
    const result = await fetchIssues(targetCwd, { state: targetState, limit: ISSUE_LIMIT });
    setLoading(false);
    if (result.error) {
      setError(result.error);
      setIssues([]);
      setHasMore(false);
      return;
    }
    setError(null);
    setIssues(result.issues);
    setHasMore(result.hasMore);
  }, []);

  useEffect(() => {
    setSelectedNumbers(new Set());
    setActiveLabel(null);
    if (!cwd) {
      setIssues(null);
      setError(null);
      return;
    }
    let cancelled = false;
    setLoading(true);
    void fetchIssues(cwd, { state, limit: ISSUE_LIMIT }).then((result) => {
      if (cancelled) return;
      setLoading(false);
      if (result.error) {
        setError(result.error);
        setIssues(null);
        setHasMore(false);
        return;
      }
      setError(null);
      setIssues(result.issues);
      setHasMore(result.hasMore);
    });
    return () => {
      cancelled = true;
    };
  }, [cwd, state]);

  const labels = useMemo(() => collectIssueLabels(issues ?? []), [issues]);
  const visibleIssues = useMemo(
    () => filterIssues(issues ?? [], query, activeLabel),
    [issues, query, activeLabel],
  );
  const viewing = useMemo(
    () =>
      viewingNumber !== null ? ((issues ?? []).find((item) => item.number === viewingNumber) ?? null) : null,
    [issues, viewingNumber],
  );
  const chosenIssues = useMemo(
    () => (issues ?? []).filter((item) => selectedNumbers.has(item.number)),
    [issues, selectedNumbers],
  );

  const toggleSelected = (number: number, checked: boolean) => {
    setSelectedNumbers((prev) => {
      const next = new Set(prev);
      if (checked) next.add(number);
      else next.delete(number);
      return next;
    });
  };

  if (host) {
    return (
      <div className="flex min-h-0 flex-1 flex-col p-3">
        <Callout>
          Issues are available for threads on this computer. Switch to a local thread to browse them.
        </Callout>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 items-center gap-2 px-3">
        {viewing ? (
          <button
            type="button"
            onClick={() => setViewingNumber(null)}
            className="flex items-center gap-1 text-callout text-fg-muted hover:text-fg"
          >
            <ArrowLeft size={12} /> All issues
          </button>
        ) : (
          <Segments
            label="Issue state"
            value={state}
            onChange={setState}
            options={[
              { value: "open", label: "Open" },
              { value: "closed", label: "Closed" },
              { value: "all", label: "All" },
            ]}
          />
        )}
        <span className="flex-1" />
        <ClusterButton label="Reload" onClick={() => cwd && void load(cwd, state)}>
          {loading ? <Spinner size={12} /> : <RefreshCw size={13} />}
        </ClusterButton>
      </div>

      {viewing ? (
        <IssueView issue={viewing} host={host} onStart={() => setLaunching([viewing])} />
      ) : (
        <>
          <div className="flex shrink-0 flex-col gap-2 px-3 pb-2">
            <div className="flex items-center gap-1.5 rounded-md bg-fill-subtle px-2 py-1">
              <Search size={12} className="shrink-0 text-fg-muted" />
              <input
                type="text"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Filter issues by #number, title, author, or label…"
                aria-label="Filter issues"
                {...noTextHelpers}
                className="min-w-0 flex-1 bg-transparent text-callout text-fg outline-none placeholder:text-fg-faint"
              />
              {query && (
                <Tooltip label="Clear filter">
                  <button
                    type="button"
                    aria-label="Clear filter"
                    onClick={() => setQuery("")}
                    className="flex size-5 items-center justify-center rounded text-fg-muted hover:text-fg"
                  >
                    <X size={12} />
                  </button>
                </Tooltip>
              )}
            </div>
            {labels.length > 0 && (
              <div className="flex flex-wrap items-center gap-1">
                {labels.slice(0, 12).map((label) => {
                  const active = activeLabel === label;
                  return (
                    <Pill
                      key={label}
                      size="sm"
                      prominent={active}
                      quiet={!active}
                      onClick={() => setActiveLabel(active ? null : label)}
                    >
                      {label}
                    </Pill>
                  );
                })}
              </div>
            )}
          </div>

          <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
            {!cwd && (
              <p className="px-2 py-2 text-callout text-fg-muted">Open a project folder to browse issues.</p>
            )}
            {cwd && error && <IssueErrorView error={error} />}
            {cwd && !error && issues === null && <IssueListSkeleton />}
            {cwd && !error && issues !== null && visibleIssues.length === 0 && (
              <p className="px-2 py-2 text-callout text-fg-muted">
                {issues.length === 0 ? "No issues." : "No issues match the current filter."}
              </p>
            )}
            {visibleIssues.map((issue) => {
              const checked = selectedNumbers.has(issue.number);
              return (
                <div
                  key={issue.number}
                  className="flex items-start gap-2 rounded-md px-2 py-2 hover:bg-fill-subtle"
                >
                  {!host && (
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={(e) => toggleSelected(issue.number, e.target.checked)}
                      aria-label={`Select issue #${issue.number}`}
                      className="mt-1 shrink-0 accent-[var(--rc-accent)]"
                    />
                  )}
                  <button
                    type="button"
                    onClick={() => setViewingNumber(issue.number)}
                    className="flex min-w-0 flex-1 flex-col gap-0.5 text-left"
                  >
                    <span className="flex w-full items-baseline gap-1.5">
                      <CircleDot size={12} className={cx("mt-0.5 shrink-0", STATE_TONE[issue.state])} />
                      <span className="min-w-0 flex-1 truncate font-medium text-body">{issue.title}</span>
                      <span className="shrink-0 text-caption text-fg-faint tabular">#{issue.number}</span>
                    </span>
                    <span className="flex flex-wrap items-center gap-x-2 gap-y-1 text-caption text-fg-muted">
                      <span className="truncate">
                        {[
                          issue.author,
                          issue.assignees.length ? `→ ${issue.assignees.join(", ")}` : undefined,
                          relativeAge(issue.updatedAt),
                        ]
                          .filter(Boolean)
                          .join(" · ")}
                      </span>
                      {issue.labels.slice(0, 3).map((label) => (
                        <span
                          key={label.name}
                          className="rounded bg-fill-subtle px-1 py-0.5 text-caption text-fg-muted"
                        >
                          {label.name}
                        </span>
                      ))}
                    </span>
                  </button>
                </div>
              );
            })}
            {hasMore && (
              <p className="px-2 py-2 text-caption text-fg-faint">Showing the latest {ISSUE_LIMIT} issues.</p>
            )}
          </div>

          {!host && chosenIssues.length > 0 && (
            <div className="flex shrink-0 items-center gap-2 border-t border-border bg-surface px-3 py-2">
              <span className="flex-1 text-callout text-fg-muted">
                {count(chosenIssues.length, "issue")} selected
              </span>
              <Pill quiet onClick={() => setSelectedNumbers(new Set())}>
                Clear
              </Pill>
              <Pill prominent onClick={() => setLaunching(chosenIssues)}>
                <GitBranchPlus size={12} />
                {chosenIssues.length > 1 ? `Fan out (${chosenIssues.length})` : "Start thread"}
              </Pill>
            </div>
          )}
        </>
      )}

      {launching && cwd && (
        <IssueLaunchSheet
          cwd={cwd}
          issues={launching}
          onClose={() => setLaunching(null)}
          onLaunched={() => setSelectedNumbers(new Set())}
        />
      )}
    </div>
  );
}
