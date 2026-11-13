// Threads working for threads, in the chat: agent rows above the composer for
// the threads this one started or messaged (what it is doing, open, stop), a
// "Working for" banner in a thread doing work for another, and the prompt that
// asks the user before a thread reaches another one.

import {
  CircleCheck,
  CircleX,
  Clock,
  CornerDownRight,
  GitBranch,
  Hand,
  Laptop,
  LoaderCircle,
  PauseCircle,
  Rows3,
} from "lucide-react";
import { Dialog } from "radix-ui";
import { useEffect, useMemo } from "react";
import { agentActive, agentElapsed, type SubagentInfo, spawnedAgentId } from "../acp/rustcode";
import { runningAgents, runningSubagents } from "../state/delegates";
import { linksOf, resolveGrant, stateOf, stopLink, verb } from "../state/host";
import { type Link, type LinkState, linkStatusText } from "../state/orchestration";
import { modeLabel } from "../state/permissions";
import { selectThread, setView, useApp } from "../state/store";
import { AgentRow, SubagentRow, useNow } from "./AgentViews";
import { cx, Pill, Tooltip } from "./primitives";

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
 * Above the composer: the thread this one works for, its latest fan-out, and the harness's own
 * subagents at work. The threads it delegated to are not drawn here: the status panel above this
 * (`DelegateStatus.tsx`) is their one place, folded to a line, so the same delegate is never a card
 * here and a row there.
 */
export function LinkedThreads({ threadId }: { threadId: string }) {
  const book = useApp((s) => s.links);
  const items = useApp((s) => s.threads[threadId]?.items);
  const { drivenBy } = linksOf(book, threadId);
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
  const fanOuts = book.fanOuts.filter((f) => f.orchestrator === threadId).slice(-1);
  if (!drivenBy && !fanOuts.length && !natives.length && !builtins.length) return null;
  return (
    // The one surface this strip shares with the jump-to-latest pill above it and the status panel
    // (`index.css` `.rc-glass-overlay`): the transcript keeps showing through, so this reads as an
    // overlay on the chat rather than a hole cut in it. Its rows are transparent over it.
    <div className="rc-glass-overlay relative flex flex-col gap-0.5 rounded-xl p-1 rc-in">
      {drivenBy && <WorkingFor link={drivenBy} />}
      {(fanOuts.length > 0 || natives.length > 0 || builtins.length > 0) && (
        // Capped and scrolled on its own: a long run of agents stops growing over the chat.
        <div className="flex max-h-[min(320px,45vh)] flex-col gap-0.5 overflow-y-auto">
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
