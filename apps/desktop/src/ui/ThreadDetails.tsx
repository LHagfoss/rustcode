// The side panel's Details tab: where the thread runs, its branch, lineage, a few plain stats
// (agent, messages, tool calls, cost, age) and context usage.
// Changes and Pull requests are the panel's other tabs (ui/RightPanel.tsx).

import { useEffect, useMemo, useState } from "react";
import { modelName } from "../acp/types";
import { type GitChanges, gitChangeSummary, openPath } from "../lib/native";
import { appPlatform, fileManagerName } from "../lib/platform";
import { contextPercent } from "../state/details";
import { formatCost } from "../state/pricing";
import { useApp, worktreeOf } from "../state/store";
import { formatTokens, isRunning, type TranscriptItem } from "../state/thread";
import { relativeAge } from "../state/threadFormat";
import { splitThreadKey } from "../state/threadKeys";
import { DelegatePreview, delegateBranchCount } from "./DelegateTree";
import { ChevronRight, FolderOpen, GitBranch, Laptop } from "./icons";
import {
  FOLDED_EVENT,
  isFolded as isFoldedOf,
  readFolded,
  type OverviewSection as SectionId,
  toggleFolded,
  writeFolded,
} from "./overview";
import { Tooltip } from "./primitives";

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
      {branch && (
        <div className="rule-t [--rc-rule-inset:1rem] px-2 py-1">
          <Tooltip label={branch}>
            <div className={row}>
              <GitBranch size={16} className={icon} />
              <span className="truncate rounded-md bg-fill-subtle px-2 py-0.5 font-mono text-caption">
                {branch}
              </span>
            </div>
          </Tooltip>
        </div>
      )}
      {delegates > 0 && (
        <div className="rule-t [--rc-rule-inset:1rem] px-2 py-2">
          <DelegatePreview
            threadId={id}
            expanded={!isFolded("delegates")}
            onToggle={() => toggle("delegates")}
          />
        </div>
      )}
      <dl className="rule-t [--rc-rule-inset:1rem] grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 px-4 py-3 text-caption">
        {stats.map(([label, value]) => (
          <div key={label} className="contents">
            <dt className="text-fg-faint">{label}</dt>
            <dd className="min-w-0 truncate text-right text-fg-muted tabular">{value}</dd>
          </div>
        ))}
      </dl>
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
    </div>
  );
}
