// The overview: the thread's right panel as one scrolling column of sections —
// what it is (Properties), the pull requests its branches have, what it
// delivered, what its turns cost, and the rare facts behind Details — each one
// foldable from its own header, separated by room rather than cards
// (docs/design.md › Right panel). A thread opens on this; Changes, Pull
// requests and a beside thread stay their own views of the panel, reachable
// from the header and from the rows here.

import { useEffect, useMemo, useState } from "react";
import { modelName } from "../acp/types";
import { type GitChanges, gitChanges } from "../lib/native";
import { selectOption } from "../state/harness";
import { useApp, worktreeOf } from "../state/store";
import { DeliverablesSection } from "./OverviewDeliverables";
import { DetailsSection } from "./OverviewDetails";
import { ExecutionLogSection } from "./OverviewLog";
import { PropertiesSection } from "./OverviewProperties";
import { PullRequestsSection } from "./OverviewPulls";
import {
  delegatedIds,
  isFolded as isFoldedOf,
  pullBranches,
  readFolded,
  type OverviewSection as SectionId,
  toggleFolded,
  writeFolded,
} from "./overview";

/** The thread's working tree against HEAD, read once its turn ends, so a finished turn counts. */
function useChanges(cwd: string | undefined, running: boolean) {
  const [changes, setChanges] = useState<GitChanges | null>(null);
  useEffect(() => {
    if (!cwd || running) return;
    let alive = true;
    void gitChanges(cwd).then(
      (result) => alive && setChanges(result),
      () => alive && setChanges(null),
    );
    return () => {
      alive = false;
    };
  }, [cwd, running]);
  return changes;
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
  const changes = useChanges(thread?.cwd, !!thread?.running);
  // Which sections are folded away: the reader's habit, kept for every thread.
  const [folded, setFolded] = useState<SectionId[]>(readFolded);
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

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-2 pt-1 pb-3">
      <div className="flex flex-col gap-4">
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
    </div>
  );
}
