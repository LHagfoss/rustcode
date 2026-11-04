// What a thread's Details panel shows, worked out from the thread: the
// subagents it started on any harness, its plan progress and its context
// window. Pure, so the panel only draws.

import { agentActive, type SubagentInfo, spawnedAgentId } from "../acp/rustcode";
import type { PlanEntry, SessionUsage } from "../acp/types";
import { subagentOf, type TranscriptItem } from "./thread";

export type SubagentState = "working" | "done" | "failed";

export interface SubagentLine {
  key: string;
  name: string;
  task: string;
  state: SubagentState;
  startedAt?: number;
  endedAt?: number;
}

/**
 * Every subagent of a thread, in the order they started: a harness's `Agent`
 * and `Task` calls, and RustCode's own agents. A `spawn_agent` call the agent
 * list knows is the same agent, so it only shows once.
 */
export function subagentLines(
  items: readonly TranscriptItem[],
  agents: readonly SubagentInfo[],
): SubagentLine[] {
  const known = new Set(agents.map((a) => a.id));
  const lines: SubagentLine[] = [];
  for (const item of items) {
    if (item.kind !== "tool") continue;
    const agent = subagentOf(item);
    if (!agent) continue;
    const spawned = item.title === "spawn_agent" ? spawnedAgentId(item.output) : undefined;
    if (spawned !== undefined && known.has(spawned)) continue;
    lines.push({
      key: item.id,
      name: agent.name,
      task: agent.task,
      state:
        item.status === "failed"
          ? "failed"
          : item.status === "pending" || item.status === "in_progress" || item.background === "agent"
            ? "working"
            : "done",
      startedAt: item.startedAt,
      endedAt: item.endedAt,
    });
  }
  for (const a of agents) {
    lines.push({
      key: `rc-${a.id}`,
      name: a.agentType && a.agentType !== "default" ? a.agentType : a.name,
      task: a.task.split("\n")[0] ?? "",
      state: agentActive(a) ? "working" : a.status === "failed" ? "failed" : "done",
      startedAt: a.startedAt,
      endedAt: a.finishedAt,
    });
  }
  return lines;
}

/** `3 of 5 done`, or nothing without a plan. */
export function planProgress(plan: readonly PlanEntry[]): string | undefined {
  if (!plan.length) return undefined;
  return `${plan.filter((p) => p.status === "completed").length} of ${plan.length} done`;
}

/** How full the context window is, 0-100; nothing when the harness reports no size. */
export function contextPercent(usage: SessionUsage | undefined): number | undefined {
  if (!usage?.size) return undefined;
  return Math.min(100, Math.round((usage.used / usage.size) * 100));
}
