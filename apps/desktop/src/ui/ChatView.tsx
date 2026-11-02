// The chat column: header (project / title, plan, usage, panels), the
// transcript, and the composer stack with error and approval cards. A new
// thread shows the centred empty state with the composer under it.

import {
  CircleCheck,
  CircleDashed,
  CircleDot,
  Eye,
  FileDiff,
  Folder,
  GitBranch,
  GitPullRequest,
  ListChecks,
  SquareTerminal,
  Users,
} from "lucide-react";
import { DropdownMenu, Popover } from "radix-ui";
import { useEffect, useRef, useState } from "react";
import { agentActive } from "../acp/rustcode";
import { useBanner } from "../lib/banner";
import { pickFolder } from "../lib/native";
import { observeSize } from "../lib/resize";
import {
  refreshSessions,
  restartEngine,
  setPanel,
  setProjectCwd,
  setTerminalOpen,
  setThreadStatus,
  setView,
  statusOf,
  useApp,
  worktreeOf,
} from "../state/store";
import type { ThreadState } from "../state/thread";
import { AttentionCard } from "./AttentionCards";
import { Composer } from "./Composer";
import { LinkedThreads } from "./Links";
import { MainHeader } from "./MainHeader";
import { NewThreadBanner } from "./NewThreadBanner";
import { Callout, Cluster, ClusterButton, cx, Pill, Spinner, Tooltip } from "./primitives";
import { StatusChip } from "./Status";
import { Transcript } from "./Transcript";

function PlanButton({ thread }: { thread: ThreadState }) {
  if (!thread.plan.length) return null;
  const done = thread.plan.filter((p) => p.status === "completed").length;
  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <button
          type="button"
          className="flex h-7 items-center gap-1.5 rounded-full bg-fill-subtle px-2.5 font-medium text-callout text-fg-muted hover:bg-fill data-[state=open]:bg-fill-strong"
        >
          <ListChecks size={13} />
          <span className="tabular">
            {done}/{thread.plan.length}
          </span>
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content align="end" sideOffset={6} className="elevated z-40 w-[380px] rounded-xl p-3 rc-in">
          <div className="mb-1 flex items-baseline justify-between px-1">
            <span className="font-semibold text-body">Plan</span>
            <span className="text-callout text-fg-muted">
              {done} of {thread.plan.length} done
            </span>
          </div>
          <p className="mb-2 px-1 text-caption text-fg-faint">As the agent marked each step.</p>
          <ol className="flex flex-col">
            {thread.plan.map((entry, i) => (
              <li key={i} className="flex items-start gap-2 rounded-md px-2 py-[7px]">
                {entry.status === "completed" ? (
                  <CircleCheck size={14} className="mt-px shrink-0 text-success" />
                ) : entry.status === "in_progress" ? (
                  <CircleDot size={14} className="mt-px shrink-0 text-accent" />
                ) : (
                  <CircleDashed size={14} className="mt-px shrink-0 text-fg-faint" />
                )}
                <span className={cx("text-body", entry.status === "completed" && "text-fg-muted")}>
                  {entry.content}
                </span>
              </li>
            ))}
          </ol>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

/** The thread's status chip; a click marks it for review, done, or clears it. */
function ThreadStatusMenu({ threadId }: { threadId: string }) {
  const status = useApp((s) => statusOf(s, threadId));
  const wt = useApp((s) => worktreeOf(s, threadId));
  const item =
    "flex w-full items-center gap-2 rounded-md px-2.5 py-[5px] text-body outline-none data-[highlighted]:bg-fill";
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          aria-label="Thread status"
          className="flex h-7 items-center gap-1.5 rounded-full bg-fill-subtle px-2.5 text-callout text-fg-muted hover:bg-fill data-[state=open]:bg-fill-strong"
        >
          {wt && (
            <span className="flex max-w-[160px] items-center gap-1 font-mono text-caption">
              <GitBranch size={11} className="shrink-0" />
              <span className="truncate">{wt.branch}</span>
            </span>
          )}
          {status === "idle" ? <CircleDashed size={12} /> : <StatusChip status={status} className="-mx-1" />}
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          sideOffset={6}
          className="elevated z-50 min-w-[190px] rounded-lg p-1"
        >
          <DropdownMenu.Item className={item} onSelect={() => setView({ kind: "review", threadId })}>
            <FileDiff size={13} /> Review Changes
          </DropdownMenu.Item>
          <DropdownMenu.Separator className="mx-1.5 my-1 h-px bg-border" />
          <DropdownMenu.Item className={item} onSelect={() => void setThreadStatus(threadId, "review")}>
            <Eye size={13} /> Mark for Review
          </DropdownMenu.Item>
          <DropdownMenu.Item className={item} onSelect={() => void setThreadStatus(threadId, "done")}>
            <CircleCheck size={13} /> Mark Done
          </DropdownMenu.Item>
          <DropdownMenu.Item className={item} onSelect={() => void setThreadStatus(threadId, "idle")}>
            <CircleDashed size={13} /> Clear Status
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

function Header({ thread, overBanner }: { thread?: ThreadState; overBanner?: boolean }) {
  const panel = useApp((s) => s.panel);
  const terminalOpen = useApp((s) => s.terminalOpen);
  const agentsOn = useApp((s) => !!s.agent?.agents) && (thread?.harness?.capabilities.subagents ?? true);
  const pullsOn = useApp((s) => !!s.agent?.pullRequests);
  const working = useApp((s) => (thread ? (s.agents[thread.id] ?? []).filter(agentActive).length : 0));
  const projectCwd = useApp((s) => s.projectCwd);
  const listed = useApp((s) => (thread ? s.sessions.find((x) => x.id === thread.id)?.title : undefined));
  const title = thread ? (thread.title ?? listed ?? "New thread") : "New Thread";
  const cwd = thread?.cwd ?? projectCwd;
  return (
    <MainHeader
      // Over a new thread's banner: frosted, its foot fading into the banner.
      className={overBanner ? "rc-frost" : "bg-bg"}
      title={
        <>
          {cwd && (
            <Tooltip label={cwd}>
              <span className="max-w-[260px] shrink-0 truncate px-2 text-callout text-fg-muted" dir="rtl">
                <bdi>{cwd.replace(/^\/Users\/[^/]+/, "~")}</bdi>
              </span>
            </Tooltip>
          )}
          {cwd && <span className="font-light text-fg-faint text-title3">/</span>}
          <h1 className="min-w-0 truncate font-semibold text-body">{title}</h1>
        </>
      }
      right={
        <>
          {thread && <ThreadStatusMenu threadId={thread.id} />}
          {thread && <PlanButton thread={thread} />}
          <Cluster>
            {thread && agentsOn && (
              <ClusterButton
                label="Agents"
                tooltip={working ? `Agents (${working} working)` : "Agents this thread started"}
                on={panel === "agents"}
                onClick={() => setPanel("agents")}
              >
                <span className="relative">
                  <Users size={14} />
                  {working > 0 && (
                    <span className="absolute -top-0.5 -right-1 size-1.5 rounded-full bg-accent" />
                  )}
                </span>
              </ClusterButton>
            )}
            {pullsOn && (
              <ClusterButton
                label="Pull requests"
                tooltip="Pull requests of this project"
                on={panel === "pulls"}
                onClick={() => setPanel("pulls")}
              >
                <GitPullRequest size={14} />
              </ClusterButton>
            )}
            <ClusterButton
              label="Terminal"
              tooltip="Terminal (⌘J)"
              on={terminalOpen}
              onClick={() => setTerminalOpen(!terminalOpen)}
            >
              <SquareTerminal size={14} />
            </ClusterButton>
            <ClusterButton
              label="Changes"
              tooltip="Changes in the working tree (⌘⌥B toggles the panel)"
              on={panel === "changes"}
              onClick={() => setPanel("changes")}
            >
              <FileDiff size={14} />
            </ClusterButton>
          </Cluster>
        </>
      }
    />
  );
}

function EmptyState({ banner }: { banner: boolean }) {
  const projectCwd = useApp((s) => s.projectCwd);
  return (
    <div className="flex flex-col items-center gap-3.5 pb-6 text-center">
      {/* A banner is drawn behind the chat instead of the icon. */}
      {!banner && <img src="/icon.png" alt="" className="size-[88px] select-none" draggable={false} />}
      <h2 className="font-semibold text-title">What should we build?</h2>
      <div className="flex max-w-[560px] items-center gap-1.5 text-callout text-fg-muted">
        <Folder size={13} />
        <span className="truncate">
          {projectCwd ? projectCwd.replace(/^\/Users\/[^/]+/, "~") : "No folder chosen"}
        </span>
        <button
          type="button"
          onClick={() => void pickFolder(projectCwd).then((p) => p && setProjectCwd(p))}
          className="text-accent hover:underline"
        >
          {projectCwd ? "Change" : "Choose…"}
        </button>
      </div>
    </div>
  );
}

function ErrorBanner({
  text,
  onDismiss,
  action,
}: {
  text: string;
  onDismiss?: () => void;
  action?: React.ReactNode;
}) {
  return (
    <Callout
      floating
      action={
        (action || onDismiss) && (
          <>
            {action}
            {onDismiss && <Pill onClick={onDismiss}>Dismiss</Pill>}
          </>
        )
      }
    >
      {text}
    </Callout>
  );
}

function EngineBanner() {
  const status = useApp((s) => s.status);
  const detail = useApp((s) => s.statusDetail);
  if (status === "ready") return null;
  if (status === "connecting") {
    return (
      <div className="flex items-center justify-center gap-2 py-1 text-callout text-fg-muted">
        <Spinner />
        {detail ?? "Starting the engine…"}
      </div>
    );
  }
  return (
    <ErrorBanner
      text={detail ?? "The engine stopped."}
      action={
        <Pill prominent onClick={() => void restartEngine()}>
          Restart Engine
        </Pill>
      }
    />
  );
}

export function ChatView() {
  const selectedId = useApp((s) => s.selectedId);
  const thread = useApp((s) => (s.selectedId ? s.threads[s.selectedId] : undefined));
  const stackRef = useRef<HTMLDivElement>(null);
  const [stackHeight, setStackHeight] = useState(140);
  const [dismissed, setDismissed] = useState<string | undefined>();
  const [width, setWidth] = useState(1000);
  const [height, setHeight] = useState(800);
  const banner = useBanner();
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = stackRef.current;
    if (!el) return;
    setStackHeight(el.offsetHeight);
    return observeSize(el, () => setStackHeight(el.offsetHeight));
  }, []);

  useEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    return observeSize(el, () => {
      setWidth(el.offsetWidth);
      setHeight(el.offsetHeight);
    });
  }, []);

  const isNew = !selectedId;
  const request = thread?.permissions[0];
  const error = thread?.error && thread.error !== dismissed ? thread.error : undefined;
  const side = width >= 600 ? 28 : width >= 360 ? 20 : 16;
  const showsBanner = isNew && !!banner.name;

  return (
    <main ref={rootRef} className="relative flex min-h-0 min-w-0 flex-1 flex-col bg-bg" aria-label="Chat">
      {showsBanner && (
        <NewThreadBanner
          key={banner.style}
          name={banner.name}
          style={banner.style}
          height={Math.round(height / 2)}
          className="!absolute top-0 left-0 rc-fade-in"
        />
      )}
      <Header thread={thread} overBanner={showsBanner} />
      {thread && !thread.loading ? (
        // Keyed: each thread gets a fresh virtualizer that starts at its end (Transcript.tsx).
        <Transcript key={thread.id} thread={thread} bottomInset={stackHeight + 36} />
      ) : thread?.loading ? (
        <div className="flex flex-1 items-center justify-center gap-2 text-callout text-fg-muted">
          <Spinner />
          Loading thread…
        </div>
      ) : (
        <div className="flex-1" />
      )}
      {/* The transcript fades out under the composer. */}
      {!isNew && (
        <div
          className="pointer-events-none absolute right-0 bottom-0 left-0"
          // The transcript fades out as it nears the floating composer, which it then scrolls under.
          style={{
            height: stackHeight + 72,
            background:
              "linear-gradient(to bottom, transparent 0, color-mix(in srgb, var(--rc-bg) 72%, transparent) 56px, color-mix(in srgb, var(--rc-bg) 80%, transparent) 100%)",
          }}
        />
      )}
      <div
        ref={stackRef}
        className={cx(
          "absolute left-1/2 flex w-full max-w-[980px] -translate-x-1/2 flex-col gap-2.5 transition-[bottom,transform] duration-500 ease-snappy",
          isNew ? "bottom-[max(16px,calc(50%-150px))]" : "bottom-4",
        )}
        style={{ paddingLeft: side, paddingRight: side }}
      >
        {isNew && <EmptyState banner={showsBanner} />}
        <EngineBanner />
        {error && (
          <ErrorBanner
            text={error}
            onDismiss={() => {
              setDismissed(error);
              void refreshSessions();
            }}
          />
        )}
        {thread && <LinkedThreads threadId={thread.id} />}
        {request && (
          <AttentionCard
            request={request}
            pending={thread?.permissions.filter((p) => !p.question).length ?? 1}
          />
        )}
        <Composer thread={thread} compact={width < 600} />
      </div>
    </main>
  );
}
