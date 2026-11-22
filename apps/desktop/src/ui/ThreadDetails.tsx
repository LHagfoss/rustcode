// The side panel's Details tab, Multica-style: foldable sections of label and value rows. The
// thread's properties (status, agent, model, account, machine, project, branch), its uncommitted
// changes, lineage, activity (messages, tool calls, tokens, cost, age) and context usage.
// Changes and Pull requests are the panel's other tabs (ui/RightPanel.tsx).

import { type ReactNode, useEffect, useMemo, useState } from "react";
import { modelName, projectName } from "../acp/types";
import { STATUS_LABEL } from "../acp/v2";
import { type GitChanges, gitChangeSummary, openPath } from "../lib/native";
import { appPlatform, fileManagerName } from "../lib/platform";
import { contextPercent } from "../state/details";
import { formatCost } from "../state/pricing";
import { showPanel, useApp, worktreeOf } from "../state/store";
import { formatTokens, isRunning, type TranscriptItem } from "../state/thread";
import { relativeAge } from "../state/threadFormat";
import { splitThreadKey } from "../state/threadKeys";
import { statusOf } from "../state/threadStatus";
import { DelegatePreview, delegateBranchCount } from "./DelegateTree";
import { HarnessIcon } from "./HarnessIcon";
import { ChevronDown, ChevronRight, FileDiff, GitBranch, Laptop } from "./icons";
import {
  FOLDED_EVENT,
  isFolded as isFoldedOf,
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
  const stats: [string, string][] = [];
  if (harnessName || model) stats.push(["Agent", [harnessName, model].filter(Boolean).join(" · ")]);
  if (account) stats.push(["Account", account]);
  stats.push(["Messages", String(counts.messages)]);
  stats.push(["Tool calls", String(counts.tools)]);
  if (usage?.inputTokens !== undefined || usage?.outputTokens !== undefined)
    stats.push([
      "Tokens",
      `${formatTokens(usage.inputTokens ?? 0)} in · ${formatTokens(usage.outputTokens ?? 0)} out`,
    ]);
  if (usage?.cost !== undefined && (!usage.currency || usage.currency === "USD"))
    stats.push(["Cost", formatCost(usage.cost)]);
  if (counts.startedAt) stats.push(["Started", relativeAge(counts.startedAt)]);
  if (summary?.updatedAt) stats.push(["Last active", relativeAge(summary.updatedAt)]);

  const fileCount = changes?.files.length ?? 0;
  const added = changes?.files.reduce((sum, f) => sum + f.added, 0) ?? 0;
  const removed = changes?.files.reduce((sum, f) => sum + f.removed, 0) ?? 0;

  const project = projectName(worktree?.repo || thread.cwd);
  const machineLabel =
    machine ?? (host ? "Paired computer" : appPlatform() === "mac" ? "This Mac" : "This computer");
  const icon = "shrink-0 text-fg-muted";
  const activity = stats.filter(([label]) => label !== "Agent" && label !== "Account");

  // Multica-style sections: a title you can fold, then label and value rows, the label in a fixed
  // column and the value beside it with its icon, so the eye runs down one edge.
  return (
    <div className="min-h-0 flex-1 overflow-y-auto pb-3">
      <Section id="properties" title="Properties" folded={isFolded("properties")} onToggle={toggle}>
        <Property label="Status">
          <StatusIcon status={status} size={13} />
          <span className="truncate">{STATUS_LABEL[status]}</span>
        </Property>
        {harnessId && (
          <Property label="Agent">
            <HarnessIcon id={harnessId} size={14} />
            <span className="truncate">{harnessName}</span>
          </Property>
        )}
        {model && (
          <Property label="Model">
            <span className="truncate">{model}</span>
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
          folded={isFolded("changes")}
          onToggle={toggle}
          action={
            <Tooltip label="Open Changes">
              <button
                type="button"
                aria-label="Open Changes"
                onClick={() => showPanel("changes")}
                className="flex size-6 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg"
              >
                <ChevronRight size={14} />
              </button>
            </Tooltip>
          }
        >
          <button
            type="button"
            onClick={() => showPanel("changes")}
            className="flex min-h-8 w-full items-center gap-2 rounded-md px-2 text-left text-callout hover:bg-fill-subtle"
          >
            <FileDiff size={14} className={icon} />
            <span className="flex-1 truncate">
              {fileCount
                ? `${fileCount} changed file${fileCount === 1 ? "" : "s"}`
                : "No uncommitted changes"}
            </span>
            {fileCount > 0 && <DiffCount added={added} removed={removed} />}
          </button>
        </Section>
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
        {activity.map(([label, value]) => (
          <Property key={label} label={label}>
            <span className="truncate tabular">{value}</span>
          </Property>
        ))}
      </Section>
      {usage && used !== undefined && (
        <Section id="context" title="Context window" folded={isFolded("context")} onToggle={toggle}>
          <div className="flex min-h-8 items-center gap-3 px-2 text-callout">
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
            <span className="shrink-0 text-fg-muted tabular">
              {formatTokens(usage.used)} / {formatTokens(usage.size)}
            </span>
          </div>
        </Section>
      )}
    </div>
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
    "flex min-h-8 min-w-0 flex-1 items-center gap-2 rounded-md px-2 text-left text-callout text-fg";
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
  folded,
  onToggle,
  action,
  children,
}: {
  id: SectionId;
  title: string;
  folded: boolean;
  onToggle: (id: SectionId) => void;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="px-2 pt-2">
      <div className="flex min-h-8 items-center gap-1 pr-1">
        <button
          type="button"
          aria-expanded={!folded}
          onClick={() => onToggle(id)}
          className="flex min-h-7 items-center gap-1 rounded-md px-2 font-medium text-callout text-fg hover:bg-fill-subtle"
        >
          {title}
          <ChevronDown
            size={12}
            className={cx("text-fg-muted transition-transform", folded && "-rotate-90")}
          />
        </button>
        <span className="flex-1" />
        {action}
      </div>
      {!folded && <div className="pb-1">{children}</div>}
    </section>
  );
}
