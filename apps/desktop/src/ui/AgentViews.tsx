// Subagents, after AgentViews.swift: a row per agent a turn started, where
// it started it; a pill over the composer while agents work; and the Agents
// panel, which lists every agent of the thread and reads one's conversation,
// with a field to send it more and Stop.

import { ArrowUp, ChevronLeft, ChevronRight, CircleCheck, CircleX, Square, Users } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import {
  AGENT_STATUS_LABEL,
  agentActive,
  agentElapsed,
  compactCount,
  type SubagentInfo,
  spawnedAgentId,
} from "../acp/rustcode";
import {
  agentHistory,
  cancelAgent,
  openAgent,
  refreshAgents,
  sendToAgent,
  setPanel,
  useApp,
} from "../state/store";
import type { ThreadState, TranscriptItem } from "../state/thread";
import { Markdown } from "./Markdown";
import { cx, Spinner, Tooltip } from "./primitives";
import { AssistantRow, ThoughtRow, ToolRow, UserRow } from "./Rows";

const EMPTY: SubagentInfo[] = [];

function useNow(active: boolean) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!active) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [active]);
  return now;
}

export function AgentMark({ agent }: { agent?: SubagentInfo }) {
  if (!agent || agentActive(agent)) return <Spinner size={11} />;
  if (agent.status === "completed") return <CircleCheck size={13} className="text-success" />;
  if (agent.status === "failed") return <CircleX size={13} className="text-danger" />;
  return <Square size={10} className="text-fg-muted" />;
}

/** A `spawn_agent` call, drawn as the agent it started. */
export function AgentSpawnRow({
  item,
  threadId,
}: {
  item: Extract<TranscriptItem, { kind: "tool" }>;
  threadId: string;
}) {
  const agents = useApp((s) => s.agents[threadId] ?? EMPTY);
  const failed = item.status === "failed";
  const agentId = failed ? undefined : spawnedAgentId(item.output);
  const info = agentId !== undefined ? agents.find((a) => a.id === agentId) : undefined;
  const now = useNow(!!info && agentActive(info));
  const input = item.input && typeof item.input === "object" && !Array.isArray(item.input) ? item.input : {};
  const request = item.summary ?? (typeof input.task === "string" ? input.task : undefined);
  const name =
    info?.name ??
    (agentId !== undefined ? `agent-${agentId}` : failed ? "Agent did not start" : "Starting agent…");
  const task = info?.task.split("\n")[0] ?? request ?? (failed ? item.output?.split("\n")[0] : undefined);
  const tail = info
    ? agentActive(info)
      ? agentElapsed(info, now)
      : [
          AGENT_STATUS_LABEL[info.status],
          agentElapsed(info),
          info.inputTokens + info.outputTokens > 0
            ? `${compactCount(info.inputTokens + info.outputTokens)} tokens`
            : undefined,
        ]
          .filter(Boolean)
          .join(" · ")
    : undefined;
  return (
    <button
      type="button"
      disabled={agentId === undefined}
      onClick={() => agentId !== undefined && openAgent(agentId)}
      title={info ? `Open ${info.name}'s conversation` : undefined}
      className="group -mx-1 flex h-6 w-[calc(100%+8px)] items-center gap-2 rounded-sm px-1 text-left font-rounded text-callout hover:bg-fill-subtle disabled:hover:bg-transparent"
    >
      <span className="flex w-[18px] shrink-0 justify-center">
        {failed ? <CircleX size={13} className="text-danger" /> : <AgentMark agent={info} />}
      </span>
      <span className="shrink-0 font-medium">{name}</span>
      {info && info.agentType !== "default" && (
        <span className="shrink-0 text-fg-muted">{info.agentType}</span>
      )}
      {task && <span className="min-w-0 flex-1 truncate text-fg-muted">{task}</span>}
      {!task && <span className="flex-1" />}
      {tail && (
        <span
          className={cx(
            "shrink-0 text-caption tabular",
            info?.status === "failed" ? "text-danger" : "text-fg-faint",
          )}
        >
          {tail}
        </span>
      )}
      {agentId !== undefined && <ChevronRight size={11} className="shrink-0 text-fg-faint" />}
    </button>
  );
}

/** Over the composer while agents work: `● 2 agents working`, which opens the panel. */
export function AgentPill({ thread }: { thread?: ThreadState }) {
  const agents = useApp((s) => (thread ? (s.agents[thread.id] ?? EMPTY) : EMPTY));
  const working = agents.filter(agentActive).length;
  if (!working) return null;
  return (
    <button
      type="button"
      onClick={() => setPanel("agents")}
      className="flex h-[26px] items-center gap-1.5 rounded-lg px-2.5 text-callout text-accent hover:bg-fill"
    >
      <span className="size-1.5 animate-pulse rounded-full bg-accent" />
      {working === 1 ? "1 agent working" : `${working} agents working`}
    </button>
  );
}

function AgentListRow({ agent, now }: { agent: SubagentInfo; now: number }) {
  return (
    <button
      type="button"
      onClick={() => openAgent(agent.id)}
      className="flex w-full items-start gap-2.5 rounded-md px-2.5 py-2 text-left hover:bg-fill-subtle"
      style={{ paddingLeft: 10 + Math.max(0, agent.depth - 1) * 14 }}
    >
      <span className="mt-0.5 flex w-4 justify-center">
        <AgentMark agent={agent} />
      </span>
      <span className="min-w-0 flex-1">
        <span className="flex items-baseline gap-1.5">
          <span className="font-medium text-body">{agent.name}</span>
          {agent.agentType !== "default" && (
            <span className="text-caption text-fg-muted">{agent.agentType}</span>
          )}
          <span className="flex-1" />
          <span className="text-caption text-fg-faint tabular">
            {agentActive(agent) ? agentElapsed(agent, now) : AGENT_STATUS_LABEL[agent.status]}
          </span>
        </span>
        <span className="line-clamp-2 text-callout text-fg-muted">{agent.task}</span>
      </span>
    </button>
  );
}

function AgentConversation({ threadId, agent }: { threadId: string; agent: SubagentInfo }) {
  const [history, setHistory] = useState<ThreadState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [text, setText] = useState("");
  const active = agentActive(agent);
  const now = useNow(active);

  // Reload while it works, and once more when it ends.
  useEffect(() => {
    let live = true;
    const load = () =>
      agentHistory(threadId, agent.id).then(
        (h) => live && setHistory(h),
        (e) => live && setError(e instanceof Error ? e.message : String(e)),
      );
    void load();
    if (!active) return;
    const t = setInterval(load, 2500);
    return () => {
      live = false;
      clearInterval(t);
    };
  }, [threadId, agent.id, active]);

  const totals = [
    agent.inputTokens + agent.outputTokens > 0
      ? `${compactCount(agent.inputTokens + agent.outputTokens)} tokens${agent.cachedInputTokens ? ` (${compactCount(agent.cachedInputTokens)} cached)` : ""}`
      : undefined,
    agent.toolCalls ? `${agent.toolCalls} tool call${agent.toolCalls === 1 ? "" : "s"}` : undefined,
    agent.requests ? `${agent.requests} request${agent.requests === 1 ? "" : "s"}` : undefined,
  ].filter(Boolean);

  const send = () => {
    const t = text.trim();
    if (!t) return;
    setText("");
    void sendToAgent(threadId, agent.id, t);
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="border-border border-b px-3 py-2.5">
        <div className="flex items-center gap-2">
          <AgentMark agent={agent} />
          <span className="font-semibold text-body">{agent.name}</span>
          <span className="text-caption text-fg-muted">
            {[
              agent.agentType !== "default" ? agent.agentType : undefined,
              agent.model,
              active ? agentElapsed(agent, now) : AGENT_STATUS_LABEL[agent.status],
            ]
              .filter(Boolean)
              .join(" · ")}
          </span>
          <span className="flex-1" />
          {active && (
            <Tooltip label="Stop this agent and the agents it started">
              <button
                type="button"
                onClick={() => void cancelAgent(threadId, agent.id)}
                className="flex h-[24px] items-center gap-1 rounded-full bg-danger/12 px-2.5 font-medium text-caption text-danger hover:bg-danger/18"
              >
                <Square size={8} fill="currentColor" strokeWidth={0} /> Stop
              </button>
            </Tooltip>
          )}
        </div>
        <p className="selectable mt-1.5 line-clamp-4 text-callout text-fg-muted">{agent.task}</p>
        {(totals.length > 0 || agent.workspacePath) && (
          <p className="mt-1 text-caption text-fg-faint">
            {[...totals, agent.workspaceMode === "isolated" ? "own workspace" : undefined]
              .filter(Boolean)
              .join(" · ")}
          </p>
        )}
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-3 py-3">
        {error && <p className="text-callout text-fg-muted">{error}</p>}
        {!history && !error && (
          <div className="flex items-center gap-2 text-callout text-fg-muted">
            <Spinner /> Loading the conversation…
          </div>
        )}
        <div className="flex flex-col gap-3">
          {history?.items.map((item) =>
            item.kind === "user" ? (
              <UserRow key={item.id} item={item} />
            ) : item.kind === "assistant" ? (
              <AssistantRow key={item.id} item={item} />
            ) : item.kind === "thought" ? (
              <ThoughtRow key={item.id} item={item} live={false} />
            ) : item.kind === "tool" ? (
              <ToolRow key={item.id} item={item} cwd={history.cwd} />
            ) : null,
          )}
        </div>
        {agent.completion && !active && (
          <div className="mt-3 rounded-md bg-fill-subtle p-2.5">
            <div className="mb-1 font-semibold text-caption text-fg-muted">Returned</div>
            <Markdown text={agent.completion} className="text-callout" />
          </div>
        )}
      </div>
      <div className="border-border border-t p-2.5">
        <div className="elevated flex items-end gap-2 rounded-xl p-2">
          <textarea
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                send();
              }
            }}
            rows={1}
            placeholder={active ? "Tell it more; it reads it at its next step…" : "Send it a follow-up…"}
            aria-label={`Message ${agent.name}`}
            className="max-h-[120px] min-h-[22px] flex-1 resize-none bg-transparent px-1.5 text-body outline-none placeholder:text-fg-faint"
          />
          <button
            type="button"
            aria-label="Send to agent"
            disabled={!text.trim()}
            onClick={send}
            className="flex size-[26px] items-center justify-center rounded-full bg-fg text-bg disabled:bg-fg-muted/25"
          >
            <ArrowUp size={13} strokeWidth={2.75} />
          </button>
        </div>
      </div>
    </div>
  );
}

export function AgentsPanel() {
  const threadId = useApp((s) => s.selectedId);
  const agents = useApp((s) => (s.selectedId ? (s.agents[s.selectedId] ?? EMPTY) : EMPTY));
  const openId = useApp((s) => s.agentOpen);
  const open = agents.find((a) => a.id === openId);
  const anyActive = agents.some(agentActive);
  const now = useNow(anyActive);
  const sorted = useMemo(
    () => [...agents].sort((a, b) => Number(agentActive(b)) - Number(agentActive(a)) || a.id - b.id),
    [agents],
  );

  useEffect(() => {
    if (threadId) void refreshAgents(threadId);
  }, [threadId]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 items-center gap-2 px-3">
        {open ? (
          <button
            type="button"
            onClick={() => openAgent(null)}
            className="-ml-1 flex items-center gap-1 rounded-sm px-1 font-semibold text-body hover:bg-fill-subtle"
          >
            <ChevronLeft size={14} /> Agents
          </button>
        ) : (
          <span className="flex items-center gap-1.5 font-semibold text-body">
            <Users size={14} /> Agents
          </span>
        )}
        <span className="text-caption text-fg-muted">
          {agents.length ? `${agents.filter(agentActive).length} working · ${agents.length} in all` : ""}
        </span>
        <span className="flex-1" />
      </div>
      {open && threadId ? (
        <AgentConversation threadId={threadId} agent={open} />
      ) : (
        <div className="min-h-0 flex-1 overflow-y-auto p-2">
          {!sorted.length && (
            <p className="px-2.5 py-2 text-callout text-fg-muted">
              No agents yet. When the agent splits work, each agent it starts shows here and in the chat.
            </p>
          )}
          {sorted.map((agent) => (
            <AgentListRow key={agent.id} agent={agent} now={now} />
          ))}
        </div>
      )}
    </div>
  );
}
