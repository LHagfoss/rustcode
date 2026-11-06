// The overlapping marks a parent shows for the threads it started, in place of
// child rows: at most three circles, then `+N`, what needs you first, and one
// quiet sign that work is under way. The same stack in the sidebar's rows and in
// the chat's group cards, at two sizes (state/agentStack.ts).

import { Bot } from "lucide-react";
import { useEffect, useState } from "react";
import {
  type AgentStackView,
  agentStack,
  STACK_STATE_WORD,
  type StackAgent,
  stackDescription,
} from "../state/agentStack";
import { useApp } from "../state/store";
import { formatDuration } from "../state/thread";
import { HarnessIcon } from "./HarnessIcon";
import { cx, Spinner, Tooltip } from "./primitives";

/** The tip's clock: it runs only while the tip is open and something is working. */
function useTipNow(active: boolean) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    // Reopening the tip after the row was idle must not start from an old `now`.
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [active]);
  return now;
}

/**
 * What a stack's hover says: how many threads and subagents it holds, and for
 * each mark its state, its name, which of the two it is, and how long it has
 * run. A thread has its own thread to open; a subagent runs inside this one.
 */
function StackTip({ view }: { view: AgentStackView }) {
  const now = useTipNow(view.running);
  return (
    <div className="flex flex-col gap-1">
      <span className="flex items-baseline gap-1.5">
        <span className="font-medium">{view.label}</span>
        {view.summary && <span className="text-fg-muted">· {view.summary}</span>}
      </span>
      {view.order.map((agent) => {
        const active = agent.state === "working" || agent.state === "needs";
        const elapsed =
          active && agent.since !== undefined
            ? formatDuration(Math.floor((now - agent.since) / 1000) * 1000)
            : undefined;
        return (
          <span key={agent.id} className="flex min-w-0 items-center gap-1.5">
            <span
              className={cx(
                "shrink-0",
                agent.state === "needs"
                  ? "text-warning"
                  : agent.state === "working"
                    ? "text-accent"
                    : "text-fg-muted",
              )}
            >
              {STACK_STATE_WORD[agent.state]}
            </span>
            <span className="line-clamp-2 min-w-0 break-words text-fg-muted">{agent.label ?? "Thread"}</span>
            {agent.kind && <span className="shrink-0 text-fg-faint">{agent.kind}</span>}
            {elapsed && <span className="ml-auto shrink-0 text-fg-faint tabular">{elapsed}</span>}
          </span>
        );
      })}
    </div>
  );
}

/** The circle every mark draws in: one size in a sidebar row, one on a group card. */
const MARK_SIZE = { sidebar: 16, chat: 20 } as const;
/** The glyph fills this much of the circle, so a π is legible at 16px. */
const GLYPH = 0.6;
/** Icons overlap by this much of the diameter; each one's shadow on the one under it keeps them apart. */
const OVERLAP = 0.3;
/** The one badge a stack draws, on the last of its icons. */
const BADGE = 8;

/**
 * The icons alone, for a trigger that has its own card: at most three, `+N`, one
 * sign of work.
 *
 * Each icon is one solid circle, a step off the surface, with a soft shadow on
 * its leading edge, so overlapping marks read as separate without a border,
 * and the state is ONE badge on the stack rather than a badge per icon: what
 * needs you, else what failed. A stack whose marks all finished shows none of
 * it; the text beside it already says `done`.
 */
export function AgentMarks({
  agents,
  size = "sidebar",
  tone = "sidebar",
}: {
  agents: readonly StackAgent[];
  size?: "sidebar" | "chat";
  tone?: "sidebar" | "surface";
}) {
  const harnesses = useApp((s) => s.harnesses?.harnesses);
  const view = agentStack(agents, { idle: size === "chat" ? "show" : "hide" });
  const chat = size === "chat";
  // The badge's ring takes the colour of what the stack sits on, so the dot stands off its icon.
  const ring = tone === "sidebar" ? "ring-sidebar" : "ring-surface";
  const icon = MARK_SIZE[size];
  const glyph = Math.round(icon * GLYPH);
  const overlap = Math.round(icon * OVERLAP);
  // One badge for the stack as a whole, on its last icon.
  const badge = view.state === "needs" ? "bg-warning" : view.state === "failed" ? "bg-danger" : undefined;
  const last = view.visible.at(-1)?.id;
  return (
    <span className="flex items-center">
      {view.visible.map((agent, i) => (
        <span
          key={agent.id}
          className={cx(
            // Opaque, so the mark under it never shows through; no ring (index.css `.rc-agent-mark`).
            "rc-agent-mark relative flex shrink-0 items-center justify-center rounded-full text-fg-muted",
          )}
          style={{
            width: icon,
            height: icon,
            marginLeft: i > 0 ? -overlap : undefined,
          }}
        >
          {agent.harness ? (
            <HarnessIcon
              id={agent.harness}
              name={harnesses?.find((h) => h.id === agent.harness)?.name}
              size={glyph}
              plain
            />
          ) : (
            <Bot size={glyph} className="text-fg-muted" />
          )}
          {badge && agent.id === last && (
            <span
              aria-hidden
              className={cx("-right-px -bottom-px absolute rounded-full ring-[1.5px] rc-dot-in", ring, badge)}
              style={{ width: BADGE, height: BADGE }}
            />
          )}
        </span>
      ))}
      {view.extra > 0 && (
        <span
          className={cx("shrink-0 text-fg-muted tabular", chat ? "ml-1 text-caption" : "ml-0.5 text-[10px]")}
        >
          +{view.extra}
        </span>
      )}
      {/* One sign for the stack, not a spinner per icon. */}
      {view.working > 0 && <Spinner size={chat ? 10 : 9} className="ml-1 text-accent" />}
    </span>
  );
}

export function AgentStack({
  agents,
  size = "sidebar",
  tone = "sidebar",
  className,
}: {
  agents: readonly StackAgent[];
  /** `sidebar` in a thread row, `chat` on a group card's header. */
  size?: "sidebar" | "chat";
  /** The surface the icons overlap on, for their separating ring. */
  tone?: "sidebar" | "surface";
  className?: string;
}) {
  // A group card keeps its finished delegates once they all are; a sidebar row
  // shows nothing then.
  const view = agentStack(agents, { idle: size === "chat" ? "show" : "hide" });
  if (!view.order.length) return null;
  const chat = size === "chat";
  return (
    <Tooltip label={<StackTip view={view} />}>
      <span
        role="img"
        aria-label={stackDescription(view)}
        className={cx(
          "flex shrink-0 items-center justify-end",
          // A stable slot: a count going from one to three never moves the title
          // beside it (`docs/design.md`, sidebar rows).
          !chat && "w-[74px]",
          className,
        )}
      >
        <AgentMarks agents={agents} size={size} tone={tone} />
      </span>
    </Tooltip>
  );
}
