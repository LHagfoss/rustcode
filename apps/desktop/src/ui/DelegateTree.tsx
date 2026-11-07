// The threads a thread delegated to, drawn as a tree: each row is a mark of the
// harness, the thread's name, what it is doing, how long its branch worked and
// what it cost, with the threads that node started indented under it behind a
// tree line. A branch that finished folds to one line; opening a node folds the
// one that was open, so the panel never becomes a wall of rows. The title opens
// the thread, the reply button shows its last answer in place, and a Board card
// reads the same tree as one summary line (state/delegates.ts has the rules).

import { ChevronRight, ListTree, MessageSquareText } from "lucide-react";
import { useMemo, useState } from "react";
import { parseUsageReport } from "../acp/rustcode";
import { Method } from "../acp/types";
import type { ThreadStatus } from "../acp/v2";
import {
  AGENT_WORD,
  type AgentState,
  branchWord,
  costLabel,
  type DelegateRead,
  type DelegateView,
  delegateCount,
  delegateTree,
  delegateViews,
  forestTotal,
  timeLabel,
} from "../state/delegates";
import { type CostTotals, costOf } from "../state/pricing";
import { selectThread, useApp } from "../state/store";
import { stripHarnessMarkup } from "../state/thread";
import { AgentAvatar, AgentTip, useNow } from "./AgentViews";
import { Markdown } from "./Markdown";
import { cx, Tooltip } from "./primitives";
import { useEngineData } from "./settingsKit";

/** What each delegate thread's own turns cost, from the same read Statistics uses. */
export function useDelegateCosts(): ReadonlyMap<string, CostTotals> {
  const report = useEngineData(Method.dataUsage, parseUsageReport, { groupBy: "thread" });
  return useMemo(() => {
    const costs = new Map<string, CostTotals>();
    for (const row of report.data?.rows ?? []) {
      if (!row.thread) continue;
      costs.set(
        row.thread,
        costOf({
          harness: row.modelHarness ?? row.harness,
          // A thread is estimated only when all its turns ran one harness and model.
          model: row.modelHarness ? row.model : undefined,
          totals: row,
        }),
      );
    }
    return costs;
  }, [report.data]);
}

/** What the store says of one delegate right now. */
interface Reading {
  /** When the approval it waits on was asked. */
  askedAt?: number;
  running: boolean;
  /** The status the engine listed for it, which outlives a restart of this app. */
  listed?: ThreadStatus;
}

const ACTIVITY_SEP = "\u0001";
const FIELD_SEP = "\u0000";

/**
 * The tree of a thread's delegates, with the live readings its rows need. The
 * selector returns one string, so a streamed chunk somewhere else does not
 * redraw the tree: only a delegate starting, stopping or asking does.
 */
function useDelegateViews(threadId: string, costs: ReadonlyMap<string, CostTotals> | undefined) {
  const links = useApp((s) => s.links.links);
  const sessions = useApp((s) => s.sessions);
  const nodes = useMemo(() => delegateTree(threadId, links), [threadId, links]);
  const ids = useMemo(() => delegateIds(nodes), [nodes]);
  // The list's own answer, which changes rarely, stays out of the selector below.
  const listed = useMemo(() => new Map(sessions.map((session) => [session.id, session.status])), [sessions]);
  const activity = useApp((s) =>
    ids
      .map((id) => {
        const thread = s.threads[id];
        return [id, thread?.permissions[0]?.at ?? "", thread?.running ? 1 : 0].join(FIELD_SEP);
      })
      .join(ACTIVITY_SEP),
  );
  const readings = useMemo(() => {
    const map = new Map<string, Reading>();
    for (const entry of activity.split(ACTIVITY_SEP)) {
      if (!entry) continue;
      const [id, asked, running] = entry.split(FIELD_SEP);
      if (!id) continue;
      map.set(id, {
        askedAt: asked ? Number(asked) : undefined,
        running: running === "1",
        listed: listed.get(id),
      });
    }
    return map;
  }, [activity, listed]);
  // A row only ticks while a thread works: a waiting one holds still, and a
  // finished branch has nothing to count.
  const ticking = useMemo(
    () =>
      flattenNodes(nodes).some((node) => {
        const reading = readings.get(node.id);
        return (
          node.link.state === "working" ||
          node.link.state === "queued" ||
          !!reading?.running ||
          reading?.listed === "running"
        );
      }),
    [nodes, readings],
  );
  const now = useNow(ticking);
  const byId = useMemo(() => new Map(sessions.map((session) => [session.id, session])), [sessions]);
  const read: DelegateRead = {
    now,
    title: (id, link) => byId.get(id)?.title ?? link.targetTitle,
    harness: (id, link) => byId.get(id)?.harness ?? link.harness,
    activity: (id) => {
      const reading = readings.get(id);
      if (reading?.askedAt !== undefined) return "needs_input";
      if (reading?.running) return "running";
      return reading?.listed;
    },
    waitingSince: (id, link) => readings.get(id)?.askedAt ?? link.waitingSince,
    cost: (id) => costs?.get(id),
  };
  return delegateViews(nodes, read);
}

function delegateIds(nodes: ReturnType<typeof delegateTree>): string[] {
  return flattenNodes(nodes).map((node) => node.id);
}

function flattenNodes(nodes: ReturnType<typeof delegateTree>): ReturnType<typeof delegateTree> {
  return nodes.flatMap((node) => [node, ...flattenNodes(node.children)]);
}

const STATE_TINT: Partial<Record<AgentState, string>> = {
  working: "text-accent",
  needs: "text-warning",
  failed: "text-danger",
};

/** One node's line, and the rows of what it started when it is open, under a tree line. */
function DelegateRow({
  view,
  depth,
  ancestors,
  openPath,
  answerId,
  onOpen,
  onAnswer,
}: {
  view: DelegateView;
  depth: number;
  /** The nodes above it, so opening it keeps them open. */
  ancestors: string[];
  /** The one branch that is open, from its top down. */
  openPath: readonly string[];
  answerId: string | undefined;
  onOpen: (id: string, above: string[]) => void;
  onAnswer: (id: string) => void;
}) {
  const open = openPath.includes(view.id);
  const showAnswer = answerId === view.id;
  const hasChildren = view.children.length > 0;
  const answer = view.link.answer ? stripHarnessMarkup(view.link.answer).trim() : "";
  const time = timeLabel(view.total.ms);
  const cost = costLabel(view.total.usd, view.total.estimated);
  const summary = hasChildren ? `${view.total.threads} threads · ${branchWord(view.total)}` : undefined;
  return (
    <div className="relative">
      <div className="group relative flex min-w-0 items-center gap-2 rounded-md py-1 pr-0.5 pl-2 hover:bg-fill-subtle">
        {depth > 0 && <span aria-hidden className="absolute top-1/2 -left-2 h-px w-2 bg-border" />}
        <AgentAvatar harness={view.harness} state={view.state} />
        <Tooltip
          label={
            <AgentTip
              title={view.title}
              relationship={summary ?? "Thread it started"}
              state={view.state}
              elapsed={`${time} · ${cost}`}
              task={view.task}
            />
          }
        >
          <button
            type="button"
            onClick={() => selectThread(view.id)}
            aria-label={`Open ${view.title}`}
            className="min-w-0 flex-1 truncate text-left font-medium text-fg text-sub"
          >
            {view.title}
          </button>
        </Tooltip>
        <span className={cx("shrink-0 text-caption", STATE_TINT[view.state] ?? "text-fg-muted")}>
          {AGENT_WORD[view.state]}
        </span>
        <span className="w-12 shrink-0 text-right text-caption text-fg-faint tabular">{time}</span>
        <span className="w-14 shrink-0 text-right text-caption text-fg-faint tabular">{cost}</span>
        {answer && (
          <Tooltip label={showAnswer ? "Hide its last answer" : "Show its last answer"}>
            <button
              type="button"
              aria-label={showAnswer ? `Hide ${view.title}'s answer` : `Show ${view.title}'s answer`}
              aria-expanded={showAnswer}
              onClick={() => onAnswer(view.id)}
              className={cx(
                "flex size-[22px] shrink-0 items-center justify-center rounded-sm text-fg-faint hover:bg-fill hover:text-fg",
                !showAnswer && "opacity-0 group-focus-within:opacity-100 group-hover:opacity-100",
              )}
            >
              <MessageSquareText size={12} />
            </button>
          </Tooltip>
        )}
        {hasChildren && (
          <button
            type="button"
            aria-label={open ? `Fold ${view.title}` : `Unfold ${view.title}`}
            aria-expanded={open}
            onClick={() => onOpen(view.id, ancestors)}
            className="flex size-[22px] shrink-0 items-center justify-center rounded-sm text-fg-faint hover:bg-fill hover:text-fg"
          >
            <ChevronRight
              size={14}
              aria-hidden
              className={cx("transition-transform duration-200", open && "rotate-90")}
            />
          </button>
        )}
      </div>
      {showAnswer && answer && (
        <div className="rc-expand mb-1 ml-8 max-h-48 overflow-y-auto rounded-lg bg-fill-subtle p-2.5">
          <Markdown text={answer} className="text-callout" />
        </div>
      )}
      {open && hasChildren && (
        <div className="rc-expand ml-3 flex flex-col border-border border-l pl-2">
          {view.children.map((child) => (
            <DelegateRow
              key={child.id}
              view={child}
              depth={depth + 1}
              ancestors={[...ancestors, view.id]}
              openPath={openPath}
              answerId={answerId}
              onOpen={onOpen}
              onAnswer={onAnswer}
            />
          ))}
        </div>
      )}
    </div>
  );
}

/** Every delegate of a branch, indented, for a hover tip. */
function DelegateTip({ views }: { views: readonly DelegateView[] }) {
  const rows: { view: DelegateView; depth: number }[] = [];
  const walk = (list: readonly DelegateView[], depth: number) => {
    for (const view of list) {
      rows.push({ view, depth });
      walk(view.children, depth + 1);
    }
  };
  walk(views, 0);
  return (
    <div className="flex flex-col gap-0.5">
      {rows.map(({ view, depth }) => (
        <span key={view.id} className="flex min-w-0 items-center gap-1.5" style={{ paddingLeft: depth * 10 }}>
          <span className={cx("shrink-0", STATE_TINT[view.state] ?? "text-fg-muted")}>
            {AGENT_WORD[view.state]}
          </span>
          <span className="min-w-0 flex-1 truncate text-fg-muted">{view.title}</span>
          <span className="shrink-0 text-fg-faint tabular">{timeLabel(view.total.ms)}</span>
          <span className="shrink-0 text-fg-faint tabular">
            {costLabel(view.total.usd, view.total.estimated)}
          </span>
        </span>
      ))}
    </div>
  );
}

/**
 * A thread's delegates: the full tree in the right panel, or one sum line for a
 * Board card (`compact`). The costs are read once by the caller that draws many
 * of them, so a board of cards is one read, not one per card.
 */
export function DelegateTree({
  threadId,
  compact = false,
  costs,
}: {
  threadId: string;
  /** One line: how many delegates, what they do and what the branch cost. */
  compact?: boolean;
  costs?: ReadonlyMap<string, CostTotals>;
}) {
  const views = useDelegateViews(threadId, costs);
  // The branch the reader opened, as the path down to it: opening a node folds
  // what was open beside it, while the nodes above it stay open around it.
  const [openPath, setOpenPath] = useState<string[]>(() => {
    const live = views.find((view) => view.children.length && !view.total.finished);
    return live ? [live.id] : [];
  });
  const [answerId, setAnswerId] = useState<string | undefined>(undefined);

  if (compact) {
    if (!views.length) return null;
    const total = forestTotal(views);
    const word = branchWord(total);
    return (
      <Tooltip label={<DelegateTip views={views} />}>
        <span className="mt-1 flex min-w-0 items-center gap-1 text-caption text-fg-muted">
          <ListTree size={11} aria-hidden className="shrink-0 text-fg-faint" />
          <span className="shrink-0 tabular">
            {total.threads} {total.threads === 1 ? "delegate" : "delegates"}
          </span>
          <span className="min-w-0 truncate">· {word}</span>
          <span className="ml-auto shrink-0 text-fg-faint tabular">
            {costLabel(total.usd, total.estimated)}
          </span>
        </span>
      </Tooltip>
    );
  }

  if (!views.length) return null;
  return (
    <div role="group" aria-label="Delegated threads" className="flex flex-col gap-px py-0.5">
      {views.map((view) => (
        <DelegateRow
          key={view.id}
          view={view}
          depth={0}
          ancestors={[]}
          openPath={openPath}
          answerId={answerId}
          onOpen={(id, above) => setOpenPath((open) => (open.includes(id) ? above : [...above, id]))}
          onAnswer={(id) => setAnswerId((open) => (open === id ? undefined : id))}
        />
      ))}
    </div>
  );
}

/**
 * The details section's tree: the component that reads the costs, mounted only
 * when the thread has delegates, so a thread without any costs nothing to read.
 */
export function DelegatedThreads({ threadId }: { threadId: string }) {
  const costs = useDelegateCosts();
  return <DelegateTree key={threadId} threadId={threadId} costs={costs} />;
}

/** A thread's delegates, named for a `Started` heading: one count for the whole branch. */
export function delegateBranchCount(threadId: string, links: Parameters<typeof delegateTree>[1]): number {
  return delegateCount(delegateTree(threadId, links));
}
