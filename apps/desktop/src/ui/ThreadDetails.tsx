// The side panel's home, Multica-style: foldable sections of label and value rows, all on one
// card with no tabs. The thread's properties (status, agent and model, account, machine, project,
// branch), its uncommitted files, its pull requests, lineage, and activity (counts, tokens, cost,
// age, context). The full diff and pull list open from their sections (ui/RightPanel.tsx).

import { type ReactNode, useEffect, useMemo, useState } from "react";
import { pullErrorTitle } from "../acp/rustcode";
import { modelName, projectName } from "../acp/types";
import { STATUS_LABEL } from "../acp/v2";
import { type GitChanges, gitChangeSummary, openExternal, openPath } from "../lib/native";
import { appPlatform, fileManagerName } from "../lib/platform";
import { contextPercent } from "../state/details";
import { formatCost } from "../state/pricing";
import { PULL_LIMIT, usePullList } from "../state/pullQueries";
import { showPanel, useApp, worktreeOf } from "../state/store";
import { formatTokens, isRunning, type TranscriptItem } from "../state/thread";
import { relativeAge } from "../state/threadFormat";
import { splitThreadKey } from "../state/threadKeys";
import { statusOf } from "../state/threadStatus";
import { DelegatePreview, delegateBranchCount } from "./DelegateTree";
import { HarnessIcon } from "./HarnessIcon";
import { ChevronDown, ChevronRight, FileDiff, GitBranch, Laptop } from "./icons";
import { PullRow } from "./OverviewPulls";
import {
  FOLDED_EVENT,
  isFolded as isFoldedOf,
  pullsForBranches,
  readFolded,
  type OverviewSection as SectionId,
  toggleFolded,
  writeFolded,
} from "./overview";
import { ProjectMark } from "./ProjectMark";
import { cx, Tooltip } from "./primitives";
import { DiffCount } from "./Rows";
import { StatusIcon } from "./Status";

/**
 * The thread's working tree against HEAD, read once its turn ends, so a finished turn counts. Only
 * the counts and branch: the panel shows no diff here. Another folder starts empty, so the previous
 * thread's branch is never shown under this one while the read is out.
 */
function useChanges(cwd: string | undefined, running: boolean) {
  const [changes, setChanges] = useState<{ cwd: string; changes: GitChanges | null } | null>(null);
  useEffect(() => {
    if (!cwd || running) return;
    let alive = true;
    void gitChangeSummary(cwd).then(
      (result) => alive && setChanges({ cwd, changes: result }),
      () => alive && setChanges(null),
    );
    return () => {
      alive = false;
    };
  }, [cwd, running]);
  return changes && changes.cwd === cwd ? changes.changes : null;
}

/** What the thread did, counted from its transcript: messages typed, tool calls, first message. */
export function threadCounts(items: readonly TranscriptItem[]) {
  let messages = 0;
  let tools = 0;
  let startedAt: number | undefined;
  for (const item of items) {
    if (item.kind === "user") {
      if (item.wake || item.continued) continue;
      messages++;
      if (startedAt === undefined && item.at) startedAt = item.at;
    } else if (item.kind === "tool") tools++;
  }
  return { messages, tools, startedAt };
}

export function ThreadDetails() {
  const id = useApp((s) => s.selectedId);
  const thread = useApp((s) => (s.selectedId ? s.threads[s.selectedId] : undefined));
  const worktree = useApp((s) => (s.selectedId ? worktreeOf(s, s.selectedId) : undefined));
  const links = useApp((s) => s.links.links);
  const host = id ? splitThreadKey(id).host : "";
  const machine = useApp((s) => s.machineNames[host]);
  const summary = useApp((s) => (s.selectedId ? s.sessions.find((x) => x.id === s.selectedId) : undefined));
  const pullsOn = useApp((s) => !!s.agent?.pullRequests);
  const harnessId = thread?.harness?.id ?? summary?.harness;
  const harnessName = useApp((s) =>
    harnessId ? (s.harnesses?.harnesses.find((h) => h.id === harnessId)?.name ?? harnessId) : undefined,
  );
  const status = useApp((s) => (s.selectedId ? statusOf(s, s.selectedId) : "idle"));
  const items = thread?.items;
  const counts = useMemo(() => threadCounts(items ?? []), [items]);
  const changes = useChanges(host ? undefined : thread?.cwd, isRunning(thread));
  // Which sections are folded away: the reader's habit, kept for every thread.
  const [folded, setFolded] = useState<SectionId[]>(readFolded);
  // A row elsewhere (`+2 nested` in the strip's popover) opens a section: the fold is read again.
  useEffect(() => {
    const reread = () => setFolded(readFolded());
    window.addEventListener(FOLDED_EVENT, reread);
    return () => window.removeEventListener(FOLDED_EVENT, reread);
  }, []);
  // The whole branch the thread handed work to, for the Delegates section's count.
  const delegates = useMemo(() => (id ? delegateBranchCount(id, links) : 0), [id, links]);
  const toggle = (section: SectionId) => {
    setFolded((current) => {
      const next = toggleFolded(current, section);
      writeFolded(next);
      return next;
    });
  };
  const isFolded = (section: SectionId) => isFoldedOf(folded, section);

  const branch = worktree?.branch ?? changes?.branch;

  if (!id || !thread) {
    return (
      <p className="px-3 py-4 text-center text-callout text-fg-muted">Open a thread to see its details.</p>
    );
  }

  const usage = thread.usage;
  const used = contextPercent(usage);
  const model = modelName(summary?.model);
  const account = thread.harness?.account ?? summary?.account;
  const activity: [string, string][] = [
    ["Messages", String(counts.messages)],
    ["Tool calls", String(counts.tools)],
  ];
  if (usage?.cost !== undefined && (!usage.currency || usage.currency === "USD"))
    activity.push(["Cost", formatCost(usage.cost)]);
  if (usage?.inputTokens !== undefined) activity.push(["Tokens in", formatTokens(usage.inputTokens)]);
  if (usage?.outputTokens !== undefined) activity.push(["Tokens out", formatTokens(usage.outputTokens)]);
  if (counts.startedAt) activity.push(["Started", relativeAge(counts.startedAt)]);
  if (summary?.updatedAt) activity.push(["Last active", relativeAge(summary.updatedAt)]);

  const files = changes?.files ?? [];
  const added = files.reduce((sum, f) => sum + f.added, 0);
  const removed = files.reduce((sum, f) => sum + f.removed, 0);
  const shownFiles = files.slice(0, FILE_ROWS);

  const project = projectName(worktree?.repo || thread.cwd);
  const machineLabel =
    machine ?? (host ? "Paired computer" : appPlatform() === "mac" ? "This Mac" : "This computer");
  const icon = "shrink-0 text-fg-muted";
  const openChanges = () => showPanel("changes");

  // Everything about the thread on one card, no tabs: its properties, what it changed, its pull
  // requests, what it did and how full its context is. Rows are tight and the activity is a grid,
  // so the whole card fits the window; the diff and the pull list open from their sections.
  return (
    <div className="min-h-0 flex-1 overflow-y-auto pt-1 pb-2">
      <Section id="properties" title="Properties" folded={isFolded("properties")} onToggle={toggle}>
        <Property label="Status">
          <StatusIcon status={status} size={13} />
          <span className="truncate">{STATUS_LABEL[status]}</span>
        </Property>
        {harnessId && (
          <Property label="Agent" tip={model ? `${harnessName} · ${model}` : undefined}>
            <HarnessIcon id={harnessId} size={14} />
            <span className="shrink-0">{harnessName}</span>
            {model && <span className="truncate text-fg-muted">{model}</span>}
          </Property>
        )}
        {account && (
          <Property label="Account">
            <span className="truncate">{account}</span>
          </Property>
        )}
        <Property label="Machine">
          <Laptop size={14} className={icon} />
          <span className="truncate">{machineLabel}</span>
        </Property>
        <Property
          label="Project"
          onClick={host ? undefined : () => openPath(thread.cwd)}
          tip={host ? undefined : `Open in ${fileManagerName(appPlatform())}`}
        >
          <ProjectMark name={project} />
          <span className="truncate">{project}</span>
        </Property>
        {branch && (
          <Property label="Branch" tip={branch}>
            <GitBranch size={14} className={icon} />
            <span className="truncate font-mono text-caption">{branch}</span>
          </Property>
        )}
      </Section>
      {changes && (
        <Section
          id="changes"
          title="Changes"
          count={files.length || undefined}
          folded={isFolded("changes")}
          onToggle={toggle}
          action={
            <>
              {files.length > 0 && <DiffCount added={added} removed={removed} />}
              <OpenButton label="Open the diff" onClick={openChanges} />
            </>
          }
        >
          {files.length === 0 && (
            <p className="flex min-h-7 items-center gap-2 px-2 text-callout text-fg-muted">
              <FileDiff size={14} className={icon} />
              No uncommitted changes
            </p>
          )}
          {shownFiles.map((file) => (
            <FileRow
              key={file.path}
              path={file.path}
              added={file.added}
              removed={file.removed}
              onOpen={openChanges}
            />
          ))}
          {files.length > shownFiles.length && (
            <button
              type="button"
              onClick={openChanges}
              className="flex min-h-7 w-full items-center rounded-md px-2 text-left text-caption text-fg-muted hover:bg-fill-subtle hover:text-fg"
            >
              {files.length - shownFiles.length} more file{files.length - shownFiles.length === 1 ? "" : "s"}
            </button>
          )}
        </Section>
      )}
      {pullsOn && (
        <PullsSection
          cwd={worktree?.repo || thread.cwd}
          branch={branch}
          folded={isFolded("pulls")}
          onToggle={toggle}
        />
      )}
      {delegates > 0 && (
        <Section id="delegates" title="Lineage" folded={false} onToggle={toggle}>
          <DelegatePreview
            threadId={id}
            expanded={!isFolded("delegates")}
            onToggle={() => toggle("delegates")}
          />
        </Section>
      )}
      <Section id="activity" title="Activity" folded={isFolded("activity")} onToggle={toggle}>
        <dl className="grid grid-cols-3 gap-x-2 gap-y-2 px-2 py-1">
          {activity.map(([label, value]) => (
            <div key={label} className="min-w-0">
              <dt className="truncate text-caption text-fg-muted">{label}</dt>
              <dd className="truncate text-callout text-fg tabular">{value}</dd>
            </div>
          ))}
        </dl>
        {usage && used !== undefined && (
          <div className="flex min-h-8 items-center gap-3 px-2 pt-1 text-callout">
            <span className="shrink-0 text-caption text-fg-muted">Context</span>
            <div
              role="meter"
              aria-label="Context used"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={used}
              className="h-1.5 flex-1 overflow-hidden rounded-full bg-fill"
            >
              <div className="h-full rounded-full bg-accent" style={{ width: `${used}%` }} />
            </div>
            <span className="shrink-0 text-caption text-fg-muted tabular">
              {formatTokens(usage.used)} / {formatTokens(usage.size)}
            </span>
          </div>
        )}
      </Section>
    </div>
  );
}

/** How many changed files the card lists before it points to the diff. */
const FILE_ROWS = 4;
/** How many pull requests the card lists before it points to the full list. */
const PULL_ROWS = 2;

/** A changed file: its name, the folder it sits in, quieter, and its line counts. */
function FileRow({
  path,
  added,
  removed,
  onOpen,
}: {
  path: string;
  added: number;
  removed: number;
  onOpen: () => void;
}) {
  const cut = path.lastIndexOf("/");
  const name = path.slice(cut + 1);
  const folder = cut > 0 ? path.slice(0, cut) : "";
  return (
    <Tooltip label={path}>
      <button
        type="button"
        onClick={onOpen}
        className="flex min-h-7 w-full items-center gap-2 rounded-md px-2 text-left text-callout hover:bg-fill-subtle"
      >
        <span className="shrink-0 truncate">{name}</span>
        <span className="min-w-0 flex-1 truncate text-caption text-fg-faint">{folder}</span>
        <DiffCount added={added} removed={removed} />
      </button>
    </Tooltip>
  );
}

/** The pulls opened from the thread's branch: the first few, the full list one press away. */
function PullsSection({
  cwd,
  branch,
  folded,
  onToggle,
}: {
  cwd: string | undefined;
  branch: string | undefined;
  folded: boolean;
  onToggle: (id: SectionId) => void;
}) {
  const branches = useMemo(() => (branch ? [branch] : []), [branch]);
  const { data, error } = usePullList(cwd, "all", { limit: PULL_LIMIT, enabled: !!branch });
  const rows = useMemo(() => pullsForBranches(data, branches), [data, branches]);
  const openAll = () => showPanel("pulls");
  return (
    <Section
      id="pulls"
      title="Pull requests"
      count={rows.length || undefined}
      folded={folded}
      onToggle={onToggle}
      action={<OpenButton label="All pull requests" onClick={openAll} />}
    >
      {rows.slice(0, PULL_ROWS).map((pull) => (
        <PullRow
          key={pull.number}
          pull={pull}
          onOpen={() => (pull.url ? openExternal(pull.url) : openAll())}
        />
      ))}
      {rows.length > PULL_ROWS && (
        <button
          type="button"
          onClick={openAll}
          className="flex min-h-7 w-full items-center rounded-md px-2 text-left text-caption text-fg-muted hover:bg-fill-subtle hover:text-fg"
        >
          {rows.length - PULL_ROWS} more
        </button>
      )}
      {rows.length === 0 && (
        <p className="flex min-h-7 items-center px-2 text-callout text-fg-muted">
          {error ? pullErrorTitle(error.code) : branch ? "None for this branch yet." : "No branch yet."}
        </p>
      )}
    </Section>
  );
}

/** A section's way to its full view: the diff, the pull list. */
function OpenButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <Tooltip label={label}>
      <button
        type="button"
        aria-label={label}
        onClick={onClick}
        className="flex size-6 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg"
      >
        <ChevronRight size={14} />
      </button>
    </Tooltip>
  );
}

/** A label in the fixed left column and its value beside it; a value that acts gets a hover fill. */
function Property({
  label,
  children,
  onClick,
  tip,
}: {
  label: string;
  children: ReactNode;
  onClick?: () => void;
  tip?: string;
}) {
  const value =
    "flex min-h-7 min-w-0 flex-1 items-center gap-2 rounded-md px-2 text-left text-callout text-fg";
  const body = onClick ? (
    <button type="button" onClick={onClick} className={cx(value, "hover:bg-fill-subtle")}>
      {children}
    </button>
  ) : (
    <div className={value}>{children}</div>
  );
  return (
    <div className="flex items-center gap-1">
      <span className="w-24 shrink-0 px-2 text-callout text-fg-muted">{label}</span>
      {tip ? <Tooltip label={tip}>{body}</Tooltip> : body}
    </div>
  );
}

/** A titled group that folds away from its title; the fold is the reader's habit, kept for every thread. */
function Section({
  id,
  title,
  count,
  folded,
  onToggle,
  action,
  children,
}: {
  id: SectionId;
  title: string;
  count?: number;
  folded: boolean;
  onToggle: (id: SectionId) => void;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="px-2 pt-1.5">
      <div className="flex min-h-7 items-center gap-1 pr-1">
        <button
          type="button"
          aria-expanded={!folded}
          onClick={() => onToggle(id)}
          className="flex min-h-7 items-center gap-1 rounded-md px-2 font-medium text-callout text-fg hover:bg-fill-subtle"
        >
          {title}
          {count !== undefined && <span className="text-caption text-fg-faint tabular">{count}</span>}
          <ChevronDown
            size={12}
            className={cx("text-fg-muted transition-transform", folded && "-rotate-90")}
          />
        </button>
        <span className="flex-1" />
        <div className="flex shrink-0 items-center gap-1.5">{action}</div>
      </div>
      {!folded && <div className="pb-1">{children}</div>}
    </section>
  );
}
