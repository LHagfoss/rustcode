// The stack of delegates a parent shows instead of child rows: which icons are
// drawn, how many are folded into `+N`, in what order and how the stack reads.
// Pure, so the sidebar, the chat's group cards and their tests share one answer
// (ui/AgentStack.tsx).

/** A delegate as the stack reads it: what it is doing and what runs it. */
export type StackState = "needs" | "working" | "done" | "failed";

export interface StackAgent {
  id: string;
  state: StackState;
  /** The harness it runs on; absent for a subagent, which runs on its parent's. */
  harness?: string;
  /** Its title, for the stack's tooltip. */
  label?: string;
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
  return {
    visible,
    extra: order.length - visible.length,
    order,
    needs,
    working,
    running: needs + working > 0,
    label: stackLabel(needs, working, order.length),
  };
}
