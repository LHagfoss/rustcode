// The chat view: the header (project / title, plan, usage, panels), then one row
// holding the chat column — transcript and composer stack with error and
// approval cards — and the right panel docked beside it (ui/RightPanel.tsx). A
// new thread centres the composer and places its optional overview below it.

import {
  ChevronDown,
  CircleCheck,
  CircleDashed,
  CircleDot,
  Eye,
  FileDiff,
  GitBranch,
  GitPullRequest,
  ListChecks,
  type LucideIcon,
  PanelRight,
  Sparkles,
  SquareTerminal,
} from "lucide-react";
import { DropdownMenu, Popover } from "radix-ui";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { displayPath, projectName } from "../acp/types";
import { STATUS_LABEL } from "../acp/v2";
import { darkenOverlay, useBanner } from "../lib/banner";
import { observeSize } from "../lib/resize";
import { motionAllowed, PENDING_ENTRY_MS } from "../lib/sendFlight";
import { composedText, emptyExtras, sentAttachments } from "../state/compose";
import { chooseFolder } from "../state/folders";
import type { HeaderButtonId } from "../state/headerButtons";
import { heldSetup, type PendingSetup, pendingStatus } from "../state/setup";
import {
  autoCompactOffFor,
  dismissPendingSetup,
  refreshSessions,
  repairThread,
  retryPendingSetup,
  setAutoCompactForThread,
  setPanel,
  setTerminalOpen,
  setThreadStatus,
  settleThread,
  setView,
  statusOf,
  useApp,
  useShortcutHint,
  worktreeOf,
} from "../state/store";
import { formatClock, type ThreadState } from "../state/thread";
import { overviewExpanded, setOverviewExpanded } from "../state/widgets";
import { AttentionCard, ResolvedApprovalBar } from "./AttentionCards";
import { Composer } from "./Composer";
import { HarnessIcon } from "./HarnessIcon";
import { LinkedThreads } from "./Links";
import { HEADER_HEIGHT, MainHeader } from "./MainHeader";
import { NewThreadBanner } from "./NewThreadBanner";
import { NEW_THREAD_WIDGETS_MIN_HEIGHT, NewThreadWidgets } from "./NewThreadWidgets";
import { boundTips } from "./newThreadTips";
import { ProjectMark } from "./ProjectMark";
import {
  Callout,
  Cluster,
  ClusterButton,
  cx,
  Kbd,
  MenuContent,
  menuItem,
  menuSeparator,
  Pill,
  Skeleton,
  Spinner,
  surfaceCollisionPadding,
  Tooltip,
} from "./primitives";
import { RemoteThreadIndicator } from "./RemoteAccessPage";
import { ReportPanel } from "./ReportPanel";
import { RightPanel } from "./RightPanel";
import { RollingNumber } from "./RollingNumber";
import { UserRow } from "./Rows";
import { StatusChip, useThreadStatus } from "./Status";
import { FIRST_ROW_PAD, Transcript } from "./Transcript";
import { WhileAwayCard } from "./WhileAway";

function PlanButton({ thread }: { thread: ThreadState }) {
  if (!thread.plan.length) return null;
  const done = thread.plan.filter((p) => p.status === "completed").length;
  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <button
          type="button"
          className="flex h-7 items-center gap-1.5 rounded-full bg-fill-subtle px-2 font-medium text-callout text-fg-muted hover:bg-fill data-[state=open]:bg-fill-strong"
        >
          <ListChecks size={13} />
          <span className="flex items-center gap-0.5 tabular">
            <RollingNumber value={done} />
            <span className="text-fg-faint">/</span>
            <RollingNumber value={thread.plan.length} />
          </span>
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="end"
          sideOffset={6}
          collisionPadding={surfaceCollisionPadding}
          className="elevated z-40 max-h-[var(--radix-popover-content-available-height,80vh)] w-[min(380px,var(--radix-popover-content-available-width,calc(100vw-16px)))] overflow-y-auto p-3 rc-in"
        >
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

/** The thread's status chip; a click marks it for review, settles or unsettles it. */
function ThreadStatusMenu({ threadId }: { threadId: string }) {
  const status = useApp((s) => statusOf(s, threadId));
  const autoCompactOff = useApp((s) => autoCompactOffFor(s, threadId));
  const wt = useApp((s) => worktreeOf(s, threadId));
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          aria-label={`Thread status: ${STATUS_LABEL[status]}`}
          className="flex h-7 items-center gap-1.5 rounded-full bg-fill-subtle px-2 text-callout text-fg-muted hover:bg-fill data-[state=open]:bg-fill-strong"
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
      <MenuContent align="end">
        <DropdownMenu.Item className={menuItem} onSelect={() => setView({ kind: "review", threadId })}>
          <FileDiff size={13} /> Review Changes
        </DropdownMenu.Item>
        <DropdownMenu.Separator className={menuSeparator} />
        {status !== "review" && (
          <DropdownMenu.Item className={menuItem} onSelect={() => void setThreadStatus(threadId, "review")}>
            <Eye size={13} /> Mark for Review
          </DropdownMenu.Item>
        )}
        {status === "done" ? (
          <DropdownMenu.Item className={menuItem} onSelect={() => void setThreadStatus(threadId, "idle")}>
            <CircleDashed size={13} /> Unsettle
          </DropdownMenu.Item>
        ) : (
          <DropdownMenu.Item className={menuItem} onSelect={() => settleThread(threadId)}>
            <CircleCheck size={13} /> Settle
          </DropdownMenu.Item>
        )}
        {status === "review" && (
          <DropdownMenu.Item className={menuItem} onSelect={() => void setThreadStatus(threadId, "idle")}>
            <CircleDashed size={13} /> Clear Status
          </DropdownMenu.Item>
        )}
        <DropdownMenu.Separator className={menuSeparator} />
        <DropdownMenu.Item
          className={menuItem}
          onSelect={() => setAutoCompactForThread(threadId, autoCompactOff)}
        >
          <Sparkles size={13} /> {autoCompactOff ? "Turn On Auto-compact" : "Turn Off Auto-compact"}
        </DropdownMenu.Item>
      </MenuContent>
    </DropdownMenu.Root>
  );
}

// The header's buttons, one entry per id in state/headerButtons.ts: the label
// and icon Settings shows, and the button itself, which decides on its own
// whether this thread has anything to show (a plan, a pull requests view). The
// record is typed by id, so a new button in that list does not compile without
// its entry here, and a user's order only ever names an id.

/** Stands alone as a pill, or joins the header's cluster of icon buttons. */
type HeaderButtonKind = "pill" | "cluster";

interface HeaderButtonEntry {
  label: string;
  icon: LucideIcon;
  kind: HeaderButtonKind;
  Button: (props: { thread?: ThreadState }) => ReactNode;
}

/** The thread's status menu; a thread of its own, no chip on a new one. */
function StatusHeaderButton({ thread }: { thread?: ThreadState }) {
  return thread ? <ThreadStatusMenu threadId={thread.id} /> : null;
}

/** The plan's progress, while there is a plan. */
function PlanHeaderButton({ thread }: { thread?: ThreadState }) {
  return thread ? <PlanButton thread={thread} /> : null;
}

/** The pull requests panel, where the harness reports pull requests. */
function PullsHeaderButton() {
  const on = useApp((s) => s.panel === "pulls");
  const available = useApp((s) => !!s.agent?.pullRequests);
  if (!available) return null;
  return (
    <ClusterButton
      label="Pull requests"
      tooltip="Pull requests of this project"
      on={on}
      onClick={() => setPanel("pulls")}
    >
      <GitPullRequest size={14} />
    </ClusterButton>
  );
}

function TerminalHeaderButton() {
  const open = useApp((s) => s.terminalOpen);
  const key = useShortcutHint("toggle-terminal");
  return (
    <ClusterButton
      label="Terminal"
      tooltip={`Terminal (${key})`}
      on={open}
      onClick={() => setTerminalOpen(!open)}
    >
      <SquareTerminal size={14} />
    </ClusterButton>
  );
}

// Both panel buttons carry `data-panel-toggle`: a panel that closes with nothing
// else focused hands the keyboard back to one of them (ui/RightPanel.tsx).
function DetailsHeaderButton() {
  const on = useApp((s) => s.panel === "details");
  return (
    <ClusterButton
      label="Thread details"
      tooltip="Thread details"
      data-panel-toggle
      on={on}
      onClick={() => setPanel("details")}
    >
      <PanelRight size={14} />
    </ClusterButton>
  );
}

function ChangesHeaderButton() {
  const on = useApp((s) => s.panel === "changes");
  const key = useShortcutHint("toggle-panel");
  return (
    <ClusterButton
      label="Changes"
      tooltip={`Changes in the working tree (${key} toggles the panel)`}
      data-panel-toggle
      on={on}
      onClick={() => setPanel("changes")}
    >
      <FileDiff size={14} />
    </ClusterButton>
  );
}

export const HEADER_BUTTONS: Record<HeaderButtonId, HeaderButtonEntry> = {
  status: { label: "Status", icon: CircleDashed, kind: "pill", Button: StatusHeaderButton },
  plan: { label: "Plan", icon: ListChecks, kind: "pill", Button: PlanHeaderButton },
  pulls: { label: "Pull requests", icon: GitPullRequest, kind: "cluster", Button: PullsHeaderButton },
  terminal: { label: "Terminal", icon: SquareTerminal, kind: "cluster", Button: TerminalHeaderButton },
  details: { label: "Thread details", icon: PanelRight, kind: "cluster", Button: DetailsHeaderButton },
  changes: { label: "Changes", icon: FileDiff, kind: "cluster", Button: ChangesHeaderButton },
};

/**
 * The buttons that toggle the side panel. They always draw after the rest of
 * the header's buttons, in their kept order, except that "details" is forced
 * last: the panel's control is the last, rightmost one the user can reach, and
 * no saved order puts it before "changes".
 */
const PANEL_BUTTONS: ReadonlySet<HeaderButtonId> = new Set<HeaderButtonId>(["details", "changes"]);

function Header({
  thread,
  pending,
  overBanner,
}: {
  thread?: ThreadState;
  /** A thread setting up has no session yet, but its title and folder are known (state/setup.ts). */
  pending?: PendingSetup;
  overBanner?: boolean;
}) {
  const projectCwd = useApp((s) => s.projectCwd);
  const shown = useApp((s) => s.headerButtons);
  const folderKey = useShortcutHint("thread-folder");
  const listed = useApp((s) => (thread ? s.sessions.find((x) => x.id === thread.id)?.title : undefined));
  const title = pending?.title ?? (thread ? (thread.title ?? listed ?? "New thread") : "New thread");
  const cwd = pending?.cwd ?? thread?.cwd ?? projectCwd;
  // The buttons the user keeps, in their order, with the panel's toggles last:
  // the panel is the last thing beside the chat, so its controls are the last
  // controls too, and "details" is the rightmost of them whatever order the
  // user saved. Each one decides whether this thread shows it, and the icon
  // buttons share one cluster wherever they land.
  const right: ReactNode[] = [];
  let cluster: ReactNode[] = [];
  const closeCluster = () => {
    if (!cluster.length) return;
    right.push(<Cluster key={`cluster-${right.length}`}>{cluster}</Cluster>);
    cluster = [];
  };
  const kept = [
    ...shown.filter((id) => !PANEL_BUTTONS.has(id)),
    ...shown.filter((id) => PANEL_BUTTONS.has(id) && id !== "details"),
    ...shown.filter((id) => id === "details"),
  ];
  for (const id of kept) {
    const entry = HEADER_BUTTONS[id];
    const node = <entry.Button key={id} thread={thread} />;
    if (entry.kind === "cluster") cluster.push(node);
    else {
      closeCluster();
      right.push(node);
    }
  }
  closeCluster();
  return (
    <MainHeader
      // Glass like every header; over a new thread's banner a lighter tint, so the picture shows.
      className={overBanner ? "rc-frost" : undefined}
      title={
        <>
          {cwd && (
            <Tooltip
              label={
                thread?.running
                  ? `${displayPath(cwd)} · stop the turn to change folder`
                  : `${displayPath(cwd)} · change folder (${folderKey})`
              }
            >
              <button
                type="button"
                disabled={thread?.running}
                onClick={() => void chooseFolder(false)}
                className="-mx-1 flex max-w-[220px] shrink-0 items-center gap-1.5 rounded-sm px-1 text-body text-fg-muted enabled:hover:bg-fill-subtle enabled:hover:text-fg disabled:cursor-default"
              >
                <ProjectMark name={projectName(cwd)} />
                <span className="min-w-0 truncate">{projectName(cwd)}</span>
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
          {/* A paired device is attached, or the thread is shared with them (RemoteAccessPage.tsx). */}
          <RemoteThreadIndicator sessionId={thread?.id} />
        </>
      }
      right={right.length ? right : undefined}
    />
  );
}

function ErrorBanner({
  text,
  detail,
  onDismiss,
  action,
}: {
  text: string;
  /** What the harness or engine sent, when `text` says it in other words: shown under Details. */
  detail?: string;
  onDismiss?: () => void;
  action?: React.ReactNode;
}) {
  const [open, setOpen] = useState(false);
  return (
    <Callout
      floating
      action={
        (action || onDismiss || detail) && (
          <>
            {action}
            {detail && (
              <Pill aria-expanded={open} onClick={() => setOpen((o) => !o)}>
                {open ? "Hide details" : "Details"}
              </Pill>
            )}
            {onDismiss && <Pill onClick={onDismiss}>Dismiss</Pill>}
          </>
        )
      }
    >
      {text}
      {detail && open && (
        <span className="mt-1 block select-text break-all font-mono text-caption text-fg-muted">
          {detail}
        </span>
      )}
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
        className="mx-auto flex w-full max-w-[var(--rc-chat-content-max-width)] flex-col gap-[18px] px-3 sm:px-5"
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
      'button, a, input, textarea, select, label, [role="button"], [contenteditable], .rc-composer, .rc-composer-footer, .rc-composer-tray, .rc-composer-drawer',
    )
  )
    return;
  active.blur();
}

/** The setup's clock, ticking on its own so the chat does not re-render every second. */
function PendingClock({ since }: { since: number }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  return <>{formatClock(now - since)}</>;
}

/**
 * The message a thread was sent with, while the engine is still making it: the same user row the
 * transcript will draw, with its pictures and files, in the place it will keep, and under it what is
 * happening and for how long. Nothing here waits on the field at the foot: the thread's own chat
 * takes the screen the moment the session exists. Failed, the reason takes the line's place with
 * Retry, which sends the same message again, and Dismiss, which puts the text back in the field.
 */
function PendingSetupView({ pending }: { pending: PendingSetup }) {
  const extras = pending.extras ?? emptyExtras;
  const shown = composedText(pending.text, extras);
  const attachments = sentAttachments(extras);
  return (
    // Absolute like the composer's stack beside it: the column holds nothing above this view, and
    // taking it out of the flow keeps it that way, so no sibling that mounts during a send can push
    // the message down. `overflow-anchor: none` and `overscroll-contain` are the transcript's own
    // scroller settings: nothing here may scroll or be scrolled by the browser on our behalf.
    <div className="absolute inset-0 overflow-y-auto overscroll-contain px-3 [overflow-anchor:none] [scrollbar-gutter:stable_both-edges] sm:px-12">
      <div
        // The row takes the place the transcript's first row keeps — the lane's cap, the same 20px
        // under the glass header (`FIRST_ROW_PAD`), so the thread takes the screen over without
        // moving it — and arrives in 160ms from 8px below. The thread waits for that entry before it
        // takes the screen (`PENDING_ENTRY_MS`), because a session can exist in one frame.
        className="rc-send-in mx-auto flex w-full max-w-[var(--rc-chat-content-max-width)] flex-col gap-3"
        style={{ paddingTop: FIRST_ROW_PAD + HEADER_HEIGHT }}
      >
        <UserRow
          item={{
            kind: "user",
            id: pending.id,
            text: shown || pending.title,
            attachments: attachments.length ? attachments : undefined,
          }}
        />
        {pending.failure ? (
          <Callout
            action={
              <>
                <Pill prominent onClick={() => void retryPendingSetup()}>
                  Retry
                </Pill>
                <Pill onClick={dismissPendingSetup}>Dismiss</Pill>
              </>
            }
          >
            {pending.failure.text}
          </Callout>
        ) : (
          <div className="flex items-center gap-2 text-body text-fg-muted" role="status">
            <Spinner size={14} />
            <span>{pendingStatus(pending)}</span>
            <span className="text-fg-faint tabular">
              <PendingClock since={pending.startedAt} />
            </span>
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * A settled thread says so above the field, with the way out of it in reach: settling
 * leaves the chat for a new thread, so a thread you are reading needs its own way back.
 */
function SettledBar({ threadId }: { threadId: string }) {
  const status = useThreadStatus(threadId);
  if (status !== "done") return null;
  return (
    <div
      role="status"
      className="rc-glass-overlay relative flex h-[32px] items-center gap-2 rounded-full pr-1.5 pl-3 text-callout rc-in"
    >
      <CircleCheck size={12} className="shrink-0 text-success" />
      <span className="min-w-0 flex-1 truncate font-medium text-fg-muted">Settled</span>
      <Pill size="sm" quiet onClick={() => void setThreadStatus(threadId, "idle")}>
        <CircleDashed size={11} /> Unsettle
      </Pill>
    </div>
  );
}

/**
 * A stack child of the composer's column. It is per child, not on the stack, so one of the
 * footer's controls can paint above the transcript while another stays under it. `over` lifts a
 * footer control — the composer with its drawer over it, an approval card, an error's actions —
 * above the rows the transcript draws.
 */
function LaneItem({ over = false, children }: { over?: boolean; children: ReactNode }) {
  return (
    <div
      // `empty:hidden`: a child that renders nothing (no linked threads, no report, no approval)
      // leaves no flex item behind, so the stack's gap does not turn into a phantom one.
      className={cx("empty:hidden", over && "relative z-30")}
    >
      {children}
    </div>
  );
}

function NewThreadHeading({
  project,
  banner,
  showTip,
}: {
  project: string | null;
  banner: boolean;
  showTip: boolean;
}) {
  const [tip] = useState(() => {
    const tips = boundTips(useApp.getState().shortcuts);
    return tips[Math.floor(Math.random() * tips.length)];
  });
  const shortcut = useShortcutHint(tip?.command ?? "");
  return (
    <div className="flex min-w-0 max-w-full flex-col items-center gap-2 pb-2 text-center">
      {/* No pill behind the words: over a picture, a soft halo of the pane's colour carries them
          instead (index.css `.rc-text-over-image`), and over the pane itself nothing is needed. */}
      <h2
        className={cx(
          "min-w-0 max-w-full font-semibold text-title tracking-tight",
          banner && "rc-text-over-image",
        )}
      >
        {project ? (
          <>
            What should we build in{" "}
            <button
              type="button"
              onClick={() => void chooseFolder(false)}
              title={`Change folder (${displayPath(project)})`}
              className="-mx-1 inline-block max-w-full truncate rounded-md px-1 align-bottom text-fg hover:bg-fill"
            >
              {projectName(project)}
              <span className="sr-only"> (change folder)</span>
            </button>
            ?
          </>
        ) : (
          "What should we build?"
        )}
      </h2>
      {showTip && tip && shortcut && (
        <p className={cx("text-caption text-fg-muted", banner && "rc-text-over-image")}>
          Tip: {tip.text} with <Kbd>{shortcut}</Kbd>
        </p>
      )}
    </div>
  );
}

export function ChatView() {
  const selectedId = useApp((s) => s.selectedId);
  const projectCwd = useApp((s) => s.projectCwd);
  const pending = useApp((s) => s.pendingSetup);
  const thread = useApp((s) => (s.selectedId ? s.threads[s.selectedId] : undefined));
  const stackRef = useRef<HTMLDivElement>(null);
  const [stackHeight, setStackHeight] = useState(140);
  const [overview, setOverview] = useState(overviewExpanded);
  // The overview stays mounted for the length of its exit, so closing fades and drops it instead of
  // cutting it; once hidden it unmounts and costs nothing. `leaving` picks the exit over the entry.
  const [overviewMounted, setOverviewMounted] = useState(overviewExpanded);
  const [overviewLeaving, setOverviewLeaving] = useState(false);
  const toggleOverview = () => {
    const next = !overview;
    setOverviewExpanded(next);
    setOverview(next);
    if (next) {
      setOverviewLeaving(false);
      setOverviewMounted(true);
    } else if (motionAllowed()) {
      setOverviewLeaving(true);
    } else {
      // Nothing to play where motion is off: the overview is simply gone.
      setOverviewMounted(false);
    }
  };
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
    const measure = () => {
      setWidth(el.offsetWidth);
      setHeight(el.offsetHeight);
    };
    measure();
    return observeSize(el, measure);
  }, []);

  // The message's arrival on the new-thread screen (`rc-send-in`) is 160ms, and a session can exist
  // in a single frame: the thread must not take the screen over mid-entry, or the entry is cut to
  // one frame that reads as no motion at all. The screen it keeps is the same picture — the same row,
  // in the same place, at rest — so the hand-over itself moves nothing; a harness slower than the
  // entry is already past it and takes the screen the moment its session exists.
  const [arrived, setArrived] = useState(true);
  const shownSetup = useRef<PendingSetup | null>(null);
  // The entry's clock belongs to the setup, not to its lifetime: it must not be stopped when the
  // session exists, because that is the moment the hold begins. (Stopping it there — a cleanup on a
  // `pending` dependency — is what left the chat on `Starting…` for good: the session, the sidebar
  // row and the turn all arrived, and the screen that was waiting for the entry never let go.) A new
  // setup, a retry or the view going away is what ends a clock.
  const entryTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  if (pending) shownSetup.current = pending;
  // biome-ignore lint/correctness/useExhaustiveDependencies: keyed on the setup's id; its clock runs on after it
  useEffect(() => {
    if (!pending) return;
    setArrived(false);
    clearTimeout(entryTimer.current);
    // Nothing to wait for where motion is off: the entry is instant, so the thread may be too.
    entryTimer.current = setTimeout(() => setArrived(true), motionAllowed() ? PENDING_ENTRY_MS : 0);
  }, [pending?.id]);
  useEffect(() => () => clearTimeout(entryTimer.current), []);
  // Which thread the setup became: the one selected as the setup went. A later selection (the reader
  // picking another thread in the sidebar) ends any hold at once, so the screen never keeps a thread
  // the reader has left.
  const became = useRef<string | null>(null);
  if (pending && !selectedId) became.current = null;
  if (!pending && selectedId && !became.current && shownSetup.current) became.current = selectedId;
  const held = heldSetup({
    pending,
    last: shownSetup.current,
    became: became.current,
    selectedId,
    hasThread: !!thread,
    arrived,
  });
  const setup = pending ?? held;
  // A thread setting up owns the new-thread screen until its session exists: the chat keeps its
  // message and the composer sits at the foot, where the thread's own will be.
  const showsPending = !!setup && (!selectedId || !!held);
  const isNew = !selectedId && !showsPending;
  // Toasts sit centred at the top of the chat column, under its header (ui/Toaster.tsx).
  // biome-ignore lint/correctness/useExhaustiveDependencies: re-place when the column resizes, which opening the right panel also does
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const r = root.getBoundingClientRect();
    const doc = document.documentElement.style;
    doc.setProperty("--rc-toast-center", `${Math.round(r.left + r.width / 2)}px`);
    doc.setProperty("--rc-toast-top", `${Math.round(r.top + HEADER_HEIGHT + 8)}px`);
  }, [isNew, stackHeight, width, height]);

  const ownRequest = thread?.permissions[0];
  // The approval just answered, which stands where its card was for a moment.
  const resolved = useApp((s) => (thread ? s.resolvedApprovals[thread.id] : undefined));
  const report = useApp((s) => s.report);
  const error = thread?.error && thread.error !== dismissed ? thread.error : undefined;
  // The lane's gutter sits outside the cap: `px-3 sm:px-12` on the transcript's scroller (its rows
  // carry the cap alone) and `inset-x-3 sm:inset-x-12` here, so the field's edges and a message's
  // edges are one line at every width.
  // The new-thread screen always has the picture; an open thread has it when Settings says so.
  const showsBanner = !!banner.name && (isNew || banner.inThreads);
  const veil = showsBanner ? darkenOverlay(banner.darken) : null;

  return (
    <main
      className="relative flex min-h-0 min-w-0 flex-1 flex-col bg-bg"
      aria-label="Chat"
      onMouseDown={releaseComposer}
    >
      {/* The chosen picture covers the whole pane, the header included: the header floats over the
          column, so the column's `height` is the pane's, and the screen is the picture all the way
          down without running past the window's bottom edge. */}
      {showsBanner && (
        <NewThreadBanner
          key={banner.style}
          name={banner.name}
          style={banner.style}
          height={Math.round(height)}
          blur={banner.blur}
          className="!absolute top-0 left-0 rc-fade-in"
        />
      )}
      {/* The Darken setting: black over the picture, under the pane's wash. */}
      {veil && (
        <div aria-hidden className="pointer-events-none absolute inset-0" style={{ background: veil }} />
      )}
      {/* One light scrim over it, so the field and the cards stay the readable thing: dark text on
          a light theme's pane wash, light text on the dark one's. A new thread shows the picture
          alone, so its wash is light; an open thread is read line by line over whatever picture was
          chosen, so its wash has to carry the text on its own. The white photo and the dark theme
          are the worst case (the black photo and the light theme are the mirror): at 60% the body
          text read at 4.4:1 and its muted lines at 2.0:1, where 80% keeps 9.4:1 and 4.3:1. A
          picture that should show more than that is a darker one, or the Darken slider. */}
      {showsBanner && (
        <div
          aria-hidden
          className={cx("pointer-events-none absolute inset-0", isNew ? "bg-bg/35" : "bg-bg/80")}
        />
      )}
      <Header thread={thread} pending={showsPending ? setup : undefined} overBanner={showsBanner} />
      {/*
        The chat column and the right panel share one row, so the panel is docked beside
        the chat rather than floating over it: the column is `flex-1` and narrows by the
        card's width, and the lane re-centres in what is left (`mx-auto` on the stack).
        The header floats above both, over the row.
      */}
      <div className="flex min-h-0 min-w-0 flex-1">
        <div ref={rootRef} className="relative flex min-h-0 min-w-0 flex-1 flex-col">
          <div
            className="pointer-events-none absolute inset-x-3 z-20 mx-auto max-w-[var(--rc-chat-content-max-width)] sm:inset-x-12"
            style={{ top: HEADER_HEIGHT + 8 }}
          >
            <WhileAwayCard className="pointer-events-auto" />
          </div>
          {showsPending && setup ? (
            // Preferred over the thread: while its message is still arriving, the screen that has it
            // is this one, and the row it draws is the row the transcript draws (`FIRST_ROW_PAD`).
            <PendingSetupView pending={setup} />
          ) : thread && !thread.loading ? (
            // Keyed: each thread gets a fresh virtualizer that starts at its end (Transcript.tsx).
            <Transcript
              key={thread.id}
              thread={thread}
              bottomInset={stackHeight + 32}
              topInset={HEADER_HEIGHT}
            />
          ) : thread?.loading ? (
            <TranscriptSkeleton />
          ) : (
            <div className="flex-1" />
          )}
          {/* The transcript fades out at the column's bottom edge, behind the composer: the
              card's glass blurs the text behind it, while this short fade stops text reading
              against the window's very edge. */}
          {!isNew && <div className="rc-composer-fade pointer-events-none absolute inset-x-0 bottom-0" />}
          <div
            ref={stackRef}
            // Centred by `mx-auto` and not by a transform, so its children can set their own z-index
            // (LaneItem). An empty thread sits half the stack up from the middle of the column, measured.
            style={{
              transition: "bottom 500ms var(--ease-snappy)",
              ...(isNew ? { bottom: `max(16px, calc(50% - ${Math.round(stackHeight / 2)}px))` } : undefined),
            }}
            className={cx(
              "absolute inset-x-3 bottom-4 mx-auto flex flex-col gap-2 sm:inset-x-12",
              // An empty thread's cards are a dashboard, not a lane: they take the wider measure
              // (up to 1100px) while the field and everything over it keep the chat's own.
              isNew ? "max-w-[1100px]" : "max-w-[var(--rc-chat-content-max-width)]",
            )}
          >
            <div className="mx-auto flex w-full max-w-[var(--rc-chat-content-max-width)] flex-col gap-2">
              {isNew && (
                <NewThreadHeading
                  project={projectCwd}
                  banner={showsBanner}
                  // The overview's cards scroll in the room below the field and never move it, so
                  // the tip above the field has its height either way: it stays while it is open.
                  showTip={height >= NEW_THREAD_WIDGETS_MIN_HEIGHT}
                />
              )}
              {error && (
                <LaneItem over>
                  <ErrorBanner
                    key={error}
                    text={error}
                    detail={thread?.errorDetail}
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
                </LaneItem>
              )}
              {thread && (
                <LaneItem over>
                  <LinkedThreads threadId={thread.id} />
                </LaneItem>
              )}
              {report && report.threadId === thread?.id && (
                <LaneItem over>
                  <ReportPanel text={report.text} />
                </LaneItem>
              )}
              {ownRequest && (
                <LaneItem over>
                  <AttentionCard
                    request={ownRequest}
                    pending={thread?.permissions.filter((p) => !p.question).length ?? 1}
                    project={thread?.cwd}
                  />
                </LaneItem>
              )}
              {!ownRequest && resolved && (
                <LaneItem over>
                  <ResolvedApprovalBar approval={resolved} />
                </LaneItem>
              )}
              {thread && (
                <LaneItem over>
                  <SettledBar threadId={thread.id} />
                </LaneItem>
              )}
              <LaneItem over>
                {/* Over a new thread's picture the field is a step more opaque than its usual
                    glass, so it reads as a panel rather than as another window on the picture. */}
                <Composer
                  thread={thread}
                  compact={width < 600}
                  className={isNew ? "rc-composer-over" : undefined}
                />
              </LaneItem>
            </div>
          </div>
          {isNew && height >= NEW_THREAD_WIDGETS_MIN_HEIGHT && (
            <div
              className="absolute inset-x-3 bottom-0 mx-auto flex max-w-[1100px] flex-col items-center gap-2 pt-2 sm:inset-x-12"
              style={{ top: `calc(50% + ${Math.round(stackHeight / 2)}px)` }}
            >
              <button
                type="button"
                aria-expanded={overview}
                aria-controls="new-thread-overview"
                className={cx(
                  "flex shrink-0 items-center gap-1 rounded-full px-2 py-1 text-caption text-fg-muted",
                  // Over a picture the label needs the floating pills' glass to read.
                  showsBanner ? "rc-glass hover:text-fg" : "hover:bg-fill-subtle",
                )}
                onClick={toggleOverview}
              >
                <ChevronDown
                  size={14}
                  aria-hidden
                  className={cx("transition-transform duration-200 ease-snappy", overview && "rotate-180")}
                />
                {overview ? "Hide overview" : "Show overview"}
              </button>
              {overviewMounted && (
                <div
                  id="new-thread-overview"
                  aria-hidden={!overview}
                  className={cx(
                    "min-h-0 w-full overflow-y-auto pb-3",
                    overviewLeaving ? "rc-overview-out" : "rc-overview-in",
                  )}
                  onAnimationEnd={(event) => {
                    // A widget's own animation bubbles up here too; only this box's exit ends it.
                    if (event.target !== event.currentTarget || !overviewLeaving) return;
                    setOverviewLeaving(false);
                    setOverviewMounted(false);
                  }}
                >
                  <NewThreadWidgets project={projectCwd} />
                </div>
              )}
            </div>
          )}
        </div>
        <RightPanel />
      </div>
    </main>
  );
}
