// The overlapping marks a parent shows for the threads it started, in place of
// child rows: at most three icons, then `+N`, what needs you first, and one
// quiet sign that work is under way. The same stack in the sidebar's rows and in
// the chat's group cards, at two sizes (state/agentStack.ts).

import { Bot, Check, X } from "lucide-react";
import { agentStack, type StackAgent } from "../state/agentStack";
import { useApp } from "../state/store";
import { HarnessIcon } from "./HarnessIcon";
import { cx, Spinner, Tooltip } from "./primitives";

const STATE_WORD = {
  needs: "needs you",
  working: "working",
  done: "done",
  failed: "failed",
} as const;

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
  const harnesses = useApp((s) => s.harnesses?.harnesses);
  // A group card keeps its finished delegates once they all are; a sidebar row
  // shows nothing then.
  const view = agentStack(agents, { idle: size === "chat" ? "show" : "hide" });
  if (!view.order.length) return null;
  const chat = size === "chat";
  const ring = tone === "sidebar" ? "ring-sidebar" : "ring-surface";
  const icon = chat ? 24 : 18;
  const tip = (
    <div className="flex flex-col gap-1">
      <span className="font-medium">{view.label}</span>
      {view.order.map((agent) => (
        <span key={agent.id} className="flex items-center gap-1.5">
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
            {STATE_WORD[agent.state]}
          </span>
          <span className="min-w-0 truncate text-fg-muted">{agent.label ?? "Thread"}</span>
        </span>
      ))}
    </div>
  );
  return (
    <Tooltip label={tip}>
      <span
        role="img"
        aria-label={`Delegated threads: ${view.label}`}
        className={cx(
          "flex shrink-0 items-center justify-end",
          // A stable slot: a count going from one to three never moves the title
          // beside it (`docs/design.md`, sidebar rows).
          !chat && "w-[74px]",
          className,
        )}
      >
        <span className="flex items-center">
          {view.visible.map((agent, i) => (
            <span
              key={agent.id}
              className={cx(
                "relative flex shrink-0 items-center justify-center rounded-full bg-fill ring-2",
                // What needs you keeps a warning ring: the one mark that has to be seen at a glance.
                agent.state === "needs" ? "ring-warning" : ring,
                i > 0 && (chat ? "-ml-2" : "-ml-1.5"),
              )}
              style={{ width: icon, height: icon }}
            >
              {agent.harness ? (
                <HarnessIcon
                  id={agent.harness}
                  name={harnesses?.find((h) => h.id === agent.harness)?.name}
                  size={chat ? 13 : 11}
                />
              ) : (
                <Bot size={chat ? 13 : 10} className="text-fg-muted" />
              )}
              {chat && (agent.state === "done" || agent.state === "failed") && (
                // The group card keeps the avatar's own badge once its agents are done.
                <span
                  aria-hidden
                  className={cx(
                    "-right-1 -bottom-1 absolute flex size-3 items-center justify-center rounded-full bg-bg ring-2",
                    ring,
                  )}
                >
                  {agent.state === "failed" ? (
                    <X size={9} strokeWidth={3} className="text-danger" />
                  ) : (
                    <Check size={9} strokeWidth={3} className="text-success" />
                  )}
                </span>
              )}
            </span>
          ))}
          {view.extra > 0 && (
            <span
              className={cx("ml-1 shrink-0 text-fg-muted tabular", chat ? "text-caption" : "text-[10px]")}
            >
              +{view.extra}
            </span>
          )}
          {/* One sign for the stack, not a spinner per icon. */}
          {view.working > 0 && <Spinner size={chat ? 10 : 9} className="ml-1 text-accent" />}
        </span>
      </span>
    </Tooltip>
  );
}
