// Subagents: listing a thread's subagents, reading one's history, and the poll that keeps the
// list fresh while a thread runs. Split out of state/store.ts; the state it writes still lives in the app store.

import type { JsonObject } from "../acp/jsonrpc";
import { agentActive, parseAgentList, type SubagentInfo } from "../acp/rustcode";
import { Method, parseSessionUpdate } from "../acp/types";
import { type AppState, connectionForThread, useApp } from "./store";
import { applyUpdate, emptyThread, finishLoad, isRunning, type ThreadState } from "./thread";
import { splitThreadKey } from "./threadKeys";

// Read through `useApp` at call time: store.ts imports this module, so `useApp` is not defined yet
// while this one loads.
const get = () => useApp.getState();
const set = (patch: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => useApp.setState(patch);

/** Whether the thread's harness lists subagents (`_rustcode/agents`). */
export function hasAgents(thread?: ThreadState): boolean {
  return !!get().agent?.agents && (thread?.harness?.capabilities.subagents ?? true);
}

export async function refreshAgents(id: string): Promise<SubagentInfo[]> {
  const thread = get().threads[id];
  const conn = connectionForThread(id);
  if (!conn || !hasAgents(thread)) return [];
  try {
    const agents = parseAgentList(
      await conn.request(Method.agents, { sessionId: splitThreadKey(id).id, action: "list" }, 10_000),
    );
    set((s) => ({ agents: { ...s.agents, [id]: agents } }));
    return agents;
  } catch {
    return get().agents[id] ?? [];
  }
}

/** A child's conversation, folded like a replayed thread. */
export async function agentHistory(id: string, agentId: number): Promise<ThreadState> {
  const conn = connectionForThread(id);
  if (!conn) throw new Error("That computer is not connected.");
  const result = (await conn.request(
    Method.agents,
    { sessionId: splitThreadKey(id).id, action: "history", id: agentId },
    15_000,
  )) as JsonObject | null;
  let state = emptyThread(`${id}#${agentId}`, get().threads[id]?.cwd ?? "");
  for (const update of Array.isArray(result?.updates) ? result.updates : []) {
    state = applyUpdate(state, parseSessionUpdate({ sessionId: id, update }));
  }
  return finishLoad(state);
}

let agentTimer: ReturnType<typeof setInterval> | null = null;

/** Poll the shown thread's agents while its turn runs or a child works, as the SwiftUI app does. */
export function startAgentPolling() {
  if (agentTimer) return;
  agentTimer = setInterval(() => {
    const s = get();
    const id = s.selectedId;
    const thread = id ? s.threads[id] : undefined;
    if (!id || !thread || s.status !== "ready" || !hasAgents(thread)) return;
    const known = s.agents[id];
    if (isRunning(thread) || known?.some(agentActive) || (known === undefined && thread.loaded)) {
      void refreshAgents(id);
    }
  }, 2000);
}
