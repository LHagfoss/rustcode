// Threads working for threads, in the chat: chips above the composer for
// the threads this one started or messaged (state, open, stop), a "Working
// for" banner in a thread doing work for another, and the prompt that asks
// the user before a thread reaches another one.

import {
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
import { Dialog } from "radix-ui";
import { useEffect } from "react";
import { dismissLink, linksOf, resolveGrant, stateOf, stopLink, verb } from "../state/host";
import { isActive, LINK_LABEL, type Link, type LinkState } from "../state/orchestration";
import { selectThread, setView, useApp } from "../state/store";
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

function LinkChipRow({ link }: { link: Link }) {
  const state = useApp((s) => stateOf(s, link));
  const Icon = ICON[state];
  const active = isActive(state);
  return (
    <span className="elevated flex h-[28px] shrink-0 items-center gap-1.5 rounded-full pr-1.5 pl-2.5 text-callout rc-in">
      <Icon size={12} className={cx(TINT[state] ?? "text-fg-muted", state === "working" && "animate-spin")} />
      <Tooltip label={link.message.length > 300 ? `${link.message.slice(0, 300)}…` : link.message}>
        <button
          type="button"
          onClick={() => selectThread(link.target)}
          className="flex min-w-0 items-center gap-1 rounded-full hover:text-accent"
        >
          <span className="max-w-[200px] truncate">{link.targetTitle}</span>
          <span className="text-fg-muted">· {LINK_LABEL[state]}</span>
        </button>
      </Tooltip>
      <Tooltip label={active ? "Stop this work in that thread" : "Remove"}>
        <button
          type="button"
          aria-label={active ? `Stop ${link.targetTitle}` : `Remove ${link.targetTitle}`}
          onClick={() => (active ? void stopLink(link.id) : dismissLink(link.id))}
          className="flex size-5 items-center justify-center rounded-full text-fg-faint hover:bg-fill hover:text-fg"
        >
          <X size={11} strokeWidth={2.5} />
        </button>
      </Tooltip>
    </span>
  );
}

/** Above the composer: the threads this one is working with, and the thread it works for. */
export function LinkedThreads({ threadId }: { threadId: string }) {
  const book = useApp((s) => s.links);
  const { driving, drivenBy } = linksOf(book, threadId);
  const fanOuts = book.fanOuts.filter((f) => f.orchestrator === threadId).slice(-1);
  if (!driving.length && !drivenBy) return null;
  return (
    <div className="flex flex-col gap-1.5">
      {drivenBy && <WorkingFor link={drivenBy} />}
      {driving.length > 0 && (
        <div className="flex items-center gap-1.5 overflow-x-auto pb-0.5">
          <Link2 size={12} className="ml-1 shrink-0 text-fg-faint" />
          {driving.map((l) => (
            <LinkChipRow key={l.id} link={l} />
          ))}
          {fanOuts.map((f) => (
            <Pill key={f.id} onClick={() => setView({ kind: "compare", fanOut: f.id })} className="shrink-0">
              <Rows3 size={12} /> Compare
            </Pill>
          ))}
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

/** A sidebar badge: this thread works for another (↳), or has links under way (count). */
export function LinkBadge({ threadId }: { threadId: string }) {
  const book = useApp((s) => s.links);
  const { driving, drivenBy } = linksOf(book, threadId);
  const active = driving.filter((l) => isActive(l.state)).length;
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
