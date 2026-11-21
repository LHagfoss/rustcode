// Transcript divider for harness handoffs and machine moves. Split out of ui/Rows.tsx.

import { useApp } from "../state/store";
import type { TranscriptItem } from "../state/thread";
import { HarnessIcon } from "./HarnessIcon";
import { ArrowRight } from "./icons";
import { Tooltip } from "./primitives";

type HandoffItem = Extract<TranscriptItem, { kind: "handoff" }>;

/**
 * Where the thread moved to another harness or another machine: a quiet divider,
 * `Claude Code → Codex` or `MacBook → VPS`, with the model it took.
 */
export function HandoffRow({ item }: { item: HandoffItem }) {
  const harnesses = useApp((s) => s.harnesses?.harnesses);
  const name = (id: string) => harnesses?.find((h) => h.id === id)?.name ?? id;
  const movedMachine = !!(item.fromMachine && item.toMachine);
  const harnessChanged = !!item.from && !!item.to && item.from !== item.to;
  return (
    <div className="flex items-center gap-3 py-1 text-caption text-fg-muted" role="separator">
      <span className="h-px flex-1 bg-border" />
      <span className="flex items-center gap-1.5">
        {movedMachine ? (
          <>
            <span>{item.fromMachine}</span>
            {harnessChanged && (
              <span className="flex items-center gap-1 text-fg-faint">
                <HarnessIcon id={item.from} size={12} />
                {name(item.from)}
              </span>
            )}
            <ArrowRight size={12} className="text-fg-faint" />
            <span className="text-fg">{item.toMachine}</span>
            {harnessChanged && (
              <span className="flex items-center gap-1 text-fg">
                <HarnessIcon id={item.to} size={12} />
                {name(item.to)}
              </span>
            )}
            {item.model && <span className="text-fg-faint">· {item.model}</span>}
          </>
        ) : (
          <>
            <HarnessIcon id={item.from} size={12} />
            {name(item.from)}
            <ArrowRight size={12} className="text-fg-faint" />
            <HarnessIcon id={item.to} size={12} />
            <span className="text-fg">{name(item.to)}</span>
            {item.model && <span className="text-fg-faint">· {item.model}</span>}
          </>
        )}
      </span>
      <Tooltip
        label={
          item.resumedNative
            ? "Resumed the native session on this machine"
            : movedMachine
              ? "Moved between machines · the conversation so far carries over with your next message"
              : "The new harness gets the conversation so far with your next message"
        }
      >
        <span className="text-fg-faint">
          {item.resumedNative ? "resumed session" : "context handed over"}
        </span>
      </Tooltip>
      <span className="h-px flex-1 bg-border" />
    </div>
  );
}
