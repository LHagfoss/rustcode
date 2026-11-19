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
import { DelegatePreview, delegateBranchCount } from "./DelegateTree";
import { ChevronRight, FileDiff, FolderOpen, GitBranch, GitPullRequest, Laptop } from "./icons";
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
  const row = "flex min-h-8 w-full items-center gap-2 rounded-lg px-2 text-left text-callout";
  const action = `${row} hover:bg-fill-subtle`;
  const icon = "shrink-0 text-fg-muted";

  return (
    <div className="min-h-0 overflow-y-auto">
      <div className="px-2 py-1">
        <div className={row}>
          <Laptop size={16} className={icon} />
          <span className="min-w-0 truncate font-medium">
            {machine ?? (host ? "Paired computer" : appPlatform() === "mac" ? "This Mac" : "This computer")}
          </span>
        </div>
        {!host && (
          <button type="button" className={action} onClick={() => openPath(thread.cwd)}>
            <FolderOpen size={16} className={icon} />
            <span className="flex-1">Open in {fileManagerName(appPlatform())}</span>
            <ChevronRight size={12} className="text-fg-faint" />
          </button>
        )}
      </div>
      <div className="rule-t [--rc-rule-inset:1rem] px-2 py-1">
        {branch && (
          <div className={row} title={branch}>
            <GitBranch size={16} className={icon} />
            <span className="truncate rounded-md bg-fill-subtle px-2 py-0.5 font-mono text-caption">
              {branch}
            </span>
          </div>
        )}
        {!host && (
          <button type="button" className={action} onClick={() => showPanel("changes")}>
            <FileDiff size={16} className={icon} />
            <span className="flex-1">Changes</span>
            {!!changes?.files.length && <span className="tabular text-fg-muted">{changes.files.length}</span>}
            <ChevronRight size={12} className="text-fg-faint" />
          </button>
        )}
        {pullsOn && (
          <button type="button" className={action} onClick={() => showPanel("pulls")}>
            <GitPullRequest size={16} className={icon} />
            <span className="flex-1">Pull requests</span>
            <ChevronRight size={12} className="text-fg-faint" />
          </button>
        )}
      </div>
      {delegates > 0 && (
        <div className="rule-t [--rc-rule-inset:1rem] px-2 py-2">
          <DelegatePreview
            threadId={id}
            expanded={!isFolded("delegates")}
            onToggle={() => toggle("delegates")}
          />
        </div>
      )}
      {usage && used !== undefined && (
        <div className="rule-t [--rc-rule-inset:1rem] flex items-center gap-2 px-4 py-3 text-caption text-fg-muted">
          Context
          <div
            role="meter"
            aria-label="Context used"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={used}
            className="h-1 flex-1 overflow-hidden rounded-full bg-fill"
          >
            <div className="h-full rounded-full bg-accent" style={{ width: `${used}%` }} />
          </div>
          <span className="shrink-0 tabular">
            {formatTokens(usage.used)} / {formatTokens(usage.size)}
          </span>
        </div>
      )}
      <details className="group rule-t [--rc-rule-inset:1rem] px-2 py-1">
        <summary className="flex cursor-pointer list-none items-center gap-2 rounded-lg px-2 py-2 text-caption text-fg-muted hover:bg-fill-subtle [&::-webkit-details-marker]:hidden">
          <ChevronRight size={12} className="group-open:rotate-90" />
          More thread details
        </summary>
        <div className="flex flex-col gap-4 px-2 pb-3">
          <PropertiesSection
            thread={thread}
            branch={branch}
            project={project}
            folded={isFolded("properties")}
            onToggle={() => toggle("properties")}
          />
          {delegates > 0 && (
            <div className="overflow-x-auto">
              <div className="min-w-80">
                <DelegatesSection
                  threadId={id}
                  count={delegates}
                  folded={isFolded("delegates")}
                  onToggle={() => toggle("delegates")}
                />
              </div>
            </div>
          )}
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
