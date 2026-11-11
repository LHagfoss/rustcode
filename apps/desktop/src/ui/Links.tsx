// Threads working for threads, in the chat: agent rows above the composer for
// the threads this one started or messaged (what it is doing, open, stop), a
// "Working for" banner in a thread doing work for another, and the prompt that
// asks the user before a thread reaches another one.

import {
  ChevronDown,
  CircleCheck,
  CircleX,
  Clock,
  CornerDownRight,
  GitBranch,
  Hand,
  Laptop,
  ListTree,
  LoaderCircle,
  PauseCircle,
  Rows3,
  X,
} from "lucide-react";
import { Dialog, Popover } from "radix-ui";
import { useEffect, useMemo, useSyncExternalStore } from "react";
import { agentActive, agentElapsed, type SubagentInfo, spawnedAgentId } from "../acp/rustcode";
import type { StackAgent, StackState } from "../state/agentStack";
import {
  foldStrip,
  type StripCounts,
  stripCollapsed,
  stripCount,
  stripCounts,
  stripDoing,
  stripFolded,
  stripSummary,
  subscribeStripFold,
} from "../state/delegateStrip";
import {
  type AgentState,
  addBranch,
  type BranchTotal,
  branchBar,
  type DelegateView,
  flattenViews,
  forestTotal,
  ownBranch,
  runningAgents,
  runningSubagents,
} from "../state/delegates";
import { dismissLinkThread, linksOf, resolveGrant, stateOf, stopLink, verb } from "../state/host";
import {
  doneLinks,
  isActive,
  type Link,
  type LinkState,
  type LinkThread,
  linkStatusText,
  linkThreads,
  stripLinks,
} from "../state/orchestration";
import { modeLabel } from "../state/permissions";
import { emptyCost } from "../state/pricing";
import { selectThread, setView, showPanel, useApp } from "../state/store";
import { startedLinks } from "../state/thread";
import {
  AgentGroupHeader,
  AgentRow,
  LINK_AGENT_STATE,
  SubagentRow,
  ThreadAgentRow,
  useNow,
} from "./AgentViews";
import { useDelegateBranch } from "./DelegateTree";
import { unfoldSection } from "./overview";
import { cx, Pill, Spinner, Tooltip } from "./primitives";

const NO_AGENTS: SubagentInfo[] = [];

const ICON: Record<LinkState, typeof Clock> = {
  queued: Clock,
  working: LoaderCircle,
  needs_user: Hand,
  done: CircleCheck,
  failed: CircleX,
  cancelled: PauseCircle,
  interrupted: PauseCircle,
};

const TINT: Partial<Record<LinkState, string>> = {
  needs_user: "text-warning",
  done: "text-success",
  failed: "text-danger",
  working: "text-accent",
};

/** A link's state as a small icon: the chips, the popover and a tool row all read it the same way. */
export function LinkStateIcon({ state, size = 12 }: { state: LinkState; size?: number }) {
  const Icon = ICON[state];
  return (
    <Icon size={size} className={cx(TINT[state] ?? "text-fg-muted", state === "working" && "animate-spin")} />
  );
}

/**
 * One thread this one delegated to, in the strip above the composer and in its
 * popover: the same row as a subagent, plus stop (or remove once settled). The
 * popover's rows are one line of columns, so a list of them reads as a table.
 */
function LinkRow({ row, popover = false }: { row: LinkThread; popover?: boolean }) {
  const active = isActive(row.state);
  return (
    <ThreadAgentRow
      link={row.link}
      messages={row.links.length}
      peek={false}
      popover={popover}
      action={
        <Tooltip label={active ? "Stop this work in that thread" : "Remove"}>
          <button
            type="button"
            aria-label={active ? `Stop ${row.link.targetTitle}` : `Remove ${row.link.targetTitle}`}
            onClick={() =>
              active ? void stopLink(row.link.id) : dismissLinkThread(row.link.orchestrator, row.link.target)
            }
            className="flex size-[22px] shrink-0 items-center justify-center rounded-sm text-fg-faint hover:bg-fill hover:text-fg active:bg-fill-strong"
          >
            <X size={12} strokeWidth={2.5} />
          </button>
        </Tooltip>
      }
    />
  );
}

/** When a link's turn began, for the strip's live clock: its start, else when it was made. */
const startedOf = (link: Link) => link.startedAt ?? link.createdAt;

/** What the strip reads of the branch once per render: its header, its marks and its clock. */
interface Strip {
  /** The threads this one started, and the threads those started, at every depth. */
  views: DelegateView[];
  /** The threads the tree holds: a strip row for one of them is already counted there. */
  inTree: ReadonlySet<string>;
  /** The strip's rows, one per thread however many messages it got. */
  rows: readonly LinkThread[];
  /** The branch's figures, the strip's own rows added to the tree. */
  total: BranchTotal;
  /** When each thread working now started its turn, for the header's clock. */
  runningSince: number[];
}

/**
 * What the strip reads of the branch: the details tree's figures, plus a row for
 * each thread it only messaged (which has no node of its own), and when each
 * thread working now started its turn.
 */
function readStrip(views: DelegateView[], inTree: ReadonlySet<string>, rows: readonly LinkThread[]): Strip {
  const at = Date.now();
  const total = rows.reduce((sum, row) => {
    // A thread the tree already holds, at any depth, is counted there.
    if (inTree.has(row.link.target)) return sum;
    // One it only messaged: not a delegate, but still a row of this strip.
    const state = LINK_AGENT_STATE[row.state];
    const end = row.link.settledAt ?? at;
    const elapsed = {
      ms: Math.max(0, end - row.link.createdAt),
      live: row.link.settledAt === undefined,
      frozen: false,
    };
    return addBranch(sum, ownBranch(state, elapsed, emptyCost()));
  }, forestTotal(views));
  const runningSince = [
    ...flattenViews(views).flatMap((view) => (view.state === "working" ? [startedOf(view.link)] : [])),
    ...rows.flatMap((row) =>
      !inTree.has(row.link.target) && LINK_AGENT_STATE[row.state] === "working" ? [startedOf(row.link)] : [],
    ),
  ];
  return { views, inTree, rows, total, runningSince };
}

/**
 * Three or more linked threads in the strip fold into one card header, the same
 * header a group of agents has in the transcript: the strip above the composer
 * has about 800px, and a third of it spent on a made-up title tells the reader
 * nothing. Two stay as rows of their own; they fit. The header is also the strip's
 * folded form — its own count and live summary — so a strip of this many rows has
 * one header with one chevron, which opens the popover.
 */
function ManyLinks({
  rows,
  threadId,
  strip,
  counts,
}: {
  rows: readonly LinkThread[];
  threadId: string;
  strip: Strip;
  /** What the strip's rows add up to, the words this header says after the avatars. */
  counts: StripCounts;
}) {
  const { views, inTree, total, runningSince } = strip;
  // A live clock, not the branch's summed work time: how long the longest thread working now has
  // been at it. Nothing runs once the branch finished, so nothing ticks there.
  const now = useNow(total.counts.working > 0);
  const bar = branchBar(total, runningSince, now);
  // The marks are who is live now, working or waiting: the finished ones are the popover's "Done",
  // not a wall of avatars.
  const mark = (view: DelegateView): StackAgent => ({
    id: view.id,
    state: view.state === "stopped" ? "done" : view.state,
    kind: "thread",
    harness: view.harness,
    label: view.title,
    since: view.link.createdAt,
  });
  const isLive = (state: AgentState) => state === "working" || state === "needs";
  const marks: StackAgent[] = [
    ...flattenViews(views).flatMap((view) => (isLive(view.state) ? [mark(view)] : [])),
    ...rows.flatMap((row): StackAgent[] => {
      if (inTree.has(row.link.target)) return [];
      const state = LINK_AGENT_STATE[row.state];
      if (!isLive(state)) return [];
      return [
        {
          id: row.link.target,
          state: state as StackState,
          kind: "thread" as const,
          harness: row.link.harness,
          label: row.link.targetTitle,
          since: row.link.createdAt,
        },
      ];
    }),
  ];
  // What settled after the live rows: one row per thread, newest first, capped, with a count for the
  // rest. A thread the strip still shows above is not listed twice.
  const book = useApp((s) => s.links.links);
  const settled = doneLinks(book, threadId);
  const above = new Set(rows.map((row) => row.target));
  const done = settled.shown.filter((link) => !above.has(link.target));
  const doneRows: LinkThread[] = done.map((link) => ({
    target: link.target,
    link,
    state: link.state,
    links: [link],
  }));
  const more = settled.more + (settled.shown.length - done.length);
  // Nothing runs and nothing waits: a settled branch is history the details panel keeps, not a
  // header over the composer.
  if (!bar.show) return null;
  // The threads the tree holds below the ones the strip draws: the header cannot list them, so it
  // says how many there are and opens the tree that does (the overview's Delegates section).
  const nested = Math.max(0, flattenViews(views).length - views.length);
  const openTree = () => {
    showPanel("details");
    unfoldSection("delegates");
  };
  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <button
          type="button"
          className="group flex w-full min-w-0 items-center gap-2 rounded-lg px-2 py-1.5 text-left hover:bg-fill-subtle data-[state=open]:bg-fill-subtle"
        >
          <AgentGroupHeader
            agents={marks.length ? marks : undefined}
            state={bar.state}
            label={stripCount(counts)}
            // What the branch does now, nested delegates included: the avatars carry their marks, so
            // the words beside them must count them too.
            status={stripDoing({ delegates: counts.delegates, running: bar.working, needs: bar.needs })}
            elapsedMs={bar.longestMs}
            open={false}
            chevron="down"
          />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          side="top"
          align="start"
          sideOffset={8}
          aria-label="Linked threads"
          className="elevated z-50 flex max-h-[min(420px,60vh)] w-[min(420px,calc(100vw-48px))] flex-col overflow-y-auto p-2 rc-in"
        >
          {rows.map((row) => (
            <LinkRow key={row.target} row={row} popover />
          ))}
          {doneRows.length > 0 && (
            <>
              <div className="px-3 pt-2 pb-1 text-caption text-fg-faint">Done</div>
              {doneRows.map((row) => (
                <LinkRow key={row.target} row={row} popover />
              ))}
              {more > 0 && <div className="px-3 py-1 text-caption text-fg-faint">{more} more</div>}
            </>
          )}
          {nested > 0 && (
            <Popover.Close asChild>
              <button
                type="button"
                onClick={openTree}
                className="mt-1 flex h-7 w-full shrink-0 items-center gap-1.5 rounded-lg px-3 text-left text-caption text-fg-muted hover:bg-fill-subtle active:bg-fill-strong"
              >
                <ListTree size={11} aria-hidden className="shrink-0 text-fg-faint" />+{nested} nested
              </button>
            </Popover.Close>
          )}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

/** Above the composer: the delegates this one is working with, and the thread it works for. */
export function LinkedThreads({ threadId }: { threadId: string }) {
  const book = useApp((s) => s.links);
  const items = useApp((s) => s.threads[threadId]?.items);
  const { driving, drivenBy } = linksOf(book, threadId);
  const agents = useApp((s) => s.agents[threadId] ?? NO_AGENTS);
  // A harness's own subagents, from another harness or an older session, read
  // in this same strip: one surface for everything working under the thread.
  const natives = useMemo(() => runningSubagents(items ?? []), [items]);
  // RustCode's own subagents: a live `spawn_agent` call already draws its
  // agent (`SubagentRow` reads `_rustcode/agents` for it), so only those whose
  // call returned at launch are left to add here.
  const builtins = useMemo(() => {
    const drawn = new Set(
      natives.flatMap(({ item }) => {
        const id = item.title === "spawn_agent" ? spawnedAgentId(item.output) : undefined;
        return id === undefined ? [] : [id];
      }),
    );
    return runningAgents(agents).filter((agent) => !drawn.has(agent.id));
  }, [agents, natives]);
  // A thread this one started reads in the transcript once it settles; the strip is its live
  // place until then, so the two never draw the same thread.
  const shown = useMemo(() => {
    const mine = book.links.filter((l) => l.orchestrator === threadId);
    return new Set([...startedLinks(items ?? [], mine).values()].map((l) => l.target));
  }, [items, book.links, threadId]);
  // What each of the thread's links shows now, one string so the selector's result is stable
  // between renders: a link a restart severed reads live again while its thread works.
  const drivingStates = useApp((s) => driving.map((l) => stateOf(s, l)).join(",")).split(",") as LinkState[];
  // A link that reads live keeps its strip row even when the transcript has a row for its thread.
  const live = stripLinks(driving, shown, drivingStates);
  // `live` is `driving` minus the rows it drops: the two line up by the link they came from.
  const kept = new Set(live);
  const states = drivingStates.filter((_, i) => kept.has(driving[i]!));
  // One row per delegate thread: a follow-up message is the same thread, not another row.
  const rows = linkThreads(live, states);
  // What the details panel draws as a tree is the authority for the branch: the threads this one
  // started, and the threads those started, at every depth. The strip's own rows only add the
  // threads it messaged, which have a row to stop or dismiss but no node of their own.
  const views = useDelegateBranch(threadId);
  const inTree = useMemo(() => new Set(flattenViews(views).map((view) => view.id)), [views]);
  const strip = readStrip(views, inTree, rows);
  const fanOuts = book.fanOuts.filter((f) => f.orchestrator === threadId).slice(-1);
  // Work under way or a wait on the user, with a row of its own to draw: anything settled is the
  // details panel's row (and a failure the inbox's), so the strip stays out of the chat's way
  // rather than counting a lifetime or pinning an old failure. A native agent at work is a row
  // of its own too.
  // What the strip's rows count as, for its one line: the delegates it draws, and how many of them
  // work or wait. A harness's agent and RustCode's are both listed only while at work, so they add
  // to `running` without a state of their own.
  const counts = stripCounts(
    rows.map((row) => LINK_AGENT_STATE[row.state]),
    natives.length + builtins.length,
  );
  // Folded for the session, per thread: the rows a reader put away stay away while they move
  // between threads, and come back when the app opens again. A fold only holds while the header is
  // there to unfold it: a strip of one row draws that row, folded thread or not, and a group card is
  // its own header (and already one line), so it has nothing to fold.
  const group = rows.length > 2;
  const folded = useSyncExternalStore(subscribeStripFold, () => stripFolded(threadId));
  const collapsed = !group && stripCollapsed(counts, folded);
  const { working, needs } = strip.total.counts;
  const news = rows.length > 0 && working + needs > 0;
  if (!news && !drivenBy && !fanOuts.length && !natives.length && !builtins.length) return null;
  const rowsId = `delegate-strip-${threadId}`;
  return (
    // The one surface this strip shares with the jump-to-latest pill above it and its own group card
    // (`index.css` `.rc-glass-overlay`): the transcript keeps showing through, so a run of threads
    // reads as an overlay on the chat rather than a hole cut in it. Its rows are transparent over it.
    <div className="rc-glass-overlay relative flex flex-col gap-0.5 rounded-xl p-1 rc-in">
      {drivenBy && <WorkingFor link={drivenBy} />}
      {/* The strip folds to this one line: what it holds and what that work does now. Two rows get
          it, because one row is already one line; folding leaves the header alone, and the
          transcript takes back the room the rows had (`ChatView.tsx`, the measured stack height). A
          group card draws that line itself, so it has no fold row of its own. */}
      {!group && counts.delegates > 1 && (
        <Tooltip label={folded ? "Show the delegates" : "Fold the delegates into one line"}>
          <button
            type="button"
            aria-expanded={!collapsed}
            // Only when there is a list to point at: a folded strip draws no rows.
            aria-controls={collapsed ? undefined : rowsId}
            onClick={() => foldStrip(threadId, !folded)}
            className="flex h-7 w-full min-w-0 items-center gap-1.5 rounded-lg px-2 text-left text-caption text-fg-muted hover:bg-fill-subtle active:bg-fill-strong"
          >
            <ChevronDown
              size={12}
              aria-hidden
              className={cx(
                "shrink-0 text-fg-faint transition-transform duration-200",
                folded && "-rotate-90",
              )}
            />
            <span className="min-w-0 truncate">{stripSummary(counts)}</span>
            {counts.running > 0 && <Spinner size={9} className="text-accent" />}
          </button>
        </Tooltip>
      )}
      {!collapsed && (rows.length > 0 || fanOuts.length > 0 || natives.length > 0 || builtins.length > 0) && (
        // Capped and scrolled on its own: a long run of delegates stops growing over the chat and
        // the strip keeps one height, whatever the branch does.
        <div id={rowsId} className="flex max-h-[min(320px,45vh)] flex-col gap-0.5 overflow-y-auto">
          {group ? (
            <ManyLinks rows={rows} threadId={threadId} strip={strip} counts={counts} />
          ) : (
            rows.map((row) => <LinkRow key={row.target} row={row} />)
          )}
          {fanOuts.length > 0 && (
            <div className="flex flex-wrap gap-1.5 px-2 pt-0.5">
              {fanOuts.map((f) => (
                <Pill
                  key={f.id}
                  onClick={() => setView({ kind: "compare", fanOut: f.id })}
                  className="shrink-0"
                >
                  <Rows3 size={12} /> Compare
                </Pill>
              ))}
            </div>
          )}
          {natives.map(({ item, steps }) => (
            <SubagentRow key={item.id} item={item} steps={steps} threadId={threadId} />
          ))}
          {builtins.map((agent) => (
            <BuiltinAgentRow key={agent.id} agent={agent} threadId={threadId} />
          ))}
        </div>
      )}
    </div>
  );
}

/** One of RustCode's own subagents at work, as the same row every agent gets. */
function BuiltinAgentRow({ agent, threadId }: { agent: SubagentInfo; threadId: string }) {
  const harness = useApp((s) => s.threads[threadId]?.harness?.id);
  const now = useNow(agentActive(agent));
  return (
    <AgentRow
      harness={harness}
      state="working"
      title={agent.name}
      detail={agent.task.split("\n")[0]}
      elapsed={agentElapsed(agent, now)}
      label={`${agent.name}: ${agent.task}`}
      showWord={false}
    />
  );
}

function WorkingFor({ link }: { link: Link }) {
  const state = useApp((s) => stateOf(s, link));
  return (
    // A chip on the strip's surface, not a second glass on top of it.
    <div className="flex h-[32px] items-center gap-2 rounded-full bg-fill-subtle pr-1.5 pl-3 text-callout rc-in">
      <CornerDownRight size={13} className="shrink-0 text-accent" />
      <span className="min-w-0 truncate">Working for “{link.orchestratorTitle}”</span>
      <span className="shrink-0 text-fg-muted">· {linkStatusText(link, state)}</span>
      <span className="flex-1" />
      <Pill onClick={() => selectThread(link.orchestrator)}>Open</Pill>
      <Tooltip label="Stop the work that thread gave this one; your own messages are not affected">
        <Pill onClick={() => void stopLink(link.id)}>Stop</Pill>
      </Tooltip>
    </div>
  );
}

/** A thread asking whether it may reach another one: Allow for This Thread, Allow Once, Don't Allow. */
export function GrantDialog() {
  const grant = useApp((s) => s.grant);
  useEffect(() => {
    if (!grant) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") resolveGrant("deny");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [grant]);
  if (!grant) return null;
  const starts = grant.kind === "start" || grant.kind === "fan_out";
  return (
    <Dialog.Root open onOpenChange={(open) => !open && resolveGrant("deny")}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <Dialog.Content
          aria-describedby={undefined}
          className="elevated fixed top-1/2 left-1/2 z-50 flex w-[min(460px,calc(100vw-48px))] -translate-x-1/2 -translate-y-1/2 flex-col gap-3 p-5"
        >
          <Dialog.Title className="font-semibold text-body">
            “{grant.callerTitle}” wants to {verb(grant)}
          </Dialog.Title>
          <div className="flex flex-col gap-1.5 text-callout text-fg-muted">
            {starts && (
              <span>
                {/* The card says which of the two a thread will be: the agent's
                    sub-task, or a thread of the user's own at the top level. */}
                <span className="font-medium text-fg">{grant.standalone ? "New thread" : "Delegate"}</span>
                {grant.standalone ? ", yours to follow up: " : ": "}a new thread
                {grant.harnesses?.length ? ` on ${grant.harnesses.join(", ")}` : ""}
                {grant.worktree ? ", each in its own worktree," : ""} gets this as its first message.{" "}
                {grant.standalone
                  ? `It runs at ${modeLabel(grant.access ?? "ask")}, with its own approvals and sandbox, and nothing comes back to this thread.`
                  : `It runs at ${modeLabel(grant.access ?? "ask")}, never more than this thread, with its own approvals and sandbox.`}
              </span>
            )}
            {grant.kind === "message" && (
              <span>The message arrives in that thread as yours would, headed with where it is from.</span>
            )}
            {grant.machine && (
              <span className="flex items-center gap-1.5 text-warning">
                <Laptop size={12} /> On {grant.machine}, not this computer
              </span>
            )}
            {grant.project && (
              <span className="flex items-center gap-1.5 text-warning">
                <GitBranch size={12} /> In another project: {grant.project}
              </span>
            )}
          </div>
          <pre className="selectable m-0 max-h-[180px] overflow-auto rounded-lg bg-fill-subtle p-3 font-sans text-callout whitespace-pre-wrap">
            {grant.detail}
          </pre>
          <div className="flex flex-wrap justify-end gap-2">
            <Pill onClick={() => resolveGrant("deny")}>Don’t Allow</Pill>
            <Pill onClick={() => resolveGrant("once")}>Allow Once</Pill>
            <Pill prominent autoFocus onClick={() => resolveGrant("always")}>
              Allow for This Thread
            </Pill>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * A sidebar badge: this thread works for another (↳). It goes in front of the row's cluster of
 * delegate marks, so both of the row's lines end on the same edge.
 *
 * The count of threads it messaged that still run is gone from the row: the threads it started are
 * the marks beside this, and a messaged thread it waits on is what makes the row's chip say
 * `Waiting` (`state/waiting.ts` counts those links too). Two counts on one row said the same thing
 * twice, and neither was readable at 10px.
 */
export function LinkBadge({ threadId }: { threadId: string }) {
  const book = useApp((s) => s.links);
  const { drivenBy } = linksOf(book, threadId);
  if (!drivenBy) return null;
  return (
    <Tooltip label={`Working for “${drivenBy.orchestratorTitle}”`}>
      <CornerDownRight size={11} className="shrink-0 text-accent" aria-label="Working for another thread" />
    </Tooltip>
  );
}
