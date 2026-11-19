// Compact workspace actions and context, with the full thread overview available on demand.

import { useEffect, useMemo, useState } from "react";
import { modelName } from "../acp/types";
import { type GitChanges, gitChangeSummary, openPath } from "../lib/native";
import { appPlatform, fileManagerName } from "../lib/platform";
import { contextPercent } from "../state/details";
import { selectOption } from "../state/harness";
import { showPanel, useApp, worktreeOf } from "../state/store";
import { formatTokens, isRunning } from "../state/thread";
import { splitThreadKey } from "../state/threadKeys";
import { delegateBranchCount } from "./DelegateTree";
import { FileDiff, FolderOpen, GitBranch, GitPullRequest, Laptop } from "./icons";
import { DelegatesSection } from "./OverviewDelegates";
import { DeliverablesSection } from "./OverviewDeliverables";
import { DetailsSection } from "./OverviewDetails";
import { ExecutionLogSection } from "./OverviewLog";
import { PropertiesSection } from "./OverviewProperties";
import { PullRequestsSection } from "./OverviewPulls";
import {
  delegatedIds,
  FOLDED_EVENT,
  isFolded as isFoldedOf,
  pullBranches,
  readFolded,
  type OverviewSection as SectionId,
  toggleFolded,
  writeFolded,
} from "./overview";

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

export function ThreadDetails() {
  const id = useApp((s) => s.selectedId);
  const thread = useApp((s) => (s.selectedId ? s.threads[s.selectedId] : undefined));
  const worktree = useApp((s) => (s.selectedId ? worktreeOf(s, s.selectedId) : undefined));
  const pullsOn = useApp((s) => !!s.agent?.pullRequests);
  const links = useApp((s) => s.links.links);
  const sessions = useApp((s) => s.sessions);
  const threads = useApp((s) => s.threads);
  // A delegate's branch is what its pull requests are opened from, so the panel
  // reads the threads it started as well as the one it opens.
  const delegatedBranches = useMemo(
    () =>
      id
        ? delegatedIds(id, links).map(
            (child) =>
              threads[child]?.worktree?.branch ?? sessions.find((x) => x.id === child)?.worktree?.branch,
          )
        : [],
    [id, links, sessions, threads],
  );
  const host = id ? splitThreadKey(id).host : "";
  const machine = useApp((s) => s.machineNames[host]);
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

  const model = useMemo(
    () => modelName(thread ? selectOption(thread.configOptions, "model")?.current : undefined),
    [thread],
  );
  const branch = worktree?.branch ?? changes?.branch;
  const branches = useMemo(() => pullBranches(branch, delegatedBranches), [branch, delegatedBranches]);

  if (!id || !thread) {
    return (
      <p className="px-3 py-4 text-center text-callout text-fg-muted">Open a thread to see its details.</p>
    );
  }

  // A worktree is the project's, checked out from the repository: the card names the project.
  const project = worktree?.repo ?? thread.cwd;

  const usage = thread.usage;
  const used = contextPercent(usage);
  const row = "flex min-h-8 w-full items-center gap-2 px-3 text-left text-callout";
  const action = `${row} hover:bg-fill-subtle`;

  return (
    <div className="min-h-0 overflow-y-auto">
      <div className="py-1">
        <div className={row}>
          <Laptop size={16} />
          {machine ?? (host ? "Paired computer" : appPlatform() === "mac" ? "This Mac" : "This computer")}
        </div>
        {!host && (
          <button type="button" className={action} onClick={() => openPath(thread.cwd)}>
            <FolderOpen size={16} />
            Open in {fileManagerName(appPlatform())}
          </button>
        )}
      </div>
      <div className="border-border border-t py-1">
        {branch && (
          <div className={row} title={branch}>
            <GitBranch size={16} className="shrink-0" />
            <span className="truncate">{branch}</span>
          </div>
        )}
        {!host && (
          <button type="button" className={action} onClick={() => showPanel("changes")}>
            <FileDiff size={16} />
            Changes
            {!!changes?.files.length && (
              <span className="ml-auto tabular text-fg-muted">{changes.files.length}</span>
            )}
          </button>
        )}
        {pullsOn && (
          <button type="button" className={action} onClick={() => showPanel("pulls")}>
            <GitPullRequest size={16} />
            Pull requests
          </button>
        )}
      </div>
      {delegates > 0 && (
        <div className="border-border border-t px-2 py-1">
          <DelegatesSection
            threadId={id}
            count={delegates}
            folded={isFolded("delegates")}
            onToggle={() => toggle("delegates")}
          />
        </div>
      )}
      {usage && used !== undefined && (
        <div className={`${row} border-border border-t py-2 text-fg-muted`}>
          Context
          <div
            role="meter"
            aria-label="Context used"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={used}
            className="h-1 flex-1 overflow-hidden rounded-full bg-fill"
          >
            <div className="h-full rounded-full bg-fg-muted" style={{ width: `${used}%` }} />
          </div>
          <span className="shrink-0 tabular">
            {formatTokens(usage.used)} / {formatTokens(usage.size)}
          </span>
        </div>
      )}
      <details className="border-border border-t">
        <summary className="mx-3 cursor-pointer py-2 text-caption text-fg-muted">More thread details</summary>
        <div className="flex flex-col gap-4 px-2 pb-3">
          <PropertiesSection
            thread={thread}
            branch={branch}
            project={project}
            folded={isFolded("properties")}
            onToggle={() => toggle("properties")}
          />
          {pullsOn && (
            <PullRequestsSection
              cwd={project}
              host={splitThreadKey(id).host}
              branches={branches}
              branch={branch}
              folded={isFolded("pulls")}
              onToggle={() => toggle("pulls")}
            />
          )}
          <DeliverablesSection
            items={thread.items}
            cwd={thread.cwd}
            folded={isFolded("deliverables")}
            onToggle={() => toggle("deliverables")}
          />
          <ExecutionLogSection
            items={thread.items}
            usage={thread.usage}
            harness={thread.harness?.id}
            model={model}
            folded={isFolded("log")}
            onToggle={() => toggle("log")}
          />
          <DetailsSection
            thread={thread}
            changes={changes}
            folded={isFolded("details")}
            onToggle={() => toggle("details")}
          />
        </div>
      </details>
    </div>
  );
}
