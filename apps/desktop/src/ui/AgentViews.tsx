// Subagents, one way for every harness: a row per agent a turn started (the
// same row the threads this one delegated to get), that says what it is doing
// while it works and what it answered once done, and how long it took.
// RustCode's own agents (`spawn_agent`) take the same row, fed by
// `_rustcode/agents`; a harness's own agent at work reads in the same strip
// above the composer as the delegates (`ui/Links.tsx`), not in a surface of its
// own.

import { Bot, Check, ChevronDown, ChevronRight, CircleX, Minus, PanelRight, X } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";
import { agentActive, agentElapsed, type SubagentInfo, spawnedAgentId } from "../acp/rustcode";
import { modelName } from "../acp/types";
import type { StackAgent } from "../state/agentStack";
import { answerPreview } from "../state/answerPreview";
import { AGENT_WORD, type AgentState, LINK_AGENT_STATE } from "../state/delegates";
import { dismissLink, stateOf } from "../state/host";
import {
  isActive,
  type Link,
  type LinkEvent,
  type LinkState,
  linkCounts,
  linkEventText,
  linkGroupStatus,
  linkStatusText,
} from "../state/orchestration";
import { agentHistory, peekThread, selectThread, useApp } from "../state/store";
import {
  agentStepLabel,
  formatDuration,
  relativeAge,
  stepLabel,
  stripHarnessMarkup,
  subagentOf,
  subagentTitle,
  type TranscriptItem,
  workingLabel,
} from "../state/thread";
import { AgentStack } from "./AgentStack";
import { HarnessIcon } from "./HarnessIcon";
import { Markdown } from "./Markdown";
import { cx, Skeleton, Spinner, Tooltip } from "./primitives";

const EMPTY: SubagentInfo[] = [];
const NO_STEPS: ToolItem[] = [];

export function useNow(active: boolean) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!active) return;
    // The first render after work starts must not use a `now` from when the row was idle.
    setNow(Date.now());
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

export { type AgentState, LINK_AGENT_STATE } from "../state/delegates";

const AGENT_LABEL: Record<AgentState, string> = AGENT_WORD;

/** Who is doing the work, and a badge for how it goes. The badge pops in when the state changes. */
export function AgentAvatar({
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
  return answerPreview(text);
}

/** What a hover on an agent row says: what it was asked, in full, what it is to
 * this thread, how it goes and for how long, and what runs it. A delegate's tip
 * also lists what happened to the link, oldest first, each with its age. */
export function AgentTip({
  title,
  meta,
  task,
  state,
  status,
  elapsed,
  relationship,
  events,
}: {
  title: string;
  meta?: string;
  task?: string;
  /** Its state, in words: `Working`, `Needs you`, `Done`, `Failed`, `Stopped`. */
  state?: AgentState;
  /** The exact state word, for a link: `Queued`, `Interrupted`, `Cancelled` keep their names. */
  status?: string;
  /** How long it has run. */
  elapsed?: string;
  /** What it is to the thread: `Subagent`, `Thread it started`, `Thread it messaged`. */
  relationship?: string;
  /** A delegate's events, oldest first: the chip's own list, with how long ago each was. */
  events?: readonly LinkEvent[];
}) {
  const asked = task?.trim();
  const now = Date.now();
  return (
    <div className="flex flex-col gap-0.5">
      <span className="font-medium">{title}</span>
      {meta && <span className="text-fg-muted">{meta}</span>}
      {(relationship || state || status || elapsed) && (
        <span className="flex items-center gap-1.5 text-fg-faint">
          {relationship && <span>{relationship}</span>}
          {state && (
            <span
              className={cx(
                "font-medium",
                state === "needs"
                  ? "text-warning"
                  : state === "working"
                    ? "text-accent"
                    : state === "failed"
                      ? "text-danger"
                      : undefined,
              )}
            >
              {status ?? AGENT_LABEL[state]}
            </span>
          )}
          {elapsed && <span className="ml-auto tabular">{elapsed}</span>}
        </span>
      )}
      {events && events.length > 0 && (
        <div className="mt-1 flex flex-col gap-[3px] border-border/60 border-t pt-1">
          {events.map((event, i) => (
            <span key={`${event.kind}-${event.at}`} className="flex items-baseline gap-3">
              <span className={i === events.length - 1 ? "font-medium text-fg-muted" : undefined}>
                {linkEventText(event)}
              </span>
              <span className="ml-auto shrink-0 text-fg-faint tabular">{relativeAge(event.at, now)}</span>
            </span>
          ))}
        </div>
      )}
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
  status,
  description,
  note,
  onClick,
  expanded,
  showWord = true,
  oneLine = false,
  arrow = true,
  action,
}: {
  harness?: string;
  state: AgentState;
  title: string;
  /** The line under the name: the step it is on, what it answered, or nothing for the status. */
  detail?: string;
  /** The exact state word for a link, when `detail` does not replace it. */
  status?: string;
  /** What an informational row says in words, for the group it exposes to assistive tech. */
  description?: string;
  /** A short count before the row's controls: `7 messages` for a thread messaged again. */
  note?: string;
  /** The row's trailing slot: how long it has run, or a thread's status mark and age. */
  elapsed?: ReactNode;
  tooltip?: ReactNode;
  label?: string;
  onClick?: () => void;
  expanded?: boolean;
  /**
   * Whether the state word sits beside the row. Off while a row shows its own
   * step: a running agent's elapsed time already says it is working.
   */
  showWord?: boolean;
  /**
   * One line of columns, the name beside its state and how long, with no line under
   * it: a popover's list of rows reads as a table and the name keeps the room.
   */
  oneLine?: boolean;
  /** Whether the row draws the arrow that is its way in, beside the row itself. */
  arrow?: boolean;
  /** A control beside the row, after the chevron: stop this work, show it beside this thread. */
  action?: ReactNode;
}) {
  const word =
    showWord && (oneLine || (detail !== undefined && (state !== "done" || status !== undefined)))
      ? (status ?? AGENT_LABEL[state])
      : undefined;
  const bottom = oneLine ? undefined : (detail ?? status ?? AGENT_LABEL[state]);
  // The row's own text starts where the composer's does: the card's 4px and the row's 12px.
  const rowClass = "flex w-full min-w-0 flex-1 items-center gap-2 rounded-lg py-1.5 pr-2 pl-3 text-left";
  // A row that opens is its own way in: pressing its name opens the thread, so a second arrow
  // beside it doing the same was one control too many. Only a row that folds (a subagent and its
  // steps) draws the arrow, says what it does in its own words, and turns it as it opens.
  const chevron = arrow && expanded !== undefined;
  const chevronWord = expanded ? "Hide the steps" : "Show the steps";
  const body = (
    <>
      <AgentAvatar harness={harness} state={state} />
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="min-w-0 truncate font-medium text-fg text-sub">{title}</span>
        {bottom !== undefined && (
          <span
            className={cx(
              "block truncate text-caption",
              state === "failed" ? "text-danger" : "text-fg-muted",
            )}
          >
            {bottom}
          </span>
        )}
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
      {/* The clock is part of the row the pointer opens, not a dead patch beside it. Its slot is
          wide enough for any length it takes and right-aligned, so a second ticking over never
          shifts the arrow beside it. */}
      {elapsed && (
        <span className="flex min-w-10 shrink-0 items-center justify-end text-caption text-fg-faint tabular">
          {elapsed}
        </span>
      )}
      {note && <span className="shrink-0 text-caption text-fg-faint">{note}</span>}
    </>
  );
  return (
    <div className="group flex w-full min-w-0 items-center">
      <Tooltip label={tooltip}>
        {onClick === undefined ? (
          // A row that only states something is a named, non-focusable group: no roleless tab stop,
          // and the hover details are in its accessible name for assistive tech.
          <div role="group" aria-label={description ?? title} className={rowClass}>
            {body}
          </div>
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
      {/* The way in and the control beside it, in one cluster: 4px apart, after the row's own
          right padding, so they read as the row's tail rather than two islands at the card's edge.
          The arrow is a control like the one beside it, never a mark that ignores a press. */}
      {(chevron || action) && (
        <span className="flex shrink-0 items-center justify-end gap-1 pr-0.5">
          {chevron && onClick !== undefined && (
            <Tooltip label={chevronWord}>
              <button
                type="button"
                aria-label={chevronWord}
                aria-expanded={expanded}
                onClick={onClick}
                className="flex size-[22px] shrink-0 items-center justify-center rounded-sm text-fg-faint hover:bg-fill hover:text-fg active:bg-fill-strong"
              >
                <ChevronRight
                  size={14}
                  aria-hidden
                  className={cx("transition-transform duration-200", expanded && "rotate-90")}
                />
              </button>
            </Tooltip>
          )}
          {action}
        </span>
      )}
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
  messages,
  action,
  peek = true,
  popover = false,
}: {
  link: Link;
  model?: string;
  /** How many links this thread has: a message sent again is still one row. */
  messages?: number;
  /** A control of the caller's, after the row's own: stop this work, or dismiss it. */
  action?: ReactNode;
  /**
   * The "show beside this thread" control. Off in the strip above the composer, where the row is
   * already that thread's one way in and a control that only shows on hover would leave a gap
   * between the way in and stop.
   */
  peek?: boolean;
  /**
   * One line of columns, for a popover's list: the state word always, how long when it is known,
   * and the arrow left out because the row itself is the way in.
   */
  popover?: boolean;
}) {
  const state = useApp((s) => stateOf(s, link));
  const session = useApp((s) => s.sessions.find((x) => x.id === link.target));
  const active = isActive(state);
  // What the thread does now, when this one has it open: its live line is the row's own step.
  const step = useApp((s) =>
    active && s.threads[link.target]?.running ? workingLabel(s.threads[link.target]!.items, true) : undefined,
  );
  const now = useNow(active);
  const harness = link.harness ?? session?.harness;
  const meta = useAgentMeta(harness, session?.model ?? model);
  const end = active ? now : link.settledAt;
  const elapsedMs = end !== undefined && end - link.createdAt >= 1000 ? end - link.createdAt : undefined;
  const elapsed = elapsedMs === undefined ? undefined : formatDuration(elapsedMs);
  // The chip's status is the link's own last event: `Started`, `Message sent`, `Asked you`,
  // `Answered`, `Merged`. A link saved before this build keeps its state word.
  const status = linkStatusText(link, state);
  const events = link.events ?? [];
  const title = session?.title || link.targetTitle;
  return (
    <AgentRow
      harness={harness}
      state={LINK_AGENT_STATE[state]}
      status={status}
      title={title}
      // The prompt is in the tip: its first line is boilerplate, and the title already says the job.
      detail={
        active
          ? state === "queued"
            ? undefined
            : (step ?? "Working")
          : state === "failed" && link.error
            ? firstLine(link.error)
            : firstLine(stripHarnessMarkup(link.answer ?? ""))
      }
      elapsed={elapsed}
      note={messages !== undefined && messages > 1 ? `${messages} messages` : undefined}
      label={`Open ${title}`}
      tooltip={
        <AgentTip
          title={title}
          meta={meta}
          relationship={link.started ? "Thread it started" : "Thread it messaged"}
          state={LINK_AGENT_STATE[state]}
          status={status}
          elapsed={elapsed}
          task={link.message}
          events={events}
        />
      }
      onClick={() => selectThread(link.target)}
      showWord={popover || LINK_AGENT_STATE[state] !== "working"}
      oneLine={popover}
      arrow={!popover}
      action={
        <>
          {peek && (
            <Tooltip label="Show beside this thread">
              <button
                type="button"
                aria-label={`Show ${title} beside this thread`}
                onClick={() => peekThread(link.target)}
                className="flex size-[22px] shrink-0 items-center justify-center rounded-sm text-fg-faint opacity-0 hover:bg-fill hover:text-fg group-focus-within:opacity-100 group-hover:opacity-100 focus-visible:opacity-100"
              >
                <PanelRight size={12} />
              </button>
            </Tooltip>
          )}
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
  const raw = info ? info.completion : item.title === "spawn_agent" ? undefined : item.output;
  // The harness tags a subagent's answer may open with are for the model, not the row.
  const output = raw === undefined ? undefined : stripHarnessMarkup(raw) || undefined;
  const name = info?.agentType && info.agentType !== "default" ? info.agentType : agent.name;
  // `Agent` says nothing; the task does. A named type keeps its name, and the tooltip its task.
  const title = subagentTitle(name, agent.task);
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
        title={title}
        detail={running ? (current ? agentStepLabel(current) : "Starting") : firstLine(output)}
        elapsed={elapsed}
        label={title === agent.task.trim() ? title : `${title}: ${agent.task}`}
        expanded={open}
        onClick={() => setOpen(!open)}
        tooltip={
          <AgentTip
            title={name}
            meta={meta}
            relationship="Subagent"
            state={state}
            elapsed={elapsed}
            task={agent.task}
          />
        }
        showWord={state !== "working"}
      />
      {open && (
        <div className="rc-expand mt-0.5 mb-1 ml-[42px] flex flex-col rounded-xl bg-fill-subtle px-3 py-2">
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
            <div className="mt-1.5 max-h-64 overflow-y-auto rounded-lg bg-fill-subtle p-2">
              <Markdown text={output} className="text-callout" />
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * What a group card says: the stack of its agents' marks, `2 agents · 2 working`,
 * how long, and a chevron. Shared by the transcript's agent and thread groups
 * and by the strip above the composer, so a group reads the same wherever it is.
 */
export function AgentGroupHeader({
  harness,
  agents,
  state,
  label,
  status,
  elapsedMs,
  open,
  chevron = "fold",
}: {
  harness?: string;
  /** The group's agents: their marks are stacked in place of the one what-runs-it mark. */
  agents?: readonly StackAgent[];
  state: AgentState;
  /** `2 agents`, `5 running`: the count and what it counts. */
  label: string;
  /** What follows the count after its `·`: `2 need you`; empty when the label says it all. */
  status: string;
  elapsedMs?: number;
  open: boolean;
  /** A fold turns its chevron as it opens; a popover's points down. */
  chevron?: "fold" | "down";
}) {
  return (
    <>
      {agents?.length ? (
        <AgentStack agents={agents} size="chat" tone="surface" />
      ) : (
        <AgentAvatar harness={harness} state={state} />
      )}
      <span className="flex min-w-0 flex-1 items-baseline gap-1.5">
        <span className="shrink-0 font-medium text-fg text-sub">{label}</span>
        {/* A live header's second word (`2 need you`); one whose label says it all has none. */}
        {status && (
          <span
            className={cx(
              "min-w-0 truncate text-caption",
              state === "working"
                ? "text-accent"
                : state === "needs"
                  ? "text-warning"
                  : state === "failed"
                    ? "text-danger"
                    : "text-fg-muted",
            )}
          >
            · {status}
          </span>
        )}
      </span>
      {elapsedMs !== undefined && elapsedMs >= 1000 && (
        <span className="shrink-0 text-caption text-fg-faint tabular">
          {formatDuration(Math.floor(elapsedMs / 1000) * 1000)}
        </span>
      )}
      {chevron === "down" ? (
        <ChevronDown size={14} aria-hidden className="shrink-0 text-fg-faint group-hover:text-fg" />
      ) : (
        <ChevronRight
          size={14}
          aria-hidden
          className={cx(
            "shrink-0 text-fg-faint transition-transform duration-200 group-hover:text-fg",
            open && "rotate-90",
          )}
        />
      )}
    </>
  );
}

/**
 * The soft card a group of agents or threads sits in: one header toggles it,
 * with the same insets as the rows, so opening it does not move the header.
 */
export function AgentGroupCard({
  harness,
  agents,
  state,
  label,
  status,
  elapsedMs,
  open,
  onToggle,
  faint,
  children,
}: {
  harness?: string;
  agents?: readonly StackAgent[];
  state: AgentState;
  label: string;
  status: string;
  elapsedMs?: number;
  open: boolean;
  onToggle: () => void;
  /** Dim it while nothing in it runs and it is folded away. */
  faint?: boolean;
  children: ReactNode;
}) {
  return (
    <div className={cx("rounded-xl bg-surface p-1", faint && "opacity-80")}>
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        className="group flex w-full min-w-0 items-center gap-2 rounded-lg px-2 py-1.5 text-left hover:bg-fill-subtle"
      >
        <AgentGroupHeader
          harness={harness}
          agents={agents}
          state={state}
          label={label}
          status={status}
          elapsedMs={elapsedMs}
          open={open}
        />
      </button>
      {open && <div className="rc-expand flex flex-col gap-0.5">{children}</div>}
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
  // Native subagents have no harness of their own: the stack shows the mark of the thread they run
  // on, the same mark the single avatar showed.
  const marks: StackAgent[] = items.map((item) => {
    const agentId =
      item.title === "spawn_agent" && item.status !== "failed" ? spawnedAgentId(item.output) : undefined;
    const info = agentId === undefined ? undefined : agents.find((a) => a.id === agentId);
    const running = info ? agentActive(info) : live(item);
    const failed = info ? info.status === "failed" : item.status === "failed";
    const state = running ? "working" : failed ? "failed" : "done";
    return {
      id: item.id,
      state,
      kind: "subagent",
      harness: parentHarness,
      label: info?.name ?? item.title,
      since: item.startedAt,
    };
  });
  const working = marks.filter((x) => x.state === "working").length;
  const failed = marks.filter((x) => x.state === "failed").length;
  const groupState: AgentState = working ? "working" : failed ? "failed" : "done";
  const now = useNow(working > 0);
  const started = Math.min(...items.map((i) => i.startedAt ?? Number.POSITIVE_INFINITY));
  const ended = Math.max(...items.map((i) => i.endedAt ?? 0));
  const elapsedMs = Number.isFinite(started) ? (working ? now : ended || now) - started : undefined;
  const status = working ? `${working} working` : failed ? `${failed} failed` : "done";
  return (
    <AgentGroupCard
      harness={parentHarness}
      agents={marks}
      state={groupState}
      label={`${items.length} agents`}
      status={status}
      elapsedMs={elapsedMs}
      open={open}
      onToggle={onToggle}
      faint={!working && !open}
    >
      {items.map((item) => (
        <SubagentRow key={item.id} item={item} steps={stepsOf(item.id)} threadId={threadId} />
      ))}
    </AgentGroupCard>
  );
}

/** Take a settled thread's row out of the transcript: it dismisses the link. */
function RemoveLink({ link }: { link: Link }) {
  return (
    <Tooltip label="Remove">
      <button
        type="button"
        aria-label={`Remove ${link.targetTitle}`}
        onClick={() => dismissLink(link.id)}
        className="flex size-[22px] shrink-0 items-center justify-center rounded-sm text-fg-faint opacity-0 hover:bg-fill hover:text-fg group-focus-within:opacity-100 group-hover:opacity-100 focus-visible:opacity-100"
      >
        <X size={12} strokeWidth={2.5} />
      </button>
    </Tooltip>
  );
}

/**
 * The threads a turn started, read in the transcript. While one works the strip
 * above the composer is the live place for it (Stop lives there), so a row
 * appears here once its thread settles; two or more read as one card, the same
 * card subagents use.
 */
export function StartedThreadsRow({
  links,
  open,
  onToggle,
}: {
  links: Link[];
  open: boolean;
  onToggle: () => void;
}) {
  const states = useApp((s) => links.map((l) => stateOf(s, l)).join(",")).split(",") as LinkState[];
  // Only what the strip no longer shows: a working thread is its row up there.
  const settled = links.flatMap((link, i) => (isActive(states[i]!) ? [] : [{ link, state: states[i]! }]));
  if (!settled.length) return null;
  const row = (link: Link) => (
    <ThreadAgentRow key={link.id} link={link} action={<RemoveLink link={link} />} />
  );
  if (settled.length === 1) return row(settled[0]!.link);
  const counts = linkCounts(settled.map((s) => s.state));
  const groupState: AgentState = counts.working
    ? "working"
    : counts.needs
      ? "needs"
      : counts.failed
        ? "failed"
        : "done";
  const started = Math.min(...settled.map((s) => s.link.createdAt));
  const ended = Math.max(...settled.map((s) => s.link.settledAt ?? 0));
  const marks: StackAgent[] = settled.map(({ link, state }) => {
    const agent = LINK_AGENT_STATE[state];
    return {
      id: link.target,
      state: agent === "stopped" ? "done" : agent,
      kind: "thread",
      harness: link.harness,
      label: link.targetTitle,
      since: link.createdAt,
    };
  });
  return (
    <AgentGroupCard
      agents={marks}
      state={groupState}
      label={`${settled.length} threads`}
      status={linkGroupStatus(counts)}
      elapsedMs={(ended || started) - started}
      open={open}
      onToggle={onToggle}
      faint={!counts.working && !open}
    >
      {settled.map((s) => row(s.link))}
    </AgentGroupCard>
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
