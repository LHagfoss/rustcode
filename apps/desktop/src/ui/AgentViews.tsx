// Subagents, one way for every harness: a row per agent a turn started (the
// same row the threads this one delegated to get), that says what it is doing
// while it works and what it answered once done, and how long it took; and a
// pill over the composer while agents work. RustCode's own agents
// (`spawn_agent`) take the same row, fed by `_rustcode/agents`.

import { Bot, Check, ChevronRight, CircleCheck, CircleX, Minus, PanelRight, X } from "lucide-react";
import { HoverCard } from "radix-ui";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { agentActive, agentElapsed, type SubagentInfo, spawnedAgentId } from "../acp/rustcode";
import { modelName } from "../acp/types";
import { stateOf } from "../state/host";
import { isActive, type Link, type LinkState } from "../state/orchestration";
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

export type AgentState = "working" | "needs" | "done" | "failed" | "stopped";

export const LINK_AGENT_STATE: Record<LinkState, AgentState> = {
  queued: "working",
  working: "working",
  needs_user: "needs",
  done: "done",
  failed: "failed",
  cancelled: "stopped",
  interrupted: "stopped",
};

const AGENT_LABEL: Record<AgentState, string> = {
  working: "Working",
  needs: "Needs you",
  done: "Done",
  failed: "Failed",
  stopped: "Stopped",
};

/** Who is doing the work, and a badge for how it goes. The badge pops in when the state changes. */
function AgentAvatar({
  harness,
  state,
  className,
}: {
  harness?: string;
  state: AgentState;
  className?: string;
}) {
  return (
    <span
      aria-hidden
      className={cx(
        "relative flex size-6 shrink-0 items-center justify-center rounded-full bg-fill",
        className,
      )}
    >
      {harness ? <HarnessIcon id={harness} size={13} /> : <Bot size={13} className="text-fg-muted" />}
      {state === "needs" ? (
        <span
          key={state}
          className="-right-0.5 -bottom-0.5 absolute size-2 rounded-full bg-warning ring-2 ring-bg rc-dot-in"
        />
      ) : (
        <span
          key={state}
          className="-right-1 -bottom-1 absolute flex size-3 items-center justify-center rounded-full bg-bg ring-2 ring-bg"
        >
          {state === "working" ? (
            <Spinner size={8} className="text-accent" />
          ) : state === "failed" ? (
            <X size={9} strokeWidth={3} className="text-danger" />
          ) : state === "stopped" ? (
            <Minus size={9} strokeWidth={3} className="text-fg-faint" />
          ) : (
            <Check size={9} strokeWidth={3} className="text-success rc-dot-in" />
          )}
        </span>
      )}
    </span>
  );
}

/** The first line of a result, without the markdown around it. */
function firstLine(text: string | null | undefined): string | undefined {
  const line = text
    ?.split("\n")
    .map((l) => l.trim())
    .find((l) => l.length > 0);
  if (!line) return undefined;
  const plain = line
    .replace(/^[>\-*+\s]+/, "")
    .replace(/[*`]/g, "")
    .trim();
  return plain || undefined;
}

/** What a hover on an agent row says: what it was asked, in full, and what runs it. */
export function AgentTip({ title, meta, task }: { title: string; meta?: string; task?: string }) {
  const asked = task?.trim();
  return (
    <div className="flex flex-col gap-0.5">
      <span className="font-medium">{title}</span>
      {meta && <span className="text-fg-muted">{meta}</span>}
      {asked && asked !== title.trim() && (
        <span className="mt-0.5 line-clamp-6 break-words whitespace-pre-wrap text-fg-muted">{asked}</span>
      )}
    </div>
  );
}

/**
 * Every agent row, one shape: subagents, the threads this one delegated to, and
 * the header of a group of them. A mark of the harness with a badge for how it
 * goes, the name over the line it is on, then how long, and a chevron. What it
 * was asked and the model are in the hover tip, not the row.
 */
export function AgentRow({
  harness,
  state,
  title,
  detail,
  elapsed,
  tooltip,
  label,
  onClick,
  expanded,
  action,
}: {
  harness?: string;
  state: AgentState;
  title: string;
  /** The line under the name: the step it is on, what it answered, or nothing for the status. */
  detail?: string;
  elapsed?: string;
  tooltip?: ReactNode;
  label?: string;
  onClick?: () => void;
  expanded?: boolean;
  /** A control beside the row, after the chevron: stop this work, show it beside this thread. */
  action?: ReactNode;
}) {
  const word = detail !== undefined && state !== "done" ? AGENT_LABEL[state] : undefined;
  const bottom = detail ?? AGENT_LABEL[state];
  const rowClass = "flex w-full min-w-0 flex-1 items-center gap-2.5 rounded-lg px-2 py-1.5 text-left";
  const body = (
    <>
      <AgentAvatar harness={harness} state={state} />
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="min-w-0 truncate font-medium text-fg text-sub">{title}</span>
        <span
          className={cx("block truncate text-caption", state === "failed" ? "text-danger" : "text-fg-muted")}
        >
          {bottom}
        </span>
      </span>
      {word && (
        <span
          className={cx(
            "shrink-0 text-caption",
            state === "failed" ? "text-danger" : state === "needs" ? "text-warning" : "text-fg-muted",
          )}
        >
          {word}
        </span>
      )}
      {elapsed && <span className="shrink-0 text-caption text-fg-faint tabular">{elapsed}</span>}
      {(onClick !== undefined || expanded !== undefined) && (
        <ChevronRight
          size={14}
          aria-hidden
          className={cx(
            "shrink-0 text-fg-faint transition-transform duration-200 group-hover:text-fg",
            expanded && "rotate-90",
          )}
        />
      )}
    </>
  );
  return (
    <div className="group flex w-full min-w-0 items-center">
      <Tooltip label={tooltip}>
        {onClick === undefined ? (
          <div className={rowClass}>{body}</div>
        ) : (
          <button
            type="button"
            aria-label={label}
            aria-expanded={expanded}
            onClick={onClick}
            className={cx(rowClass, "hover:bg-fill-subtle")}
          >
            {body}
          </button>
        )}
      </Tooltip>
      {action}
    </div>
  );
}

/** `Claude Code · haiku`: what runs the agent, for the row's hover tip. */
function useAgentMeta(harness: string | undefined, model: string | undefined): string {
  const name = useApp((s) =>
    harness ? (s.harnesses?.harnesses.find((h) => h.id === harness)?.name ?? harness) : undefined,
  );
  return [name, modelName(model)].filter(Boolean).join(" · ");
}

/**
 * A thread this one started (`start_session`), messaged, or is waiting on, as
 * an agent row: which harness and model it runs on, what it was asked (or what
 * it answered), how it goes, and for how long. Clicking opens it.
 */
export function ThreadAgentRow({
  link,
  model,
  action,
}: {
  link: Link;
  model?: string;
  /** A control of the caller's, after the row's own: stop this work, or dismiss it. */
  action?: ReactNode;
}) {
  const state = useApp((s) => stateOf(s, link));
  const session = useApp((s) => s.sessions.find((x) => x.id === link.target));
  const active = isActive(state);
  const now = useNow(active);
  const harness = link.harness ?? session?.harness;
  const meta = useAgentMeta(harness, session?.model ?? model);
  const end = active ? now : session?.updatedAt;
  const elapsedMs = end !== undefined && end - link.createdAt >= 1000 ? end - link.createdAt : undefined;
  const title = session?.title || link.targetTitle;
  return (
    <AgentRow
      harness={harness}
      state={LINK_AGENT_STATE[state]}
      title={title}
      detail={active ? firstLine(link.message) : firstLine(link.answer)}
      elapsed={elapsedMs === undefined ? undefined : formatDuration(Math.floor(elapsedMs / 1000) * 1000)}
      label={`Open ${title}`}
      tooltip={<AgentTip title={title} meta={meta} task={link.message} />}
      onClick={() => selectThread(link.target)}
      action={
        <>
          <Tooltip label="Show beside this thread">
            <button
              type="button"
              aria-label={`Show ${title} beside this thread`}
              onClick={() => peekThread(link.target)}
              className="flex size-6 shrink-0 items-center justify-center rounded-sm text-fg-faint opacity-0 hover:bg-fill hover:text-fg group-focus-within:opacity-100 group-hover:opacity-100 focus-visible:opacity-100"
            >
              <PanelRight size={12} />
            </button>
          </Tooltip>
          {action}
        </>
      }
    />
  );
}

/**
 * A subagent, as an agent row: the step it is on while it works, what it
 * answered once done; its steps stay folded behind the chevron. Claude Code's
 * `Agent`/`Task` calls carry their steps in the thread; a RustCode agent's come
 * from its own conversation.
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
  const state: AgentState = running ? "working" : failed ? "failed" : "done";
  return (
    <div>
      <AgentRow
        harness={parentHarness}
        state={state}
        title={name}
        detail={running ? (current ? stepLabel(current) : "Starting") : firstLine(output)}
        elapsed={elapsed}
        label={`${name}: ${agent.task}`}
        expanded={open}
        onClick={() => setOpen(!open)}
        tooltip={<AgentTip title={name} meta={meta} task={agent.task} />}
      />
      {open && (
        <div className="rc-expand mt-0.5 mb-1 ml-[42px] flex flex-col rounded-xl border border-border bg-surface-2 px-3 py-2">
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
          {!loadingSteps && steps.length === 0 && !output && (
            <span className="text-caption text-fg-faint">No steps recorded.</span>
          )}
          {output && !running && output.includes("\n") && (
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
 * Two or more subagents a turn started together, as one soft card: a header of
 * stacked marks, `3 agents · 2 working` and how long; open, each agent's own
 * row inside it.
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
  const parentHarness = useApp((s) => s.threads[threadId]?.harness?.id);
  const states = items.map((item) => {
    const agentId =
      item.title === "spawn_agent" && item.status !== "failed" ? spawnedAgentId(item.output) : undefined;
    const info = agentId === undefined ? undefined : agents.find((a) => a.id === agentId);
    const running = info ? agentActive(info) : live(item);
    const failed = info ? info.status === "failed" : item.status === "failed";
    return (running ? "working" : failed ? "failed" : "done") as AgentState;
  });
  const working = states.filter((x) => x === "working").length;
  const failed = states.filter((x) => x === "failed").length;
  // One mark for the lot: they all run on this thread's harness, so a stack of them says nothing.
  const groupState: AgentState = working ? "working" : failed ? "failed" : "done";
  const now = useNow(working > 0);
  const started = Math.min(...items.map((i) => i.startedAt ?? Number.POSITIVE_INFINITY));
  const ended = Math.max(...items.map((i) => i.endedAt ?? 0));
  const elapsedMs = Number.isFinite(started) ? (working ? now : ended || now) - started : undefined;
  const status = working ? `${working} working` : failed ? `${failed} failed` : "done";
  return (
    <div className={cx("rounded-xl border border-border bg-surface p-1", !working && !open && "opacity-80")}>
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        className="group flex w-full min-w-0 items-center gap-2.5 rounded-lg px-2 py-1 text-left hover:bg-fill-subtle"
      >
        <AgentAvatar harness={parentHarness} state={groupState} />
        <span className="flex min-w-0 flex-1 items-baseline gap-1.5">
          <span className="shrink-0 font-medium text-fg text-sub">{items.length} agents</span>
          <span
            className={cx(
              "min-w-0 truncate text-caption",
              working ? "text-accent" : failed ? "text-danger" : "text-fg-muted",
            )}
          >
            · {status}
          </span>
        </span>
        {elapsedMs !== undefined && elapsedMs >= 1000 && (
          <span className="shrink-0 text-caption text-fg-faint tabular">
            {formatDuration(Math.floor(elapsedMs / 1000) * 1000)}
          </span>
        )}
        <ChevronRight
          size={14}
          aria-hidden
          className={cx(
            "shrink-0 text-fg-faint transition-transform duration-200 group-hover:text-fg",
            open && "rotate-90",
          )}
        />
      </button>
      {open && (
        <div className="rc-expand mt-0.5 flex flex-col gap-0.5 pb-0.5">
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
