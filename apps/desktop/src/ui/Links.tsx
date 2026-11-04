// Threads working for threads, in the chat: agent rows above the composer for
// the threads this one started or messaged (what it is doing, open, stop), a
// "Working for" banner in a thread doing work for another, and the prompt that
// asks the user before a thread reaches another one.

import {
  ChevronDown,
  CircleCheck,
  CircleX,
  Clock,
  CornerDownRight,
  GitBranch,
  Hand,
  Link2,
  LoaderCircle,
  PauseCircle,
  Rows3,
  X,
} from "lucide-react";
import { Dialog, Popover } from "radix-ui";
import { useEffect } from "react";
import { dismissLink, linksOf, resolveGrant, stateOf, stopLink, verb } from "../state/host";
import { isActive, LINK_LABEL, type Link, type LinkState } from "../state/orchestration";
import { selectThread, setView, useApp } from "../state/store";
import { ThreadAgentRow } from "./AgentViews";
import { cx, Pill, Tooltip } from "./primitives";

const ICON: Record<LinkState, typeof Clock> = {
  queued: Clock,
  working: LoaderCircle,
  needs_user: Hand,
  done: CircleCheck,
  failed: CircleX,
  cancelled: PauseCircle,
  interrupted: PauseCircle,
};

const TINT: Partial<Record<LinkState, string>> = {
  needs_user: "text-warning",
  done: "text-success",
  failed: "text-danger",
  working: "text-accent",
};

/** A link's state as a small icon: the chips, the popover and a tool row all read it the same way. */
export function LinkStateIcon({ state, size = 12 }: { state: LinkState; size?: number }) {
  const Icon = ICON[state];
  return (
    <Icon size={size} className={cx(TINT[state] ?? "text-fg-muted", state === "working" && "animate-spin")} />
  );
}

/**
 * One thread this one delegated to, in the strip above the composer and in its
 * popover: the same row as a subagent, plus stop (or remove once settled).
 */
function LinkRow({ link }: { link: Link }) {
  const state = useApp((s) => stateOf(s, link));
  const active = isActive(state);
  return (
    <ThreadAgentRow
      link={link}
      action={
        <Tooltip label={active ? "Stop this work in that thread" : "Remove"}>
          <button
            type="button"
            aria-label={active ? `Stop ${link.targetTitle}` : `Remove ${link.targetTitle}`}
            onClick={() => (active ? void stopLink(link.id) : dismissLink(link.id))}
            className="flex size-6 shrink-0 items-center justify-center rounded-sm text-fg-faint hover:bg-fill hover:text-fg"
          >
            <X size={12} strokeWidth={2.5} />
          </button>
        </Tooltip>
      }
    />
  );
}

/** The counts the collapsed chip shows, from the links' states. */
const stateSummary = (states: LinkState[]) => ({
  working: states.filter((s) => s === "working").length,
  needs: states.filter((s) => s === "needs_user").length,
});

/**
 * Three or more threads working for this one fold into one row: the strip above
 * the composer has about 800px, and a third of it spent on a made-up title tells
 * the reader nothing. Two stay as rows of their own; they fit.
 */
function ManyLinks({ links }: { links: Link[] }) {
  // Joined into one string so the selector's result is stable between renders.
  const states = useApp((s) => links.map((l) => stateOf(s, l)).join(",")).split(",") as LinkState[];
  const { working, needs } = stateSummary(states);
  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <button
          type="button"
          className="group flex w-full min-w-0 items-center gap-2.5 rounded-lg px-2 py-1.5 text-left hover:bg-fill-subtle data-[state=open]:bg-fill-subtle"
        >
          <span className="relative flex size-6 shrink-0 items-center justify-center rounded-full bg-fill">
            <Link2 size={13} className="text-fg-muted" />
          </span>
          <span className="flex min-w-0 flex-1 flex-col">
            <span className="min-w-0 truncate font-medium text-fg text-sub">{links.length} threads</span>
            <span className="block truncate text-caption text-fg-muted">
              {[working > 0 ? `${working} working` : undefined, needs > 0 ? `${needs} need you` : undefined]
                .filter(Boolean)
                .join(" · ") || "done"}
            </span>
          </span>
          <ChevronDown size={14} aria-hidden className="shrink-0 text-fg-faint group-hover:text-fg" />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          side="top"
          align="start"
          sideOffset={8}
          aria-label="Linked threads"
          className="elevated z-50 flex max-h-[min(420px,60vh)] w-[340px] flex-col overflow-y-auto p-1.5 rc-in"
        >
          {links.map((l) => (
            <LinkRow key={l.id} link={l} />
          ))}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

/** Above the composer: the threads this one is working with, and the thread it works for. */
export function LinkedThreads({ threadId }: { threadId: string }) {
  const book = useApp((s) => s.links);
  const { driving, drivenBy } = linksOf(book, threadId);
  const fanOuts = book.fanOuts.filter((f) => f.orchestrator === threadId).slice(-1);
  if (!driving.length && !drivenBy) return null;
  return (
    <div className="flex flex-col gap-1 rounded-xl bg-bg py-0.5">
      {drivenBy && <WorkingFor link={drivenBy} />}
      {driving.length > 0 && (
        <div className="flex flex-col gap-0.5">
          {driving.length > 2 ? (
            <ManyLinks links={driving} />
          ) : (
            driving.map((l) => <LinkRow key={l.id} link={l} />)
          )}
          {fanOuts.length > 0 && (
            <div className="flex flex-wrap gap-1.5 px-2 pt-0.5">
              {fanOuts.map((f) => (
                <Pill
                  key={f.id}
                  onClick={() => setView({ kind: "compare", fanOut: f.id })}
                  className="shrink-0"
                >
                  <Rows3 size={12} /> Compare
                </Pill>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function WorkingFor({ link }: { link: Link }) {
  const state = useApp((s) => stateOf(s, link));
  return (
    <div className="elevated flex h-[32px] items-center gap-2 rounded-full pr-1.5 pl-3 text-callout rc-in">
      <CornerDownRight size={13} className="shrink-0 text-accent" />
      <span className="min-w-0 truncate">Working for “{link.orchestratorTitle}”</span>
      <span className="shrink-0 text-fg-muted">· {LINK_LABEL[state]}</span>
      <span className="flex-1" />
      <Pill onClick={() => selectThread(link.orchestrator)}>Open</Pill>
      <Tooltip label="Stop the work that thread gave this one; your own messages are not affected">
        <Pill onClick={() => void stopLink(link.id)}>Stop</Pill>
      </Tooltip>
    </div>
  );
}

/** A thread asking whether it may reach another one: Allow for This Thread, Allow Once, Don't Allow. */
export function GrantDialog() {
  const grant = useApp((s) => s.grant);
  useEffect(() => {
    if (!grant) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") resolveGrant("deny");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [grant]);
  if (!grant) return null;
  const starts = grant.kind === "start" || grant.kind === "fan_out";
  return (
    <Dialog.Root open onOpenChange={(open) => !open && resolveGrant("deny")}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <Dialog.Content
          aria-describedby={undefined}
          className="elevated fixed top-1/2 left-1/2 z-50 flex w-[min(460px,calc(100vw-48px))] -translate-x-1/2 -translate-y-1/2 flex-col gap-3 p-5"
        >
          <Dialog.Title className="font-semibold text-body">
            “{grant.callerTitle}” wants to {verb(grant)}
          </Dialog.Title>
          <div className="flex flex-col gap-1.5 text-callout text-fg-muted">
            {starts && (
              <span>
                A new thread{grant.harnesses?.length ? ` on ${grant.harnesses.join(", ")}` : ""}
                {grant.worktree ? ", each in its own worktree," : ""} gets this as its first message. It runs
                with its own approvals and sandbox.
              </span>
            )}
            {grant.kind === "message" && (
              <span>The message arrives in that thread as yours would, headed with where it is from.</span>
            )}
            {grant.project && (
              <span className="flex items-center gap-1.5 text-warning">
                <GitBranch size={12} /> In another project: {grant.project}
              </span>
            )}
          </div>
          <pre className="selectable m-0 max-h-[180px] overflow-auto rounded-lg bg-fill-subtle p-3 font-sans text-callout whitespace-pre-wrap">
            {grant.detail}
          </pre>
          <div className="flex flex-wrap justify-end gap-2">
            <Pill onClick={() => resolveGrant("deny")}>Don’t Allow</Pill>
            <Pill onClick={() => resolveGrant("once")}>Allow Once</Pill>
            <Pill prominent autoFocus onClick={() => resolveGrant("always")}>
              Allow for This Thread
            </Pill>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * A sidebar badge: this thread works for another (↳), or has links under way
 * (count). `childrenListed` says the sidebar already lists the threads this one
 * started; only those leave the count, since a message to a thread shown
 * elsewhere and the "working for" marker are still news.
 */
export function LinkBadge({ threadId, childrenListed }: { threadId: string; childrenListed?: boolean }) {
  const book = useApp((s) => s.links);
  const { driving, drivenBy } = linksOf(book, threadId);
  const active = driving.filter((l) => isActive(l.state) && !(childrenListed && l.started)).length;
  if (drivenBy) {
    return (
      <Tooltip label={`Working for “${drivenBy.orchestratorTitle}”`}>
        <CornerDownRight size={11} className="shrink-0 text-accent" aria-label="Working for another thread" />
      </Tooltip>
    );
  }
  if (!active) return null;
  return (
    <Tooltip label={`${active} linked thread${active === 1 ? "" : "s"} under way`}>
      <span className="flex shrink-0 items-center gap-0.5 text-caption text-accent tabular">
        <Link2 size={10} />
        {active}
      </span>
    </Tooltip>
  );
}
