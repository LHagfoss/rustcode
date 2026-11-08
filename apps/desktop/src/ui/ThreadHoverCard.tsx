// What a thread row shows a moment after the pointer rests on it: the same thread
// with room to say more — its title, where it runs, the harness and model it runs
// on, what it is doing, when it last moved, the branch it works in and the last
// thing said in it (ui/Sidebar.tsx). A preview, not a control: it takes no
// pointer, holds no focus and closes on the pointer leaving, a scroll, Escape, or
// the moment its row is dragged (ui/sidebarDrag.tsx).

import { Folder, GitBranch, MessageSquare } from "lucide-react";
import { HoverCard } from "radix-ui";
import { type ReactNode, useEffect, useMemo, useState } from "react";
import { displayPath, type SessionSummary } from "../acp/types";
import { lastWords, messageTime, type TranscriptItem } from "../state/thread";
import { ProjectMark } from "./ProjectMark";
import { cx } from "./primitives";

/** How long the pointer rests on a row before the card is worth showing. */
const HOVER_CARD_DELAY = 500;

/** One line of the card: a mark, then what it says. */
function Line({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <span className={cx("flex min-w-0 items-center gap-1.5 text-sub text-fg-muted", className)}>
      {children}
    </span>
  );
}

export function ThreadHoverCard({
  session,
  project,
  harness,
  model,
  status,
  now,
  items,
  disabled,
  children,
}: {
  session: SessionSummary;
  /** Where the row runs, as the row names it. */
  project: string;
  /** The harness's own name, when it is not RustCode's built-in one. */
  harness?: string;
  model?: string;
  /** What the row's own chip says (`Working`, `Needs you`…), or that it is idle. */
  status: string;
  /** The list's clock, so the card's age matches the rows'. */
  now: number;
  /** The thread's transcript, when this window has read it: the card quotes its last words. */
  items?: readonly TranscriptItem[];
  /** A row being dragged shows no card. */
  disabled?: boolean;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  // Closed as the drag starts; opened again only by a fresh hover after it.
  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);
  // A list that scrolls out from under the card takes it with it.
  useEffect(() => {
    if (!open) return;
    const close = () => setOpen(false);
    document.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    return () => {
      document.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
    };
  }, [open]);
  // The words are only worth reading out while the card is on screen.
  const words = useMemo(() => (open ? lastWords(items) : undefined), [open, items]);
  const branch = session.worktree?.branch;
  const said = [harness, model].filter(Boolean).join(" · ");
  return (
    <HoverCard.Root
      open={open && !disabled}
      openDelay={HOVER_CARD_DELAY}
      closeDelay={120}
      onOpenChange={setOpen}
    >
      <HoverCard.Trigger asChild>{children}</HoverCard.Trigger>
      <HoverCard.Portal>
        <HoverCard.Content
          side="right"
          align="start"
          sideOffset={6}
          collisionPadding={10}
          role="group"
          aria-label={`About ${session.title || "this thread"}`}
          // A preview: the pointer goes through it to whatever is behind, so it never
          // takes a press meant for the transcript beside the sidebar.
          className="elevated pointer-events-none z-40 flex w-[292px] flex-col gap-2 p-3 rc-in"
        >
          <span className="min-w-0 truncate font-medium text-body text-fg">
            {session.title || "New thread"}
          </span>
          <span className="flex min-w-0 flex-col gap-1">
            <Line>
              <ProjectMark name={project} />
              <span className="min-w-0 truncate">{project}</span>
            </Line>
            {said && <Line>{said}</Line>}
            <Line>
              <span className="size-1.5 shrink-0 rounded-full bg-current" aria-hidden />
              <span>{status}</span>
              <span className="text-fg-faint">·</span>
              <span className="truncate">{messageTime(session.updatedAt, now)}</span>
            </Line>
            {branch && (
              <Line>
                <GitBranch size={11} className="shrink-0" />
                <span className="min-w-0 truncate font-mono text-[10px]">{branch}</span>
              </Line>
            )}
            <Line>
              <Folder size={11} className="shrink-0" />
              <span className="min-w-0 truncate font-mono text-[10px]">{displayPath(session.cwd)}</span>
            </Line>
          </span>
          {words && (
            <span className="flex min-w-0 items-start gap-1.5 border-border border-t pt-2 text-fg-muted text-sub">
              <MessageSquare size={11} className="mt-[3px] shrink-0" />
              <span className="line-clamp-3 min-w-0">{words}</span>
            </span>
          )}
        </HoverCard.Content>
      </HoverCard.Portal>
    </HoverCard.Root>
  );
}
