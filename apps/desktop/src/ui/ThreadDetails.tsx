// The side panel's home, Multica-style: foldable sections of label and value rows, all on one
// card with no tabs. The thread's properties (status, agent and model, account, machine, project,
// branch), its uncommitted files, its pull requests, lineage, and activity (counts, tokens, cost,
// age, context). The full diff and pull list open from their sections (ui/RightPanel.tsx).

import { type ReactNode, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { pullErrorTitle } from "../acp/rustcode";
import { modelName, projectName, type SessionUsage } from "../acp/types";
import { STATUS_LABEL } from "../acp/v2";
import { type GitChanges, gitChangeSummary, openExternal, openPath } from "../lib/native";
import { appPlatform, fileManagerName } from "../lib/platform";
import { useNow } from "../lib/useNow";
import { contextPercent } from "../state/details";
import { turnItemsByEnd } from "../state/fold";
import { formatCost } from "../state/pricing";
import { PULL_LIMIT, usePullList } from "../state/pullQueries";
import { engineBusy } from "../state/runState";
import { showPanel, useApp, worktreeOf } from "../state/store";
import { formatDuration, formatTokens, isLoading, isRunning, type TranscriptItem } from "../state/thread";
import { relativeAge } from "../state/threadFormat";
import { splitThreadKey } from "../state/threadKeys";
import { statusOf } from "../state/threadStatus";
import { DelegatePreview, delegateBranchCount } from "./DelegateTree";
import { HarnessIcon } from "./HarnessIcon";
import { ChevronDown, ChevronRight, CircleCheck, CircleSlash, CircleX, GitBranch, Laptop } from "./icons";
import { PullRow } from "./OverviewPulls";
import {
  FOLDED_EVENT,
  isFolded as isFoldedOf,
  pullsForBranches,
  readFolded,
  type OverviewSection as SectionId,
  type TurnEntry,
  toggleFolded,
  turnEntries,
  turnHasTrace,
  writeFolded,
} from "./overview";
import { ProjectMark } from "./ProjectMark";
import { cx, Tooltip } from "./primitives";
import { DiffCount } from "./Rows";
import { StatusIcon } from "./Status";
import { useStoredTurns, withStoredTurns } from "./storedTurns";
import { ThreadPortsSection } from "./ThreadPorts";
import { type LogTurn, TurnEvents } from "./TurnEvents";
import { TurnTrace } from "./TurnTrace";
import { contextTone, costTotal, threadBilling, threadTotals, turnCost } from "./threadTotals";
import { WorktreeScriptsSection } from "./WorktreeScriptsSection";

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
  const harnessEntry = useApp((s) =>
    harnessId ? s.harnesses?.harnesses.find((h) => h.id === harnessId) : undefined,
  );
  const harnessName = harnessId ? (harnessEntry?.name ?? harnessId) : undefined;
  const status = useApp((s) => (s.selectedId ? statusOf(s, s.selectedId) : "idle"));
  const items = thread?.items;
  const counts = useMemo(() => threadCounts(items ?? []), [items]);
  const running = isRunning(thread) || isLoading(thread) || !!thread?.enginePhase || engineBusy(status);
  const changes = useChanges(host ? undefined : thread?.cwd, isRunning(thread));
  const entries = useMemo(() => turnEntries(items ?? [], running), [items, running]);
  const stored = useStoredTurns(id ?? undefined, entries.length);
  const turns = useMemo(() => withStoredTurns(entries, stored), [entries, stored]);
  const liveStart = thread?.turn?.startedAt ?? thread?.enginePhase?.startedAt;
  const now = useNow(liveStart !== undefined);
  const totals = useMemo(
    () =>
      threadTotals(
        turns,
        thread?.usage,
        harnessId,
        summary?.model,
        liveStart === undefined ? 0 : Math.max(0, now - liveStart),
      ),
    [turns, thread?.usage, harnessId, summary?.model, liveStart, now],
  );
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
  const billing = threadBilling(harnessEntry, account);
  const activity: [string, string][] = [
    ["Messages", String(counts.messages)],
    ["Tool calls", String(counts.tools)],
  ];
  if (counts.startedAt) activity.push(["Started", relativeAge(counts.startedAt)]);
  if (summary?.updatedAt) activity.push(["Last active", relativeAge(summary.updatedAt)]);

  const files = changes?.files ?? [];
  const added = files.reduce((sum, f) => sum + f.added, 0);
  const removed = files.reduce((sum, f) => sum + f.removed, 0);

  const project = projectName(worktree?.repo || thread.cwd);
  const machineLabel =
    machine ?? (host ? "Paired computer" : appPlatform() === "mac" ? "This Mac" : "This computer");
  const icon = "shrink-0 text-fg-muted";
  const openChanges = () => showPanel("changes");

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto pb-2 [&>*]:shrink-0">
      <div className="px-2">
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
        {changes && (
          <Property label="Changes" onClick={openChanges} tip="Open the diff">
            {files.length ? (
              <>
                <span className="shrink-0">
                  {files.length} file{files.length === 1 ? "" : "s"}
                </span>
                <DiffCount added={added} removed={removed} />
              </>
            ) : (
              <span className="truncate text-fg-muted">None uncommitted</span>
            )}
            <span className="flex-1" />
            <ChevronRight size={14} className={icon} />
          </Property>
        )}
        <Property label="Preview" onClick={() => showPanel("preview")}>
          <span className="truncate text-fg-muted">Browser panel</span>
          <span className="flex-1" />
          <ChevronRight size={14} className={icon} />
        </Property>
      </div>
      <WorktreeScriptsSection cwd={host ? undefined : thread.cwd} />
      <ThreadPortsSection cwd={host ? undefined : thread.cwd} />
      {pullsOn && (
        <PullsSection
          cwd={worktree?.repo || thread.cwd}
          remote={!!host}
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
        <dl className="mx-2 mb-2 grid grid-cols-3 divide-x divide-border rounded-lg bg-fill-subtle py-2">
          <Total {...costTotal(totals, billing, formatCost)} />
          <Total label="Worked" value={formatDuration(totals.workedMs)} tip="Time the agent spent on turns" />
          <Total
            label="Tokens"
            value={totals.tokens === undefined ? "–" : formatTokens(totals.tokens)}
            tip={
              usage?.inputTokens !== undefined || usage?.outputTokens !== undefined
                ? `${formatTokens(usage.inputTokens ?? 0)} in · ${formatTokens(usage.outputTokens ?? 0)} out`
                : "Input and output tokens"
            }
          />
        </dl>
        <dl className="grid grid-cols-3 gap-x-2 gap-y-2 px-2 py-1">
          {activity.map(([label, value]) => (
            <div key={label} className="min-w-0">
              <dt className="truncate text-caption text-fg-muted">{label}</dt>
              <dd className="truncate text-callout text-fg tabular">{value}</dd>
            </div>
          ))}
        </dl>
        {usage && used !== undefined && <ContextMeter used={used} usage={usage} />}
      </Section>
      <TurnsSection
        key={id}
        turns={turns}
        items={items ?? []}
        running={running}
        model={model}
        harness={harnessId}
        rawModel={summary?.model}
        folded={isFolded("log")}
        onToggle={toggle}
      />
    </div>
  );
}

/** How many pull requests the card lists before it points to the full list. */
const PULL_ROWS = 2;

/** The pulls opened from the thread's branch: the first few, the full list one press away. */
function PullsSection({
  cwd,
  remote,
  branch,
  folded,
  onToggle,
}: {
  cwd: string | undefined;
  remote: boolean;
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
      action={
        <div className="flex items-center gap-1">
          {!remote && <OpenButton label="Issues" onClick={() => showPanel("issues")} />}
          <OpenButton label="All pull requests" onClick={openAll} />
        </div>
      }
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

/** The event log of every turn rather than one. */
const ALL = "all";

/** A turn's outcome: its icon, its colour and what its tip says. */
const OUTCOME = {
  completed: { icon: CircleCheck, tone: "text-success", label: "Completed" },
  cancelled: { icon: CircleSlash, tone: "text-fg-muted", label: "Cancelled" },
  failed: { icon: CircleX, tone: "text-danger", label: "Failed" },
} as const;

/** A turn's price: reported, else estimated (marked `~`), else a dash, so every row has one. */
function turnPrice(turn: TurnEntry, harness: string | undefined, model: string | undefined): string {
  const cost = turnCost(turn, harness, model);
  if (!cost) return "$–";
  return `${cost.estimated ? "~" : ""}${formatCost(cost.usd)}`;
}

/** Finished turns, newest first. Open a row for its messages and calls. */
function TurnsSection({
  turns: oldestFirst,
  items,
  running,
  model,
  harness,
  rawModel,
  folded,
  onToggle,
}: {
  turns: readonly TurnEntry[];
  items: readonly TranscriptItem[];
  running?: boolean;
  model?: string;
  harness?: string;
  /** The model's id as the harness names it, for list prices. */
  rawModel?: string;
  folded: boolean;
  onToggle: (id: SectionId) => void;
}) {
  const turns = useMemo(() => [...oldestFirst].reverse(), [oldestFirst]);
  const byEnd = useMemo(() => turnItemsByEnd(items, undefined, running), [items, running]);
  const ends = useMemo(() => {
    const map = new Map<string, Extract<TranscriptItem, { kind: "turnEnd" }>>();
    for (const item of items) if (item.kind === "turnEnd") map.set(item.id, item);
    return map;
  }, [items]);
  const [open, setOpen] = useState<string | null>(null);
  const opened = open && open !== ALL ? turns.find((turn) => turn.id === open) : undefined;
  const openedEnd = opened ? ends.get(opened.id) : undefined;
  const logTurn = (turn: (typeof turns)[number]): LogTurn => ({ ...turn, items: byEnd[turn.id] ?? [] });
  const log =
    open === ALL
      ? { title: "All turns", turns: [...turns].reverse().map(logTurn) }
      : opened
        ? { title: opened.prompt, turns: [logTurn(opened)] }
        : undefined;
  return (
    <Section
      id="log"
      title="Turns"
      count={turns.length || undefined}
      action={
        turns.length > 1 ? (
          <Tooltip label="Open the transcript for all turns">
            <button
              type="button"
              onClick={() => setOpen(ALL)}
              className="flex h-6 items-center rounded-md px-2 text-caption text-fg-muted hover:bg-fill hover:text-fg"
            >
              All turns
            </button>
          </Tooltip>
        ) : undefined
      }
      folded={folded}
      onToggle={onToggle}
      grow
    >
      <TurnScroll>
        {turns.length === 0 && (
          <p className="flex min-h-7 items-center px-2 text-callout text-fg-muted">No turns yet.</p>
        )}
        {turns.map((turn) => {
          const outcome = OUTCOME[turn.outcome];
          const Icon = outcome.icon;
          return (
            <button
              key={turn.id}
              type="button"
              aria-haspopup="dialog"
              onClick={() => setOpen(turn.id)}
              className="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left hover:bg-fill-subtle"
            >
              <span
                role="img"
                aria-label={outcome.label}
                className={cx("flex size-4 shrink-0 items-center justify-center", outcome.tone)}
              >
                <Icon size={13} />
              </span>
              <span className="flex min-w-0 flex-1 flex-col">
                <span className="truncate text-callout">{turn.prompt}</span>
                <span className="truncate text-caption text-fg-muted tabular">
                  {[
                    turn.origin,
                    turn.elapsedMs === undefined ? undefined : formatDuration(turn.elapsedMs),
                    turnPrice(turn, harness, rawModel),
                  ]
                    .filter(Boolean)
                    .join(" · ")}
                </span>
              </span>
              <ChevronRight size={14} className="shrink-0 text-fg-muted" />
            </button>
          );
        })}
      </TurnScroll>
      {log && (
        <TurnEvents
          open
          onOpenChange={(next) => !next && setOpen(null)}
          title={log.title}
          turns={log.turns}
          trace={
            openedEnd && opened && turnHasTrace(opened) ? (
              <TurnTrace end={openedEnd} items={byEnd[opened.id]} model={model} />
            ) : undefined
          }
        />
      )}
    </Section>
  );
}

function TurnScroll({ children }: { children: ReactNode }) {
  const viewport = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const node = viewport.current;
    const rows = content.current;
    if (!node || !rows) return;

    const update = () => {
      node.toggleAttribute("data-more-above", node.scrollTop > 1);
      node.toggleAttribute("data-more-below", node.scrollHeight - node.clientHeight - node.scrollTop > 1);
    };
    const observer = new ResizeObserver(update);
    observer.observe(node);
    observer.observe(rows);
    node.addEventListener("scroll", update, { passive: true });
    update();
    return () => {
      observer.disconnect();
      node.removeEventListener("scroll", update);
    };
  }, []);

  return (
    <div
      ref={viewport}
      role="region"
      aria-label="Turn history"
      className="turn-history min-h-0 flex-1 overflow-y-auto overscroll-y-contain scroll-py-5"
    >
      <div ref={content} className="py-1">
        {children}
      </div>
    </div>
  );
}

/** One of the Activity section's headline numbers: a small label over a large value. */
function Total({ label, value, tip }: { label: string; value: string; tip: string }) {
  return (
    <Tooltip label={tip}>
      <div className="flex min-w-0 flex-col items-center gap-0.5 px-2">
        <dt className="truncate text-caption text-fg-muted">{label}</dt>
        <dd className="truncate font-medium text-body text-fg tabular">{value}</dd>
      </div>
    </Tooltip>
  );
}

const TONE_BAR = { accent: "bg-accent", warning: "bg-warning", danger: "bg-danger" } as const;
const TONE_TEXT = { accent: "text-fg", warning: "text-warning", danger: "text-danger" } as const;

/** How full the context window is: the percentage up front, a bar that warms as it fills. */
function ContextMeter({ used, usage }: { used: number; usage: SessionUsage }) {
  const tone = contextTone(used);
  return (
    <div className="flex flex-col gap-1.5 px-2 pt-2">
      <div className="flex items-baseline gap-2">
        <span className="text-caption text-fg-muted">Context</span>
        <span className={cx("font-medium text-callout tabular", TONE_TEXT[tone])}>{used}%</span>
        <span className="flex-1" />
        <span className="text-caption text-fg-muted tabular">
          {formatTokens(usage.used)} / {formatTokens(usage.size)}
        </span>
      </div>
      <div
        role="meter"
        aria-label="Context used"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={used}
        className="h-2 overflow-hidden rounded-full bg-fill"
      >
        <div
          className={cx("h-full rounded-full transition-[width]", TONE_BAR[tone])}
          style={{ width: `${Math.max(used, 1)}%` }}
        />
      </div>
    </div>
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
  grow,
  children,
}: {
  id: SectionId;
  title: string;
  count?: number;
  /** Take the card's remaining height and scroll the rows inside it. */
  grow?: boolean;
  folded: boolean;
  onToggle: (id: SectionId) => void;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className={cx("px-2 pt-1.5", grow && !folded && "flex min-h-40 flex-1 flex-col")}>
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
      {!folded && <div className={cx("pb-1", grow && "flex min-h-0 flex-1 flex-col")}>{children}</div>}
    </section>
  );
}
