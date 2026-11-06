// The stack of delegates a parent shows instead of child rows: which icons are
// drawn, how many are folded into `+N`, in what order and how the stack reads.
// Pure, so the sidebar, the chat's group cards and their tests share one answer
// (ui/AgentStack.tsx).

import { agentActive, type SubagentInfo } from "../acp/rustcode";
import { subagentOf, type TranscriptItem } from "./thread";

/** A delegate as the stack reads it: what it is doing and what runs it. */
export type StackState = "needs" | "working" | "done" | "failed";

/** The state word the stack and its tip use, in one place. */
export const STACK_STATE_WORD: Record<StackState, string> = {
  needs: "needs you",
  working: "working",
  done: "done",
  failed: "failed",
};

/**
 * What a mark stands for: a delegated thread, which has its own thread to open,
 * or a subagent, which runs inside the thread that started it and has none.
 */
export type StackKind = "thread" | "subagent";

export interface StackAgent {
  id: string;
  state: StackState;
  /** A thread opens; a subagent has no thread of its own (`kind`). */
  kind?: StackKind;
  /** The harness it runs on; absent for a subagent, which runs on its parent's. */
  harness?: string;
  /** Its title, for the stack's tooltip. */
  label?: string;
  /** When it started, so the tip can say how long it has run. */
  since?: number;
}

export interface AgentStackView {
  /** At most `STACK_VISIBLE` icons: what wants you first, then what works. */
  visible: StackAgent[];
  /** The delegates past the last icon, for the `+N`. */
  extra: number;
  /** Every delegate the stack stands for, in draw order: the tooltip lists them. */
  order: StackAgent[];
  needs: number;
  working: number;
  /**
   * The stack's state as one thing, for the single badge it draws: what needs you
   * outranks what failed, which outranks work under way, and a stack whose marks
   * all finished is `done` (its text says so, and it draws no badge).
   */
  state: StackState;
  /** The delegated threads the stack holds (`kind: "thread"`). */
  threads: number;
  /** The subagents the stack holds (`kind: "subagent"`). */
  subagents: number;
  /** `2 threads and 1 subagent`, or empty when every mark left its kind out. */
  summary: string;
  /** Whether anything in the stack works or waits on the user. */
  running: boolean;
  /** One line: `1 needs you · 2 working`, or `3 finished` when idle is shown. */
  label: string;
}

/** How many icons a stack draws before it counts the rest. */
export const STACK_VISIBLE = 3;

/** What wants you comes first, then what works, then what has finished. */
const rank = (agent: StackAgent) => (agent.state === "needs" ? 0 : agent.state === "working" ? 1 : 2);

function stackLabel(needs: number, working: number, finished: number): string {
  const parts: string[] = [];
  if (needs) parts.push(`${needs} ${needs === 1 ? "needs" : "need"} you`);
  if (working) parts.push(`${working} working`);
  if (!parts.length && finished) parts.push(`${finished} finished`);
  return parts.join(" · ");
}

/** How many of the marks are threads and how many are subagents, in words. */
function stackKinds(order: readonly StackAgent[]): {
  threads: number;
  subagents: number;
  summary: string;
} {
  const threads = order.filter((agent) => agent.kind === "thread").length;
  const subagents = order.filter((agent) => agent.kind === "subagent").length;
  const parts: string[] = [];
  if (threads) parts.push(`${threads} ${threads === 1 ? "thread" : "threads"}`);
  if (subagents) parts.push(`${subagents} ${subagents === 1 ? "subagent" : "subagents"}`);
  return { threads, subagents, summary: parts.join(" and ") };
}

/**
 * The stack for `agents`.
 *
 * - Live delegates only: what works or waits on the user. A finished delegate
 *   shows nothing here (the sidebar's stack), which is what makes the icons
 *   mean "work under way" rather than "threads this one ever started".
 * - With `idle: "show"`, a stack with nothing live keeps its finished delegates
 *   instead, so a group card whose work is done still shows who did it.
 * - Ties keep the order they came in, so an update that changes no state does
 *   not move the icons; one that needs the user moves its icon to the front.
 */
export function agentStack(
  agents: readonly StackAgent[],
  options: { idle?: "hide" | "show" } = {},
): AgentStackView {
  const live = agents.filter((agent) => agent.state === "needs" || agent.state === "working");
  const source = live.length || (options.idle ?? "hide") === "hide" ? live : agents;
  const order = source
    .map((agent, i) => ({ agent, i, rank: rank(agent) }))
    .sort((a, b) => a.rank - b.rank || a.i - b.i)
    .map((x) => x.agent);
  const visible = order.slice(0, STACK_VISIBLE);
  const needs = order.filter((agent) => agent.state === "needs").length;
  const working = order.filter((agent) => agent.state === "working").length;
  const failed = order.filter((agent) => agent.state === "failed").length;
  return {
    visible,
    extra: order.length - visible.length,
    order,
    needs,
    working,
    state: needs ? "needs" : failed ? "failed" : working ? "working" : "done",
    ...stackKinds(order),
    running: needs + working > 0,
    label: stackLabel(needs, working, order.length),
  };
}

/**
 * The stack in words for assistive tech: the counts, then every mark with its
 * state, name and kind. The tooltip is the same information for the pointer, so
 * the hover is not the only way to read it.
 */
export function stackDescription(view: AgentStackView): string {
  const counts = [view.summary, view.label].filter(Boolean).join(" · ");
  const marks = view.order
    .map((agent) => {
      const kind = agent.kind ? ` (${agent.kind})` : "";
      return `${STACK_STATE_WORD[agent.state]} ${agent.label ?? "Thread"}${kind}`;
    })
    .join("; ");
  return [`Delegated work: ${counts}`, marks].filter(Boolean).join(". ");
}

/**
 * A thread's own subagents at work, as stack marks: the harness's built-in ones
 * (Claude Code's Agent and Task calls) and RustCode's. They have no thread of
 * their own, so they show the mark of the thread they run on, and only a running
 * turn has any: a replayed call is over.
 */
export function liveSubagents(
  items: readonly TranscriptItem[] | undefined,
  agents: readonly SubagentInfo[],
  running: boolean,
  harness?: string,
): StackAgent[] {
  const out: StackAgent[] = agents.filter(agentActive).map((a) => ({
    id: `rc-${a.id}`,
    state: "working",
    kind: "subagent",
    harness,
    label: a.name,
    since: a.startedAt,
  }));
  if (!running) return out;
  for (const item of items ?? []) {
    if (item.kind !== "tool" || (item.status !== "pending" && item.status !== "in_progress")) continue;
    const agent = subagentOf(item);
    if (agent)
      out.push({
        id: item.id,
        state: "working",
        kind: "subagent",
        harness,
        label: agent.task || agent.name,
        since: item.startedAt,
      });
  }
  return out;
}
