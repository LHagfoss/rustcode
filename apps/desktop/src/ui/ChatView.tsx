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
  PanelLeft,
  SquarePen,
  SquareTerminal,
  TriangleAlert,
  Users,
} from "lucide-react";
import { DropdownMenu, Popover } from "radix-ui";
import { useEffect, useRef, useState } from "react";
import { agentActive, leftPercent, shownWindow, windowDetail, windowTitle } from "../acp/rustcode";
import { useBanner } from "../lib/banner";
import { pickFolder } from "../lib/native";
import {
  refreshSessions,
  restartEngine,
  setPanel,
  setProjectCwd,
  setTerminalOpen,
  setThreadStatus,
  setView,
  startNewThread,
  statusOf,
  toggleSidebar,
  useApp,
  worktreeOf,
} from "../state/store";
import { formatTokens, type ThreadState } from "../state/thread";
import { AttentionCard } from "./AttentionCards";
import { Composer } from "./Composer";
import { LinkedThreads } from "./Links";
import { NewThreadBanner } from "./NewThreadBanner";
import { Cluster, ClusterButton, cx, Pill, Spinner, Tooltip } from "./primitives";
import { StatusChip } from "./Status";
import { Transcript } from "./Transcript";

export function UsageCapsule({ thread }: { thread: ThreadState }) {
  const usage = thread.usage;
  const plan = useApp((s) => s.planUsage[thread.id]);
  const shown = plan ? shownWindow(plan) : undefined;
  if (!usage?.size && !shown) return null;
  if (!usage?.size) return <PlanMeter thread={thread} />;
  const fraction = Math.min(1, usage.used / usage.size);
  const tint = fraction < 0.8 ? "bg-brand" : fraction < 0.95 ? "bg-attention" : "bg-danger";
  const detail = [
    `${Math.round((1 - fraction) * 100)}% context left${usage.estimated ? " (estimated)" : ""}`,
    usage.inputTokens !== undefined
      ? `${formatTokens(usage.inputTokens)} in · ${formatTokens(usage.outputTokens ?? 0)} out`
      : undefined,
    usage.cachedTokens ? `${formatTokens(usage.cachedTokens)} cached` : undefined,
    ...(plan
      ? [
          "",
          ...plan.windows.map((w) => `${windowTitle(w)}: ${windowDetail(w)}`),
          plan.source === "codex" ? "As the Codex CLI reports for your ChatGPT plan" : undefined,
        ]
      : []),
  ]
    .filter(Boolean)
    .join("\n");
  return (
    <Tooltip label={<span className="whitespace-pre-line">{detail}</span>}>
      <div
        className="flex h-7 items-center gap-2 rounded-full bg-fill-3 px-2.5"
        role="meter"
        aria-label="Context used"
        aria-valuemin={0}
        aria-valuemax={usage.size}
        aria-valuenow={usage.used}
      >
        <div className="h-[5px] w-[18px] overflow-hidden rounded-full bg-fg-4">
          <div
            className={cx("h-full rounded-full transition-[width] duration-300 ease-snappy", tint)}
            style={{ width: `max(3px, ${fraction * 100}%)` }}
          />
        </div>
        <span className="font-medium text-caption text-fg-2 tabular">
          {formatTokens(usage.used)} / {formatTokens(usage.size)}
        </span>
        {shown && <PlanDot left={leftPercent(shown)} title={windowTitle(shown)} />}
      </div>
    </Tooltip>
  );
}

/** The plan's shortest window, as the capsule's second part: `5-hour 58%`. */
function PlanDot({ left, title }: { left: number; title: string }) {
  const tint = left > 20 ? "text-fg-2" : left > 5 ? "text-attention" : "text-danger";
  return (
    <span
      className={cx(
        "flex items-center gap-1 border-hairline border-l pl-2 font-medium text-caption tabular",
        tint,
      )}
    >
      <span
        className="relative size-[11px] rounded-full"
        style={{ background: `conic-gradient(currentColor ${left * 3.6}deg, var(--rc-fg-4) 0)` }}
      />
      {title} {left}%
    </span>
  );
}

function PlanMeter({ thread }: { thread: ThreadState }) {
  const plan = useApp((s) => s.planUsage[thread.id]);
  const shown = plan ? shownWindow(plan) : undefined;
  if (!plan || !shown) return null;
  return (
    <Tooltip
      label={
        <span className="whitespace-pre-line">
          {plan.windows.map((w) => `${windowTitle(w)}: ${windowDetail(w)}`).join("\n")}
        </span>
      }
    >
      <div
        className="flex h-7 items-center rounded-full bg-fill-3 px-2.5"
        role="meter"
        aria-label="Plan usage left"
        aria-valuenow={leftPercent(shown)}
      >
        <PlanDot left={leftPercent(shown)} title={windowTitle(shown)} />
      </div>
    </Tooltip>
  );
}

function PlanButton({ thread }: { thread: ThreadState }) {
  if (!thread.plan.length) return null;
  const done = thread.plan.filter((p) => p.status === "completed").length;
  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <button
          type="button"
          className="flex h-7 items-center gap-1.5 rounded-full bg-fill-3 px-2.5 font-medium text-callout text-fg-2 hover:bg-fill-2 data-[state=open]:bg-fill-1"
        >
          <ListChecks size={13} />
          <span className="tabular">
            {done}/{thread.plan.length}
          </span>
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="end"
          sideOffset={6}
          className="surface z-40 w-[380px] rounded-[20px] border border-hairline p-3 rc-in"
        >
          <div className="mb-1 flex items-baseline justify-between px-1">
            <span className="font-semibold text-body">Plan</span>
            <span className="text-callout text-fg-2">
              {done} of {thread.plan.length} done
            </span>
          </div>
          <p className="mb-2 px-1 text-caption text-fg-3">As the agent marked each step.</p>
          <ol className="flex flex-col">
            {thread.plan.map((entry, i) => (
              <li key={i} className="flex items-start gap-2 rounded-lg px-2 py-[7px]">
                {entry.status === "completed" ? (
                  <CircleCheck size={14} className="mt-px shrink-0 text-success" />
                ) : entry.status === "in_progress" ? (
                  <CircleDot size={14} className="mt-px shrink-0 text-info" />
                ) : (
                  <CircleDashed size={14} className="mt-px shrink-0 text-fg-3" />
                )}
                <span className={cx("text-body", entry.status === "completed" && "text-fg-2")}>
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
    "flex w-full items-center gap-2 rounded-[8px] px-2.5 py-[5px] text-body outline-none data-[highlighted]:bg-brand-fill data-[highlighted]:text-white";
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          aria-label="Thread status"
          className="flex h-7 items-center gap-1.5 rounded-full bg-fill-3 px-2.5 text-callout text-fg-2 hover:bg-fill-2 data-[state=open]:bg-fill-1"
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
          className="surface z-50 min-w-[190px] rounded-[12px] border border-hairline p-1"
        >
          <DropdownMenu.Item className={item} onSelect={() => setView({ kind: "review", threadId })}>
            <FileDiff size={13} /> Review Changes
          </DropdownMenu.Item>
          <DropdownMenu.Separator className="mx-1.5 my-1 h-px bg-hairline" />
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
  const sidebarHidden = useApp((s) => s.sidebarHidden);
  const projectCwd = useApp((s) => s.projectCwd);
  const listed = useApp((s) => (thread ? s.sessions.find((x) => x.id === thread.id)?.title : undefined));
  const title = thread ? (thread.title ?? listed ?? "New thread") : "New Thread";
  const cwd = thread?.cwd ?? projectCwd;
  return (
    <header
      data-tauri-drag-region
      className={cx(
        "relative z-10 flex h-[42px] shrink-0 items-center gap-2.5 pt-1.5 pb-2 pr-2.5 transition-[background-color] duration-300",
        // Over a new thread's banner: frosted, its foot fading into the banner.
        overBanner ? "rc-frost" : "bg-chat/95",
        sidebarHidden ? "pl-[80px]" : "pl-2.5",
      )}
    >
      {sidebarHidden && (
        <Cluster>
          <ClusterButton label="Show sidebar" tooltip="Show sidebar (⌘B)" onClick={toggleSidebar}>
            <PanelLeft size={14} />
          </ClusterButton>
          <ClusterButton label="New thread" tooltip="New thread (⌘N)" onClick={() => startNewThread()}>
            <SquarePen size={14} />
          </ClusterButton>
        </Cluster>
      )}
      <div className="flex min-w-0 flex-1 items-center gap-1.5" data-tauri-drag-region>
        {cwd && (
          <Tooltip label={cwd}>
            <span
              className="max-w-[260px] shrink-0 truncate rounded-lg px-2 text-callout text-fg-2"
              dir="rtl"
            >
              <bdi>{cwd.replace(/^\/Users\/[^/]+/, "~")}</bdi>
            </span>
          </Tooltip>
        )}
        {cwd && <span className="font-light text-fg-4 text-title3">/</span>}
        <h1 className="min-w-0 truncate font-semibold text-body">{title}</h1>
      </div>
      {thread && <ThreadStatusMenu threadId={thread.id} />}
      {thread && <PlanButton thread={thread} />}
      {thread && <UsageCapsule thread={thread} />}
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
              {working > 0 && <span className="absolute -top-0.5 -right-1 size-1.5 rounded-full bg-brand" />}
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
          tooltip="Changes in the working tree"
          on={panel === "changes"}
          onClick={() => setPanel("changes")}
        >
          <FileDiff size={14} />
        </ClusterButton>
      </Cluster>
    </header>
  );
}

function EmptyState({ banner }: { banner: boolean }) {
  const projectCwd = useApp((s) => s.projectCwd);
  return (
    <div className="flex flex-col items-center gap-3.5 pb-6 text-center">
      {/* A banner is drawn behind the chat instead of the icon. */}
      {!banner && <img src="/icon.png" alt="" className="size-[88px] select-none" draggable={false} />}
      <h2 className="font-semibold text-title">What should we build?</h2>
      <div className="flex max-w-[560px] items-center gap-1.5 text-callout text-fg-2">
        <Folder size={13} />
        <span className="truncate">
          {projectCwd ? projectCwd.replace(/^\/Users\/[^/]+/, "~") : "No folder chosen"}
        </span>
        <button
          type="button"
          onClick={() => void pickFolder(projectCwd).then((p) => p && setProjectCwd(p))}
          className="text-brand hover:underline"
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
    <div className="surface flex items-center gap-2.5 rounded-[14px] px-3.5 py-2.5 rc-in" role="alert">
      <TriangleAlert size={14} className="shrink-0 text-attention" />
      <span className="selectable line-clamp-4 flex-1 text-callout">{text}</span>
      {action}
      {onDismiss && <Pill onClick={onDismiss}>Dismiss</Pill>}
    </div>
  );
}

function EngineBanner() {
  const status = useApp((s) => s.status);
  const detail = useApp((s) => s.statusDetail);
  if (status === "ready") return null;
  if (status === "connecting") {
    return (
      <div className="flex items-center justify-center gap-2 py-1 text-callout text-fg-2">
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
    const observer = new ResizeObserver(() => setStackHeight(el.offsetHeight));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const observer = new ResizeObserver(() => {
      setWidth(el.offsetWidth);
      setHeight(el.offsetHeight);
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const isNew = !selectedId;
  const request = thread?.permissions[0];
  const error = thread?.error && thread.error !== dismissed ? thread.error : undefined;
  const side = width >= 600 ? 28 : width >= 360 ? 20 : 16;
  const showsBanner = isNew && !!banner.name;

  return (
    <main ref={rootRef} className="relative flex min-h-0 min-w-0 flex-1 flex-col bg-chat" aria-label="Chat">
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
        <div className="flex flex-1 items-center justify-center gap-2 text-callout text-fg-2">
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
          style={{
            height: stackHeight + 52,
            background: "linear-gradient(to bottom, transparent, var(--rc-chat) 40%)",
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
