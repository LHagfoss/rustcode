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
} from "lucide-react";
import { DropdownMenu, Popover } from "radix-ui";
import { useEffect, useRef, useState } from "react";
import { displayPath, projectName } from "../acp/types";
import { STATUS_LABEL } from "../acp/v2";
import { useBanner } from "../lib/banner";
import { pickFolder } from "../lib/native";
import { observeSize } from "../lib/resize";
import { chooseFolder } from "../state/folders";
import {
  refreshSessions,
  repairThread,
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
import { HarnessIcon } from "./HarnessIcon";
import { LinkedThreads } from "./Links";
import { HEADER_HEIGHT, MainHeader } from "./MainHeader";
import { NewThreadBanner } from "./NewThreadBanner";
import { Callout, Cluster, ClusterButton, cx, Pill, Skeleton, Tooltip } from "./primitives";
import { ReportPanel } from "./ReportPanel";
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
        <Popover.Content align="end" sideOffset={6} className="elevated z-40 w-[380px] p-3 rc-in">
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
    "flex w-full items-center gap-2 rounded-lg px-3 py-[7px] text-body outline-none data-[highlighted]:bg-fill";
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          aria-label={`Thread status: ${STATUS_LABEL[status]}`}
          className="flex h-7 items-center gap-1.5 rounded-full bg-fill-subtle px-2.5 text-callout text-fg-muted hover:bg-fill data-[state=open]:bg-fill-strong"
        >
          {wt && (
            <span className="flex max-w-[160px] items-center gap-1 font-mono text-caption">
              <GitBranch size={11} className="shrink-0" />
              <span className="truncate">{wt.branch}</span>
            </span>
          )}
          {/* The icon says it; the sidebar and the working line say the rest. */}
          {status === "idle" ? (
            <CircleDashed size={12} />
          ) : (
            <StatusChip status={status} compact className="-mx-1" />
          )}
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content align="end" sideOffset={6} className="elevated z-50 min-w-[200px] p-1.5">
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
  const pullsOn = useApp((s) => !!s.agent?.pullRequests);
  const projectCwd = useApp((s) => s.projectCwd);
  const listed = useApp((s) => (thread ? s.sessions.find((x) => x.id === thread.id)?.title : undefined));
  const title = thread ? (thread.title ?? listed ?? "New thread") : "New thread";
  const cwd = thread?.cwd ?? projectCwd;
  return (
    <MainHeader
      // Glass like every header; over a new thread's banner a lighter tint, so the picture shows.
      className={overBanner ? "rc-frost" : undefined}
      title={
        <>
          {cwd && (
            <Tooltip
              label={
                thread?.running ? `${cwd} · stop the turn to change folder` : `${cwd} · change folder (⌘O)`
              }
            >
              <button
                type="button"
                disabled={thread?.running}
                onClick={() => void chooseFolder(false)}
                className="-mx-1 max-w-[260px] shrink-0 truncate rounded-sm px-1 text-body text-fg-muted enabled:hover:bg-fill-subtle enabled:hover:text-fg disabled:cursor-default"
                dir="rtl"
              >
                <bdi>{displayPath(cwd)}</bdi>
              </button>
            </Tooltip>
          )}
          {cwd && (
            <span aria-hidden className="text-body text-fg-faint">
              /
            </span>
          )}
          {thread?.harness && thread.harness.id !== "rustcode" && (
            <HarnessIcon id={thread.harness.id} size={14} className="text-fg-muted" />
          )}
          <h1 className="min-w-0 truncate font-semibold text-body">{title}</h1>
        </>
      }
      right={
        <>
          {thread && <ThreadStatusMenu threadId={thread.id} />}
          {thread && <PlanButton thread={thread} />}
          <Cluster>
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
  const choose = () => void pickFolder(projectCwd).then((p) => p && setProjectCwd(p));
  return (
    <div className="flex flex-col items-center gap-3.5 pb-6 text-center">
      {/* A banner is drawn behind the chat instead of the icon. */}
      {!banner && <img src="/icon.png" alt="" className="size-[88px] select-none" draggable={false} />}
      <h2 className="font-semibold text-title tracking-tight">
        {projectCwd ? (
          <>
            What should we build in{" "}
            {/* The folder names itself here, and a click swaps it. The pill pads
                the name on hover without widening the sentence. */}
            <button
              type="button"
              onClick={choose}
              title={`Change folder (${displayPath(projectCwd)})`}
              className="-mx-1 inline-block max-w-full truncate rounded-md px-1 align-bottom text-fg hover:bg-fill"
            >
              {projectName(projectCwd)}
              <span className="sr-only"> (change folder)</span>
            </button>
            ?
          </>
        ) : (
          "What should we build?"
        )}
      </h2>
      <div className="flex max-w-[560px] items-center gap-1.5 text-fg-faint text-sub">
        <Folder size={13} />
        <span className="truncate">{projectCwd ? displayPath(projectCwd) : "No folder chosen"}</span>
        <button type="button" onClick={choose} className="text-accent hover:underline">
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

/** Where an opening thread's transcript will be: a sent message, then reply lines, twice. */
function TranscriptSkeleton() {
  const reply = [96, 88, 92, 54];
  // A thread that opens quickly never shows it: it appears only after 200 ms, so it does not flash.
  const [shown, setShown] = useState(false);
  useEffect(() => {
    const t = setTimeout(() => setShown(true), 200);
    return () => clearTimeout(t);
  }, []);
  if (!shown) return <div className="min-h-0 flex-1" />;
  return (
    <div className="min-h-0 flex-1 overflow-hidden" role="status" aria-label="Loading thread">
      <div
        className="mx-auto flex w-full max-w-[824px] flex-col gap-[18px] px-7"
        style={{ paddingTop: HEADER_HEIGHT + 20 }}
      >
        {[0, 1].map((turn) => (
          <div key={turn} className="flex flex-col gap-[18px]">
            <Skeleton className="h-9 w-[38%] self-end rounded-lg" />
            <div className="flex flex-col gap-2">
              {reply.map((w, i) => (
                <Skeleton key={i} className="h-3" style={{ width: `${turn ? w * 0.8 : w}%` }} />
              ))}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

/**
 * A click on blank chat space lets go of the composer, so its caret does not
 * keep blinking while you read. Controls, links, fields and the composer
 * itself keep the click; text selection still starts as usual.
 */
function releaseComposer(e: React.MouseEvent) {
  const target = e.target as HTMLElement;
  const active = document.activeElement as HTMLElement | null;
  if (!active?.matches('textarea[aria-label="Message"]')) return;
  if (
    target.closest(
      'button, a, input, textarea, select, label, [role="button"], [contenteditable], .rc-composer',
    )
  )
    return;
  active.blur();
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

  // Toasts sit centred at the top of the chat column, under its header (ui/Toaster.tsx).
  // biome-ignore lint/correctness/useExhaustiveDependencies: re-place when the column resizes
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const r = root.getBoundingClientRect();
    const doc = document.documentElement.style;
    doc.setProperty("--rc-toast-center", `${Math.round(r.left + r.width / 2)}px`);
    doc.setProperty("--rc-toast-top", `${Math.round(r.top + HEADER_HEIGHT + 8)}px`);
  }, [isNew, stackHeight, width, height]);
  const request = thread?.permissions[0];
  const report = useApp((s) => s.report);
  const error = thread?.error && thread.error !== dismissed ? thread.error : undefined;
  const side = width >= 600 ? 28 : width >= 360 ? 20 : 16;
  const showsBanner = isNew && !!banner.name;

  return (
    <main
      ref={rootRef}
      className="relative flex min-h-0 min-w-0 flex-1 flex-col bg-bg"
      aria-label="Chat"
      onMouseDown={releaseComposer}
    >
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
        <Transcript key={thread.id} thread={thread} bottomInset={stackHeight + 36} topInset={HEADER_HEIGHT} />
      ) : thread?.loading ? (
        <TranscriptSkeleton />
      ) : (
        <div className="flex-1" />
      )}
      {/* The transcript fades out at the composer's edge. */}
      {!isNew && (
        <div
          className="pointer-events-none absolute right-0 bottom-0 left-0"
          // A short fade, then solid: the transcript scrolls under the composer, not through it.
          style={{
            // The stack sits 16px up (bottom-4), so the fade ends above the composer's top edge.
            height: stackHeight + 16 + 20,
            background: "linear-gradient(to bottom, transparent 0, var(--rc-bg) 20px, var(--rc-bg) 100%)",
          }}
        />
      )}
      <div
        ref={stackRef}
        className={cx(
          "absolute left-1/2 flex w-full max-w-[824px] -translate-x-1/2 flex-col gap-2.5 transition-[bottom,transform] duration-500 ease-snappy",
          isNew ? "bottom-[max(16px,calc(50%-150px))]" : "bottom-4",
        )}
        style={{ paddingLeft: side, paddingRight: side }}
      >
        {isNew && <EmptyState banner={showsBanner} />}
        {error && (
          <ErrorBanner
            text={error}
            action={
              thread && error.startsWith("Could not open this thread") ? (
                <Pill prominent onClick={() => void repairThread(thread.id)}>
                  Repair Thread
                </Pill>
              ) : undefined
            }
            onDismiss={() => {
              setDismissed(error);
              void refreshSessions();
            }}
          />
        )}
        {thread && <LinkedThreads threadId={thread.id} />}
        {report && report.threadId === thread?.id && <ReportPanel text={report.text} />}
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
