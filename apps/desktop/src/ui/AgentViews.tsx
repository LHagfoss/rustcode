// Subagents, one way for every harness, as T3 Code draws them: a row per
// agent a turn started, where it started it, that says what it does while it
// works and how long it took once done; and a pill over the composer while
// agents work. RustCode's own agents (`spawn_agent`) take the same row, fed
// by `_rustcode/agents`.

import { ChevronRight, CircleCheck, CircleX } from "lucide-react";
import { HoverCard } from "radix-ui";
import { useEffect, useMemo, useState } from "react";
import { agentActive, agentElapsed, type SubagentInfo, spawnedAgentId } from "../acp/rustcode";
import { agentHistory, useApp } from "../state/store";
import { formatDuration, stepLabel, subagentOf, type TranscriptItem } from "../state/thread";
import { Markdown } from "./Markdown";
import { cx, Orb, Spinner } from "./primitives";

const EMPTY: SubagentInfo[] = [];
const NO_STEPS: ToolItem[] = [];

function useNow(active: boolean) {
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
  useEffect(() => {
    if (agentId === undefined || !(active || open)) return;
    let alive = true;
    const load = () =>
      agentHistory(threadId, agentId).then(
        (h) => alive && setSteps(h.items.filter((i): i is ToolItem => i.kind === "tool")),
        () => undefined,
      );
    void load();
    const t = active ? setInterval(load, 2500) : undefined;
    return () => {
      alive = false;
      if (t) clearInterval(t);
    };
  }, [threadId, agentId, active, open]);
  return steps;
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
  const steps = agentId === undefined ? inline : fetched;
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
  return (
    <div className="font-rounded">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className={cx(
          "group flex h-7 w-fit max-w-full items-center gap-2 rounded-full border px-2.5 text-left transition-colors duration-200",
          running
            ? "border-accent/25 bg-accent/[0.06]"
            : "border-border bg-fill-subtle/60 hover:bg-fill-subtle",
        )}
      >
        <span className="flex w-3.5 shrink-0 justify-center">
          {running ? (
            <Orb />
          ) : failed ? (
            <CircleX size={13} className="text-danger" />
          ) : (
            <CircleCheck size={13} className="text-success rc-dot-in" />
          )}
        </span>
        <span className="shrink-0 font-medium text-callout">{name}</span>
        <span className="min-w-0 shrink truncate text-callout text-fg-muted">{agent.task}</span>
        {running ? (
          <span className="min-w-0 max-w-[40%] shrink truncate text-caption">
            <span className="rc-shimmer">{current ? stepLabel(current) : "Starting"}</span>
          </span>
        ) : (
          done && <span className="shrink-0 text-caption text-fg-faint tabular">{done}</span>
        )}
        {(steps.length > 0 || output || agentId !== undefined) && (
          <ChevronRight
            size={11}
            className={cx("shrink-0 text-fg-faint transition-transform duration-200", open && "rotate-90")}
          />
        )}
      </button>
      {open && (
        <div className="rc-expand mt-1.5 ml-3 flex flex-col gap-0.5 border-border border-l pl-3">
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

/** What one working agent is doing, for the pill's card. */
interface Working {
  key: string;
  name: string;
  task: string;
  step?: string;
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
    const out: Working[] = agents
      .filter(agentActive)
      .map((a) => ({ key: `rc-${a.id}`, name: a.name, task: a.task.split("\n")[0] ?? "" }));
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
      out.push({ key: item.id, name: agent.name, task: agent.task, step: step && stepLabel(step) });
    }
    return out;
  }, [agents, thread?.items, thread?.running]);
  if (!working.length) return null;
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
          className="elevated z-40 flex w-[300px] flex-col gap-0.5 rounded-xl p-1.5 rc-in"
        >
          {working.map((w) => (
            <div key={w.key} className="flex items-start gap-2 rounded-lg px-2 py-1.5">
              <span className="mt-[5px] flex w-3.5 justify-center">
                <Orb />
              </span>
              <span className="min-w-0 flex-1">
                <span className="flex items-baseline gap-1.5">
                  <span className="shrink-0 font-medium text-callout">{w.name}</span>
                  <span className="min-w-0 truncate text-callout text-fg-muted">{w.task}</span>
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
