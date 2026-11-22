// The side panel's Details tab, in titled sections: where the thread runs, its branch and
// uncommitted changes, lineage, a few plain stats (agent, messages, tool calls, cost, age) and
// context usage.
// Changes and Pull requests are the panel's other tabs (ui/RightPanel.tsx).

import { type ReactNode, useEffect, useMemo, useState } from "react";
import { modelName } from "../acp/types";
import { type GitChanges, gitChangeSummary, openPath } from "../lib/native";
import { appPlatform, fileManagerName } from "../lib/platform";
import { contextPercent } from "../state/details";
import { formatCost } from "../state/pricing";
import { showPanel, useApp, worktreeOf } from "../state/store";
import { formatTokens, isRunning, type TranscriptItem } from "../state/thread";
import { relativeAge } from "../state/threadFormat";
import { splitThreadKey } from "../state/threadKeys";
import { DelegatePreview, delegateBranchCount } from "./DelegateTree";
import { ChevronRight, FileDiff, FolderOpen, GitBranch, Laptop } from "./icons";
import {
  FOLDED_EVENT,
  isFolded as isFoldedOf,
  readFolded,
  type OverviewSection as SectionId,
  toggleFolded,
  writeFolded,
} from "./overview";
import { cx, Tooltip } from "./primitives";
import { DiffCount } from "./Rows";

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

  const fileCount = changes?.files.length ?? 0;
  const added = changes?.files.reduce((sum, f) => sum + f.added, 0) ?? 0;
  const removed = changes?.files.reduce((sum, f) => sum + f.removed, 0) ?? 0;

  const row = "flex min-h-8 w-full items-center gap-2 rounded-lg px-3 text-left font-medium text-callout";
  const action = `${row} hover:bg-fill-subtle`;
  const icon = "shrink-0 text-fg-muted";

  // Titled sections, each a few 32px rows, a hairline between them: the layout T3 Code's thread
  // details card uses, holding this panel's own facts.
  return (
    <div className="min-h-0 overflow-y-auto pb-2">
      <Section title="Workspace" first>
        <div className={row}>
          <Laptop size={16} className={icon} />
          <span className="min-w-0 truncate">
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
      </Section>
      {branch && (
        <Section title="Version control">
          <Tooltip label={branch}>
            <div className={row}>
              <GitBranch size={16} className={icon} />
              <span className="min-w-0 truncate rounded-md bg-fill-subtle px-2 py-0.5 font-mono text-caption">
                {branch}
              </span>
            </div>
          </Tooltip>
          {changes && (
            <button type="button" className={action} onClick={() => showPanel("changes")}>
              <FileDiff size={16} className={icon} />
              <span className="flex-1">
                {fileCount
                  ? `${fileCount} changed file${fileCount === 1 ? "" : "s"}`
                  : "No uncommitted changes"}
              </span>
              {fileCount > 0 && <DiffCount added={added} removed={removed} />}
              <ChevronRight size={12} className="text-fg-faint" />
            </button>
          )}
        </Section>
      )}
      {delegates > 0 && (
        <Section title="Lineage">
          <div className="px-2">
            <DelegatePreview
              threadId={id}
              expanded={!isFolded("delegates")}
              onToggle={() => toggle("delegates")}
            />
          </div>
        </Section>
      )}
      <Section title="Thread">
        <dl className="grid grid-cols-[auto_1fr] gap-x-4 px-3 font-medium text-callout">
          {stats.map(([label, value]) => (
            <div key={label} className="contents">
              <dt className="flex min-h-8 items-center text-fg-muted">{label}</dt>
              <dd className="flex min-h-8 min-w-0 items-center justify-end truncate tabular">{value}</dd>
            </div>
          ))}
        </dl>
      </Section>
      {usage && used !== undefined && (
        <Section title="Context window">
          <div className="flex min-h-8 items-center gap-2 px-3 font-medium text-callout">
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

/** One titled group of rows; every one after the first starts on a hairline. */
function Section({ title, first, children }: { title: string; first?: boolean; children: ReactNode }) {
  return (
    <section className={cx("px-2 pt-1.5 pb-1", !first && "rule-t [--rc-rule-inset:1rem] mt-1.5")}>
      <h3 className="flex min-h-8 items-center px-3 font-medium text-caption text-fg-muted">{title}</h3>
      {children}
    </section>
  );
}
