// Subagents, one way for every harness: a row per
// agent a turn started, where it started it, that says what it does while it
// works and how long it took once done; and a pill over the composer while
// agents work. RustCode's own agents (`spawn_agent`) take the same row, fed
// by `_rustcode/agents`.

import { Bot, ChevronRight, CircleCheck, CircleX, PanelRight } from "lucide-react";
import { HoverCard } from "radix-ui";
import { useEffect, useMemo, useRef, useState } from "react";
import { agentActive, agentElapsed, type SubagentInfo, spawnedAgentId } from "../acp/rustcode";
import { modelName } from "../acp/types";
import { stateOf } from "../state/host";
import { isActive, LINK_LABEL, type Link, type LinkState } from "../state/orchestration";
import { agentHistory, peekThread, selectThread, useApp } from "../state/store";
import { formatDuration, stepLabel, subagentOf, type TranscriptItem } from "../state/thread";
import { HarnessIcon } from "./HarnessIcon";
import { Markdown } from "./Markdown";
import { cx, Orb, Skeleton, Spinner, Tooltip } from "./primitives";

const EMPTY: SubagentInfo[] = [];
const NO_STEPS: ToolItem[] = [];

export function useNow(active: boolean) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!active) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [active]);
  return now;
}

type ToolItem = Extract<TranscriptItem, { kind: "tool" }>;

const live = (item: ToolItem) => item.status === "pending" || item.status === "in_progress";

/** A RustCode agent's steps, read from its own conversation while it works or is open. */
function useAgentSteps(threadId: string, agentId: number | undefined, active: boolean, open: boolean) {
  const [steps, setSteps] = useState<ToolItem[]>(NO_STEPS);
  // The first read of a folded row is in flight: the steps are not known to be empty.
  const [loading, setLoading] = useState(false);
  useEffect(() => {
    if (agentId === undefined || !(active || open)) return;
    let alive = true;
    setLoading(true);
    const load = () =>
      agentHistory(threadId, agentId).then(
        (h) => {
          if (!alive) return;
          setSteps(h.items.filter((i): i is ToolItem => i.kind === "tool"));
          setLoading(false);
        },
        () => alive && setLoading(false),
      );
    void load();
    const t = active ? setInterval(load, 2500) : undefined;
    return () => {
      alive = false;
      if (t) clearInterval(t);
    };
  }, [threadId, agentId, active, open]);
  return { steps, loading };
}

type AgentState = "working" | "needs" | "done" | "failed" | "stopped";

const AGENT_DOT: Record<AgentState, string> = {
  working: "bg-accent rc-breathe",
  needs: "bg-warning",
  done: "bg-success",
  failed: "bg-danger",
  stopped: "bg-fg-faint",
};

const LINK_AGENT_STATE: Record<LinkState, AgentState> = {
  queued: "working",
  working: "working",
  needs_user: "needs",
  done: "done",
  failed: "failed",
  cancelled: "stopped",
  interrupted: "stopped",
};

/** Who is doing the work, and a dot for how it goes. The dot pops in when the state changes. */
function AgentAvatar({ harness, state }: { harness?: string; state: AgentState }) {
  return (
    <span className="relative flex size-[18px] shrink-0 items-center justify-center rounded-full bg-fill">
      {harness ? <HarnessIcon id={harness} size={11} /> : <Bot size={11} className="text-fg-muted" />}
      <span
        key={state}
        aria-hidden
        className={cx(
          "-right-0.5 -bottom-0.5 absolute size-2 rounded-full ring-2 ring-bg",
          AGENT_DOT[state],
          state !== "working" && "rc-dot-in",
        )}
      />
    </span>
  );
}

/** `Claude Code · haiku`: what runs the agent, on the row itself. */
function useAgentMeta(harness: string | undefined, model: string | undefined): string {
  const name = useApp((s) =>
    harness ? (s.harnesses?.harnesses.find((h) => h.id === harness)?.name ?? harness) : undefined,
  );
  return [name, modelName(model)].filter(Boolean).join(" · ");
}

/**
 * A thread this one started (`start_session`), as a subagent: which harness and model it runs on,
 * how it goes, and for how long. Clicking opens it.
 */
export function ThreadAgentRow({ link, model }: { link: Link; model?: string }) {
  const state = useApp((s) => stateOf(s, link));
  const session = useApp((s) => s.sessions.find((x) => x.id === link.target));
  const active = isActive(state);
  const now = useNow(active);
  const meta = useAgentMeta(link.harness ?? session?.harness, session?.model ?? model);
  const end = active ? now : session?.updatedAt;
  const elapsed = end !== undefined && end - link.createdAt >= 1000 ? end - link.createdAt : undefined;
  const agent = LINK_AGENT_STATE[state];
  return (
    <div className="group flex items-center gap-0.5">
      <button
        type="button"
        aria-label={`Open ${link.targetTitle}`}
        onClick={() => selectThread(link.target)}
        className="group -mx-1 flex min-h-6 w-fit max-w-[calc(100%+8px)] items-center gap-2 rounded-sm px-1 text-left hover:bg-fill-subtle"
      >
        <AgentAvatar harness={link.harness ?? session?.harness} state={agent} />
        <span className="min-w-0 shrink truncate font-medium text-callout text-fg">
          {session?.title || link.targetTitle}
        </span>
        {meta && (
          <span className="min-w-0 max-w-[40%] shrink truncate text-caption text-fg-faint">{meta}</span>
        )}
        <span
          className={cx(
            "shrink-0 text-caption tabular",
            agent === "working"
              ? "text-accent"
              : agent === "failed"
                ? "text-danger"
                : agent === "needs"
                  ? "text-warning"
                  : agent === "done"
                    ? "text-success"
                    : "text-fg-faint",
          )}
        >
          {LINK_LABEL[state]}
          {elapsed !== undefined && (
            <span className="text-fg-faint"> · {formatDuration(Math.floor(elapsed / 1000) * 1000)}</span>
          )}
        </span>
        <ChevronRight size={11} className="shrink-0 text-fg-faint opacity-0 group-hover:opacity-100" />
      </button>
      <Tooltip label="Show beside this thread">
        <button
          type="button"
          aria-label={`Show ${link.targetTitle} beside this thread`}
          onClick={() => peekThread(link.target)}
          className="flex size-5 shrink-0 items-center justify-center rounded-sm text-fg-faint opacity-0 hover:bg-fill hover:text-fg focus-visible:opacity-100 group-hover:opacity-100"
        >
          <PanelRight size={11} />
        </button>
      </Tooltip>
    </div>
  );
}

/**
 * A subagent, drawn as one line: what it was asked, and while it works the
 * step it is on; once done, how many steps it took and how long. Its steps
 * stay folded; the chevron lists them. Claude Code's `Agent`/`Task` calls carry
 * their steps in the thread; a RustCode agent's come from its own conversation.
 */
export function SubagentRow({
  item,
  steps: inline,
  threadId,
}: {
  item: ToolItem;
  steps: readonly ToolItem[];
  threadId: string;
}) {
  const agent = subagentOf(item) ?? { name: "Agent", task: item.title };
  const agentId =
    item.title === "spawn_agent" && item.status !== "failed" ? spawnedAgentId(item.output) : undefined;
  const info = useApp((s) =>
    agentId === undefined ? undefined : (s.agents[threadId] ?? EMPTY).find((a) => a.id === agentId),
  );
  const running = info ? agentActive(info) : live(item);
  const failed = info ? info.status === "failed" : item.status === "failed";
  const [open, setOpen] = useState(false);
  const now = useNow(running);
  const fetched = useAgentSteps(threadId, agentId, running, open);
  const steps = agentId === undefined ? inline : fetched.steps;
  const loadingSteps = agentId !== undefined && fetched.loading && steps.length === 0;
  const current = running ? ([...steps].reverse().find(live) ?? steps.at(-1)) : undefined;
  const elapsedMs =
    info || item.startedAt === undefined || !(running || item.endedAt !== undefined)
      ? undefined
      : (running ? now : item.endedAt!) - item.startedAt;
  const elapsed = info
    ? agentElapsed(info, now)
    : elapsedMs !== undefined && elapsedMs >= 1000
      ? formatDuration(elapsedMs)
      : undefined;
  const output = info ? info.completion : item.title === "spawn_agent" ? undefined : item.output;
  const count = steps.length ? `${steps.length} step${steps.length === 1 ? "" : "s"}` : undefined;
  const done = [count, elapsed].filter(Boolean).join(" · ");
  const name = info?.agentType && info.agentType !== "default" ? info.agentType : agent.name;
  // A subagent runs on its parent's harness, and on its model unless the call chose another.
  const parentHarness = useApp((s) => s.threads[threadId]?.harness?.id);
  const parentModel = useApp((s) => s.sessions.find((x) => x.id === threadId)?.model);
  const asked =
    item.input && typeof item.input === "object" && !Array.isArray(item.input) ? item.input.model : undefined;
  const meta = useAgentMeta(
    parentHarness,
    info?.model ?? (typeof asked === "string" && asked ? asked : parentModel),
  );
  return (
    <div>
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className="group -mx-1 flex min-h-6 w-fit max-w-[calc(100%+8px)] items-center gap-2 rounded-sm px-1 text-left hover:bg-fill-subtle"
      >
        <span className="flex w-[18px] shrink-0 justify-center">
          <AgentAvatar harness={parentHarness} state={running ? "working" : failed ? "failed" : "done"} />
        </span>
        <span className="shrink-0 font-medium text-callout text-fg">{name}</span>
        <span className="min-w-0 shrink truncate text-callout text-fg-muted">{agent.task}</span>
        {meta && (
          <span className="min-w-0 max-w-[30%] shrink truncate text-caption text-fg-faint">{meta}</span>
        )}
        {running ? (
          <span className="min-w-0 max-w-[40%] shrink truncate text-caption text-accent">
            {current ? stepLabel(current) : "Starting"}
          </span>
        ) : (
          <span className={cx("shrink-0 text-caption tabular", failed ? "text-danger" : "text-success")}>
            {failed ? "Failed" : "Done"}
            {done && <span className="text-fg-faint"> · {done}</span>}
          </span>
        )}
        {(steps.length > 0 || output || agentId !== undefined) && (
          <ChevronRight
            size={11}
            className={cx(
              "shrink-0 text-fg-faint transition-[transform,opacity] duration-200",
              open ? "rotate-90" : "opacity-0 group-hover:opacity-100",
            )}
          />
        )}
      </button>
      {open && (
        <div className="rc-expand mt-1 mb-1 ml-[26px] flex flex-col gap-0.5 rounded-lg border border-border bg-fill-subtle/30 px-3 py-2">
          {loadingSteps && <StepsSkeleton />}
          {steps.map((step) => (
            <div key={step.id} className="flex h-5 min-w-0 items-center gap-1.5 text-caption text-fg-muted">
              {live(step) ? (
                <Spinner size={10} />
              ) : step.status === "failed" ? (
                <CircleX size={10} className="shrink-0 text-danger" />
              ) : (
                <span className="size-1 shrink-0 rounded-full bg-fg-faint" />
              )}
              <span className="min-w-0 truncate">{stepLabel(step)}</span>
            </div>
          ))}
          {output && !running && (
            <div className="mt-1.5 max-h-64 overflow-y-auto rounded-lg bg-fill-subtle p-2.5">
              <Markdown text={output} className="text-callout" />
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * Two or more subagents a turn started together:
 * one line, `3 subagents`, with how many still work (or how long they took)
 * and a chevron; open, a card with each agent's own row.
 */
export function SubagentGroupRow({
  items,
  stepsOf,
  threadId,
  open,
  onToggle,
}: {
  items: ToolItem[];
  stepsOf: (id: string) => readonly ToolItem[];
  threadId: string;
  open: boolean;
  onToggle: () => void;
}) {
  const agents = useApp((s) => s.agents[threadId] ?? EMPTY);
  const state = items.map((item) => {
    const agentId =
      item.title === "spawn_agent" && item.status !== "failed" ? spawnedAgentId(item.output) : undefined;
    const info = agentId === undefined ? undefined : agents.find((a) => a.id === agentId);
    return {
      running: info ? agentActive(info) : live(item),
      failed: info ? info.status === "failed" : item.status === "failed",
    };
  });
  const working = state.filter((x) => x.running).length;
  const failed = state.filter((x) => x.failed).length;
  const now = useNow(working > 0);
  const started = Math.min(...items.map((i) => i.startedAt ?? Number.POSITIVE_INFINITY));
  const ended = Math.max(...items.map((i) => i.endedAt ?? 0));
  const elapsedMs = Number.isFinite(started) ? (working ? now : ended || now) - started : undefined;
  const status = working ? `${working} working` : failed ? `${failed} failed` : "done";
  return (
    <div className={cx(!working && !open && "opacity-80")}>
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        className="group -mx-1 flex min-h-6 w-fit max-w-[calc(100%+8px)] items-center gap-2 rounded-sm px-1 text-left hover:bg-fill-subtle"
      >
        <span className="flex w-[18px] shrink-0 justify-center">
          {working ? (
            <Orb />
          ) : failed ? (
            <CircleX size={13} className="text-danger" />
          ) : (
            <CircleCheck size={13} className="text-success" />
          )}
        </span>
        <span className="shrink-0 font-medium text-callout text-fg">{items.length} subagents</span>
        <span
          className={cx(
            "shrink-0 text-caption tabular",
            working ? "text-accent" : failed ? "text-danger" : "text-fg-faint",
          )}
        >
          {status}
          {elapsedMs !== undefined &&
            elapsedMs >= 1000 &&
            ` · ${formatDuration(Math.floor(elapsedMs / 1000) * 1000)}`}
        </span>
        <ChevronRight
          size={11}
          className={cx("shrink-0 text-fg-faint transition-transform duration-200", open && "rotate-90")}
        />
      </button>
      {open && (
        <div className="rc-expand mt-1 mb-1 ml-[26px] flex flex-col gap-1 rounded-lg border border-border bg-fill-subtle/30 px-2.5 py-1.5">
          {items.map((item) => (
            <SubagentRow key={item.id} item={item} steps={stepsOf(item.id)} threadId={threadId} />
          ))}
        </div>
      )}
    </div>
  );
}

const FINISHED_PILL_MS = 5000;

/** What one working agent is doing, for the pill's card. */
interface Working {
  key: string;
  name: string;
  task: string;
  step?: string;
  startedAt?: number;
}

/**
 * Over the composer while agents work: a ring and `2 agents working`; hover
 * lists each with the step it is on.
 */
export function AgentPill({ threadId }: { threadId?: string }) {
  const agents = useApp((s) => (threadId ? (s.agents[threadId] ?? EMPTY) : EMPTY));
  // Read here, not from the composer's prop: the composer skips item changes.
  const thread = useApp((s) => (threadId ? s.threads[threadId] : undefined));
  const working = useMemo<Working[]>(() => {
    const out: Working[] = agents.filter(agentActive).map((a) => ({
      key: `rc-${a.id}`,
      name: a.name,
      task: a.task.split("\n")[0] ?? "",
      startedAt: a.startedAt,
    }));
    if (!thread?.running) return out;
    const items = thread.items;
    for (const item of items) {
      if (item.kind !== "tool" || !live(item)) continue;
      const agent = subagentOf(item);
      if (!agent) continue;
      let step: ToolItem | undefined;
      for (let i = items.length - 1; i >= 0; i--) {
        const s = items[i]!;
        if (s.kind === "tool" && s.parent === item.id) {
          step = s;
          break;
        }
      }
      out.push({
        key: item.id,
        name: agent.name,
        task: agent.task,
        step: step && stepLabel(step),
        startedAt: item.startedAt,
      });
    }
    return out;
  }, [agents, thread?.items, thread?.running]);
  const now = useNow(working.length > 0);
  // Agents leaving the list finished: say so for a moment instead of the pill just going away.
  const [finished, setFinished] = useState(0);
  const before = useRef({ threadId, count: working.length });
  const stopped = thread?.items.findLast((i) => i.kind === "turnEnd");
  const cancelled = stopped?.kind === "turnEnd" && stopped.outcome === "cancelled";
  useEffect(() => {
    const was = before.current;
    before.current = { threadId, count: working.length };
    if (was.threadId !== threadId) return setFinished(0);
    if (working.length > 0) return setFinished(0);
    if (was.count === 0 || cancelled) return;
    setFinished(was.count);
    const t = setTimeout(() => setFinished(0), FINISHED_PILL_MS);
    return () => clearTimeout(t);
  }, [threadId, working.length, cancelled]);
  if (!working.length) {
    if (!finished) return null;
    return (
      <span
        role="status"
        className="flex h-[26px] items-center gap-1.5 rounded-full bg-success/10 px-2.5 text-callout text-success rc-in"
      >
        <CircleCheck size={13} className="rc-dot-in" />
        {finished === 1 ? "Agent finished" : `${finished} agents finished`}
      </span>
    );
  }
  return (
    <HoverCard.Root openDelay={80} closeDelay={120}>
      <HoverCard.Trigger asChild>
        <button
          type="button"
          className="flex h-[26px] cursor-default items-center gap-2 rounded-full bg-accent/[0.08] px-2.5 text-callout text-accent rc-in"
        >
          <Orb />
          {working.length === 1 ? "1 agent working" : `${working.length} agents working`}
        </button>
      </HoverCard.Trigger>
      <HoverCard.Portal>
        <HoverCard.Content
          side="top"
          align="end"
          sideOffset={8}
          collisionPadding={10}
          className="elevated z-40 flex w-[300px] flex-col gap-0.5 p-1.5 rc-in"
        >
          {working.map((w) => (
            <div key={w.key} className="flex items-start gap-2 rounded-lg px-3 py-[7px]">
              <span className="mt-[5px] flex w-3.5 justify-center">
                <Orb />
              </span>
              <span className="min-w-0 flex-1">
                <span className="flex items-baseline gap-1.5">
                  <span className="shrink-0 font-medium text-callout">{w.name}</span>
                  <span className="min-w-0 flex-1 truncate text-callout text-fg-muted">{w.task}</span>
                  {w.startedAt !== undefined && now - w.startedAt >= 1000 && (
                    <span className="shrink-0 text-caption text-fg-faint tabular-nums">
                      {formatDuration(Math.floor((now - w.startedAt) / 1000) * 1000)}
                    </span>
                  )}
                </span>
                {w.step && <span className="block truncate text-caption text-fg-faint">{w.step}</span>}
              </span>
            </div>
          ))}
        </HoverCard.Content>
      </HoverCard.Portal>
    </HoverCard.Root>
  );
}

/** Where an agent's steps will be: rows the height of a step, its dot and label. */
function StepsSkeleton() {
  return (
    <div role="status" aria-label="Loading" className="flex flex-col gap-0.5">
      {[52, 68, 40].map((w, i) => (
        <div key={i} className="flex h-5 items-center gap-1.5">
          <Skeleton className="size-1 shrink-0 rounded-full" />
          <Skeleton className="h-2.5" style={{ width: `${w}%` }} />
        </div>
      ))}
    </div>
  );
}
