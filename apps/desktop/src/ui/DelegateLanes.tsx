// The Delegates section's lanes: one row per agent and model, a card per delegate in the order the
// work was handed out, at any depth (state/delegates.ts `delegateLanes`). It reads a fan-out or a
// review loop as what ran where, side by side, which the tree's nesting does not show. A card opens
// its thread.

import { useMemo } from "react";
import { modelName } from "../acp/types";
import { useNow } from "../lib/useNow";
import { delegateLanes, delegateTree } from "../state/delegates";
import { isActive } from "../state/orchestration";
import { selectThread, useApp } from "../state/store";
import { formatDuration } from "../state/thread";
import { cx } from "./primitives";

const STATE_WORD: Record<string, string> = {
  queued: "Queued",
  working: "Working",
  needs_user: "Needs you",
  done: "Done",
  failed: "Failed",
  cancelled: "Stopped",
  interrupted: "Interrupted",
};

export function DelegateLanes({ threadId }: { threadId: string }) {
  const links = useApp((s) => s.links.links);
  const live = useApp((s) => s.links.links.some((l) => l.orchestrator === threadId && isActive(l.state)));
  const now = useNow(live);
  const lanes = useMemo(() => delegateLanes(delegateTree(threadId, links), now), [threadId, links, now]);
  const columns = lanes.reduce((max, lane) => Math.max(max, ...lane.cards.map((c) => c.column + 1)), 0);
  if (!lanes.length) return null;
  return (
    <div className="overflow-x-auto px-2 pb-1">
      <div
        className="grid items-center gap-x-1.5"
        style={{ gridTemplateColumns: `minmax(72px, auto) repeat(${columns}, 128px)` }}
      >
        {lanes.map((lane) => (
          <div key={lane.key} className="contents">
            <span className="flex min-w-0 flex-col py-2 pr-2 text-caption">
              <span className="truncate text-fg-muted">{lane.harness}</span>
              {lane.model && <span className="truncate text-fg-faint">{modelName(lane.model)}</span>}
            </span>
            {Array.from({ length: columns }, (_, column) => {
              const card = lane.cards.find((c) => c.column === column);
              if (!card) return <span key={column} className="border-border/60 border-b border-dashed" />;
              const live = isActive(card.state);
              return (
                <div
                  key={card.id}
                  className={cx(
                    "relative my-1 flex min-w-0 flex-col gap-0.5 rounded-lg bg-fill-subtle px-2 py-1.5 hover:bg-fill",
                    live && "ring-1 ring-accent/40",
                  )}
                >
                  <span className="flex items-baseline justify-between gap-1 text-caption text-fg-faint tabular">
                    <span>{String(card.n).padStart(2, "0")}</span>
                    {card.ms !== undefined && <span>{formatDuration(Math.max(0, card.ms))}</span>}
                  </span>
                  {/* The title is the card's control; its ::after stretches the press over the card. */}
                  <button
                    type="button"
                    onClick={() => selectThread(card.id)}
                    aria-label={`Open ${card.title}`}
                    className="truncate text-left text-callout text-fg after:absolute after:inset-0 after:rounded-lg"
                  >
                    {card.title}
                  </button>
                  <span
                    className={cx(
                      "text-caption",
                      card.state === "failed" ? "text-danger" : live ? "text-accent" : "text-fg-muted",
                      card.state === "needs_user" && "text-warning",
                    )}
                  >
                    {STATE_WORD[card.state] ?? card.state}
                  </span>
                </div>
              );
            })}
          </div>
        ))}
      </div>
    </div>
  );
}
