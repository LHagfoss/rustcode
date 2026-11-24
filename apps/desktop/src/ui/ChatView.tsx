import { isLoading, openError } from "../state/thread";
// The chat view: the header (project / title, plan, usage, panels), then one row
// holding the chat column — transcript and composer stack with error and
// approval cards — and the right panel docked beside it (ui/RightPanel.tsx). A
// new thread centres the composer and places its optional overview below it.

import { Popover } from "radix-ui";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { displayPath, projectName } from "../acp/types";
import { darkenOverlay, useBanner } from "../lib/banner";
import { observeSize } from "../lib/resize";
import { motionAllowed, PENDING_ENTRY_MS } from "../lib/sendFlight";
import { composedText, emptyExtras, sentAttachments } from "../state/compose";
import { contextPercent } from "../state/details";
import { chooseFolder } from "../state/folders";
import type { HeaderButtonId } from "../state/headerButtons";
import { offlineNotice } from "../state/machineCache";
import { hasPendingChange, useOutbox } from "../state/machineOutbox";
import { offlineSinceLabel, useMachineReconnecting, useMachines } from "../state/machines";
import { heldSetup, type PendingSetup, pendingStatus } from "../state/setup";
import {
  dismissPendingSetup,
  harnessesOn,
  openSettings,
  refreshSessions,
  repairThread,
  retryPendingSetup,
  runPrompt,
  seeFailure,
  setThreadStatus,
  statusOf,
  useApp,
  useMachineOfThread,
  useShortcutHint,
  useThreadOffline,
} from "../state/store";
import { formatClock, isRunning, type ThreadState } from "../state/thread";
import { retryStalledFirstMessage, retryThread } from "../state/threadActions";
import { splitThreadKey } from "../state/threadKeys";
import { failureHarness, turnErrorAdvice } from "../state/turnError";
import { overviewExpanded, setOverviewExpanded } from "../state/widgets";
import { AttentionCard, ResolvedApprovalBar } from "./AttentionCards";
import { OriginChip } from "./Branches";
import { NewThreadLight, ThreadLight } from "./ChatLight";
import { Composer } from "./Composer";
import { DelegateStatusPanel } from "./DelegateStatus";
import {
  ChevronDown,
  CircleCheck,
  CircleDashed,
  CircleDot,
  ListChecks,
  type LucideIcon,
  PanelRight,
} from "./icons";
import { JobRunBar, useJobRun } from "./JobRunBar";
import { LinkedThreads } from "./Links";
import { HEADER_HEIGHT, MainHeader } from "./MainHeader";
import { NewThreadBanner } from "./NewThreadBanner";
import { NEW_THREAD_WIDGETS_MIN_HEIGHT, NewThreadWidgets } from "./NewThreadWidgets";
import { boundTips } from "./newThreadTips";
import { PANEL_TOGGLE_LABEL, PanelToggle, useCardOverHeader } from "./PanelToggle";
import { ProjectMark } from "./ProjectMark";
import { ProjectPicker, useMoveBlockedReason } from "./ProjectPicker";
import { Callout, cx, Kbd, Pill, Skeleton, Spinner, surfaceCollisionPadding, Tooltip } from "./primitives";
import { RemoteThreadIndicator } from "./RemoteAccessPage";
import { ReportPanel } from "./ReportPanel";
import { RightPanel } from "./RightPanel";
import { RollingNumber } from "./RollingNumber";
import { UserRow } from "./Rows";
import { useThreadStatus } from "./Status";
import { ThreadTitle } from "./ThreadTitle";
import { FIRST_ROW_PAD, Transcript } from "./Transcript";

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

// The header's buttons, one entry per id in state/headerButtons.ts: the label
// and icon Settings shows, and the button itself, which decides on its own
// whether this thread has anything to show (a plan, a pull requests view). The
// record is typed by id, so a new button in that list does not compile without
// its entry here, and a user's order only ever names an id.

interface HeaderButtonEntry {
  label: string;
  icon: LucideIcon;
  Button: (props: { thread?: ThreadState }) => ReactNode;
}

/** The plan's progress, while there is a plan. */
function PlanHeaderButton({ thread }: { thread?: ThreadState }) {
  return thread ? <PlanButton thread={thread} /> : null;
}

/**
 * The side panel's toggle, "Thread info". While the panel is open the card rises over the header
 * and carries the toggle itself (ui/PanelToggle.tsx), so the header leaves it out.
 */
function DetailsHeaderButton() {
  const open = useApp((s) => !!s.panel);
  const overHeader = useCardOverHeader();
  return open && overHeader ? null : <PanelToggle />;
}

export const HEADER_BUTTONS: Record<HeaderButtonId, HeaderButtonEntry> = {
  plan: { label: "Plan", icon: ListChecks, Button: PlanHeaderButton },
  details: { label: PANEL_TOGGLE_LABEL, icon: PanelRight, Button: DetailsHeaderButton },
};

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
  const session = useApp((s) => (thread ? s.sessions.find((x) => x.id === thread.id) : undefined));
  const listed = session?.title;
  const machineHost = thread ? splitThreadKey(thread.id).host : "";
  const title = pending?.title ?? (thread ? (thread.title ?? listed ?? "New thread") : "New thread");
  const cwd = pending?.cwd ?? thread?.cwd ?? projectCwd;
  const newThreadHost = useApp((s) => s.newThreadHost);
  const moveBlocked = useMoveBlockedReason(pending ? undefined : thread);
  // The buttons the user keeps, with the panel's toggle last: the panel is the last thing beside
  // the chat, so its control is the rightmost one whatever order the user saved. Each one decides
  // whether this thread shows it. They stand on their own, without a shared pill behind them.
  const kept = [...shown.filter((id) => id !== "details"), ...shown.filter((id) => id === "details")];
  const right = kept.map((id) => {
    const Button = HEADER_BUTTONS[id].Button;
    return <Button key={id} thread={thread} />;
  });
  return (
    <MainHeader
      // Glass like every header; over a new thread's banner a lighter tint, so the picture shows.
      className={overBanner ? "rc-frost" : undefined}
      title={
        <>
          {cwd && (
            // The app's own project picker (ui/ProjectPicker.tsx), the same panel as the folder
            // button under the composer: it moves this thread to another project, or picks where
            // a new one starts. ⌘O still opens the system's folder dialog.
            <ProjectPicker
              thread={pending ? undefined : thread}
              cwd={cwd}
              host={thread ? machineHost : newThreadHost}
              side="bottom"
              align="start"
              tooltip={
                moveBlocked
                  ? `${displayPath(cwd)} · ${moveBlocked}`
                  : `${displayPath(cwd)} · change project (${folderKey})`
              }
              trigger={
                <button
                  type="button"
                  aria-label={`Project: ${projectName(cwd)}`}
                  className="-mx-1 flex h-7 max-w-[220px] shrink-0 items-center gap-1.5 rounded-sm px-2 text-body text-fg-muted hover:bg-fill-subtle hover:text-fg data-[state=open]:bg-fill-subtle data-[state=open]:text-fg"
                >
                  <ProjectMark name={projectName(cwd)} />
                  <span className="min-w-0 truncate">{projectName(cwd)}</span>
                </button>
              }
            />
          )}
          {cwd && (
            <span aria-hidden className="text-body text-fg-faint">
              /
            </span>
          )}
          <ThreadTitle thread={pending ? undefined : thread} title={title} />
          {/* A fork, a side chat or a delegate links back to the thread it came from. */}
          {thread && <OriginChip threadId={thread.id} />}
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

/** The harness a thread runs on, by name: the one its failure names first, else its own. */
function failedHarnessName(thread: ThreadState): string {
  const named = failureHarness(thread.errorDetail ?? openError(thread) ?? thread.error);
  if (named) return named;
  const id = thread.harness?.id;
  const known = harnessesOn(useApp.getState(), splitThreadKey(thread.id).host)?.harnesses.find(
    (h) => h.id === id,
  );
  return known?.name ?? "The agent";
}

/** A refused login said plainly (`Codex is signed out.`), the raw text kept under Details. */
function signedOutText(thread: ThreadState): string | undefined {
  const advice = turnErrorAdvice(
    thread.errorDetail ?? openError(thread) ?? thread.error,
    contextPercent(thread.usage),
  );
  return advice.signedOut ? `${failedHarnessName(thread)} is signed out.` : undefined;
}

/**
 * The fixes a failed turn offers beside Details (docs/design.md: an error offers the action that
 * fixes it): Sign in opens Settings › Agents, where the harness signs in; Compact runs `/compact`;
 * Try again sends the failed message again. Only those that can work for this failure, and none
 * while a turn runs.
 */
function ErrorFixes({ thread }: { thread: ThreadState }) {
  if (isRunning(thread)) return null;
  const { fixes } = turnErrorAdvice(
    thread.errorDetail ?? openError(thread) ?? thread.error,
    contextPercent(thread.usage),
  );
  if (!fixes.length) return null;
  const first = fixes[0];
  return (
    <>
      {fixes.includes("sign_in") && (
        <Pill prominent={first === "sign_in"} onClick={() => openSettings("agents")}>
          Sign in
        </Pill>
      )}
      {fixes.includes("compact") && (
        <Pill
          prominent={first === "compact"}
          onClick={() => void runPrompt(thread.id, "/compact", emptyExtras)}
        >
          Compact
        </Pill>
      )}
      {fixes.includes("retry") && (
        <Pill prominent={first === "retry"} onClick={() => void retryThread(thread.id)}>
          Try again
        </Pill>
      )}
    </>
  );
}

/** Where an opening thread's transcript will be: a sent message, then reply lines, twice. */
function StalledFirstMessage({ thread }: { thread: ThreadState }) {
  const first = thread.items.find((item) => item.kind === "user");
  const [retrying, setRetrying] = useState(false);
  return (
    <LaneItem over>
      <Callout
        floating
        action={
          first && (
            <button
              type="button"
              className="font-medium text-accent"
              disabled={retrying}
              onClick={() => {
                setRetrying(true);
                void retryStalledFirstMessage(thread.id, first.id).finally(() => setRetrying(false));
              }}
            >
              Retry first message
            </button>
          )
        }
      >
        The first message received no response.
      </Callout>
    </LaneItem>
  );
}

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
    <div className="rc-lane-pad absolute inset-0 overflow-y-auto overscroll-contain [overflow-anchor:none] [scrollbar-gutter:stable_both-edges]">
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
                <Pill onClick={() => dismissPendingSetup()}>Dismiss</Pill>
              </>
            }
          >
            {pending.failure.text}
          </Callout>
        ) : (
          <div className="flex items-center gap-2 text-body text-fg-muted" role="status">
            <Spinner size={14} />
            {pendingStatus(pending) && <span>{pendingStatus(pending)}</span>}
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
  // A scheduled job's run settles on its own; its own bar says so.
  const jobRun = useJobRun(threadId);
  if (jobRun) return <JobRunBar threadId={threadId} />;
  if (status !== "done") return null;
  return (
    <div
      role="status"
      className="rc-glass-overlay relative flex h-[32px] items-center gap-2 rounded-full pr-1.5 pl-3 text-callout rc-in"
    >
      <CircleCheck size={12} className="shrink-0 text-success" />
      <span className="min-w-0 flex-1 truncate font-medium text-fg-muted">Settled</span>
      <Pill size="sm" quiet onClick={() => void setThreadStatus(threadId, "idle")}>
        <CircleDashed size={12} /> Unsettle
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
    <div
      className={cx(
        "relative isolate flex min-w-0 max-w-full flex-col items-center gap-2 pb-2 text-center",
        banner && "rc-heading-over-image",
      )}
    >
      <h2
        className={cx(
          "min-w-0 max-w-full font-semibold text-title tracking-tight",
          banner && "rc-text-over-image",
        )}
      >
        {project ? (
          <>
            What should we build in{" "}
            <Tooltip label={`Change folder (${displayPath(project)})`}>
              <button
                type="button"
                onClick={() => void chooseFolder(false)}
                className="-mx-1 inline-block max-w-full truncate rounded-md px-1 align-bottom text-fg hover:bg-fill"
              >
                {projectName(project)}
                <span className="sr-only"> (change folder)</span>
              </button>
            </Tooltip>
            ?
          </>
        ) : (
          "What should we build?"
        )}
      </h2>
      {showTip && tip && shortcut && (
        // Over a picture the tip reads in `fg`, not muted: a muted line lands near what a dark
        // photo leaves under the scrim, and the halo alone cannot make that difference up.
        <p className={cx("text-caption", banner ? "rc-text-over-image-small text-fg" : "text-fg-muted")}>
          Tip: {tip.text} with <Kbd>{shortcut}</Kbd>
        </p>
      )}
    </div>
  );
}

export function ChatView() {
  const selectedId = useApp((s) => s.selectedId);
  const stalled = useApp((s) => !!s.selectedId && statusOf(s, s.selectedId) === "stalled");
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
  const laneRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = stackRef.current;
    if (!el) return;
    setStackHeight(el.offsetHeight);
    return observeSize(el, () => setStackHeight(el.offsetHeight));
  }, []);

  useEffect(() => {
    // The lane's box, not the column's: the right panel's card floats over the column and takes
    // its room from the lane, which is what the composer and the toasts are sized and centred on.
    const el = laneRef.current;
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
  // A thread whose computer is away: read-only, and the lane says so before the field does.
  const offline = useThreadOffline(thread?.id);
  const offMachine = useMachineOfThread(thread?.id);
  // Away but already on its supervisor's retry clock: read-only still, and coming back by itself.
  const offReconnecting = useMachineReconnecting(thread ? splitThreadKey(thread.id).host : undefined);
  // When it went away, and whether this computer holds a copy of the thread to read meanwhile.
  // A settle or pin made here meanwhile, waiting for the machine (state/machineOutbox.ts).
  const offUnsynced = useOutbox((o) => !!thread && hasPendingChange(o.byHost, thread.id));
  const offSince = useMachines((s) =>
    thread ? offlineSinceLabel(s.offlineSince[splitThreadKey(thread.id).host]) : undefined,
  );
  const ownRequest = thread?.permissions[0];
  // The approval just answered, which stands where its card was for a moment.
  const resolved = useApp((s) => (thread ? s.resolvedApprovals[thread.id] : undefined));
  const report = useApp((s) => s.report);
  const failure = openError(thread) ?? thread?.error;
  const error = failure && failure !== dismissed ? failure : undefined;
  // The lane's gutter sits outside the cap: `rc-lane-pad` on the transcript's scroller (its rows
  // carry the cap alone) and `rc-lane-inset` here, so the field's edges and a message's edges are
  // one line at every width, with the right panel's card open or not (index.css).
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
          down without running past the window's bottom edge. An open thread shows it at the
          Visibility setting, which is the wash of the pane colour over it put the other way round. */}
      {showsBanner && (
        <div
          aria-hidden
          className="pointer-events-none absolute inset-0"
          style={isNew ? undefined : { opacity: banner.visibility / 100 }}
        >
          <NewThreadBanner
            key={banner.style}
            name={banner.name}
            style={banner.style}
            height={Math.round(height)}
            blur={banner.blur}
            className="!absolute top-0 left-0 rc-fade-in"
          />
          {/* The Darken setting: black over the picture, under the pane's wash. */}
          {veil && <div className="absolute inset-0" style={{ background: veil }} />}
        </div>
      )}
      {/* One light scrim over a new thread's picture, so the field and the cards stay the readable
          thing: dark text on a light theme's pane wash, light text on the dark one's. An open thread
          is read line by line over whatever picture was chosen, so there the picture itself is
          faded instead (Visibility, 20% by default): the white photo and the dark theme are the
          worst case (the black photo and the light theme are the mirror), and a 20% picture, the
          80% wash first measured, keeps body text at 9.4:1 and muted lines at 4.3:1 where 40%
          left 4.4:1 and 2.0:1. A picture that should show more than that is a darker one, or the
          Darken slider. */}
      {showsBanner && isNew && <div aria-hidden className="pointer-events-none absolute inset-0 bg-bg/35" />}
      {/* The live background (Settings › Appearance › Live background): the whole pane behind a new
          thread, kept while its first message sets the thread up so the send's wave plays out. A
          chosen picture takes its place. */}
      {!showsBanner && (isNew || (showsPending && !thread)) && <NewThreadLight />}
      <Header thread={thread} pending={showsPending ? setup : undefined} overBanner={showsBanner} />
      {/*
        The right panel's card floats over the chat's row: the column keeps the row's whole
        width, so the transcript's scrollbar stays at the window's edge, and the lane pads by
        the room the card takes (`--rc-panel-room`) and re-centres in what is left.
        The header floats above both, over the row.
      */}
      <div className="relative flex min-h-0 min-w-0 flex-1">
        <div className="relative flex min-h-0 min-w-0 flex-1 flex-col">
          {/* The part of the column that is the lane's, for its width and centre. */}
          <div
            ref={laneRef}
            aria-hidden
            className="rc-lane-room pointer-events-none absolute inset-y-0 left-0"
          />
          {thread && !isNew && !showsPending && (
            <ThreadLight threadId={thread.id} stack={stackRef} stackHeight={stackHeight} />
          )}
          {showsPending && setup ? (
            // Preferred over the thread: while its message is still arriving, the screen that has it
            // is this one, and the row it draws is the row the transcript draws (`FIRST_ROW_PAD`).
            <PendingSetupView pending={setup} />
          ) : thread && !isLoading(thread) ? (
            // Keyed: each thread gets a fresh virtualizer that starts at its end (Transcript.tsx).
            <Transcript
              key={thread.id}
              thread={thread}
              bottomInset={stackHeight + 32}
              topInset={HEADER_HEIGHT}
            />
          ) : isLoading(thread) ? (
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
            // Its `bottom` eases in CSS (`rc-lane-stack`), not inline, so opening the right panel can
            // ease its `right` with the rest of the lane.
            style={isNew ? { bottom: `max(16px, calc(50% - ${Math.round(stackHeight / 2)}px))` } : undefined}
            className={cx(
              "rc-lane-inset rc-lane-stack absolute bottom-4 mx-auto flex flex-col gap-2",
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
              {stalled && thread && !offline && <StalledFirstMessage thread={thread} />}
              {thread && offline && (
                <LaneItem over>
                  <Callout floating>
                    {/* The shell retries a machine that went away on its own, so a reconnecting
                        machine reads as coming back, not as a failure the user has to fix. */}
                    {offlineNotice({
                      machine: offMachine,
                      since: offSince,
                      reconnecting: offReconnecting,
                      hasCopy: !!thread.items.length,
                      unsynced: offUnsynced,
                    })}
                    {ownRequest
                      ? ownRequest.question
                        ? " It is waiting for your answer there."
                        : " It is waiting for your approval there."
                      : ""}
                  </Callout>
                </LaneItem>
              )}
              {error && (
                <LaneItem over>
                  <ErrorBanner
                    key={error}
                    text={(thread && signedOutText(thread)) ?? error}
                    detail={thread?.errorDetail}
                    action={
                      thread && error.startsWith("Could not open this thread") ? (
                        <Pill prominent onClick={() => void repairThread(thread.id)}>
                          Repair Thread
                        </Pill>
                      ) : thread ? (
                        <ErrorFixes thread={thread} />
                      ) : undefined
                    }
                    onDismiss={() => {
                      setDismissed(error);
                      // Dismissed here is seen: its "Needs you" row goes with it.
                      if (thread) seeFailure(thread.id);
                      void refreshSessions();
                    }}
                  />
                </LaneItem>
              )}
              {thread && (
                <LaneItem over>
                  <DelegateStatusPanel key={thread.id} threadId={thread.id} />
                  <LinkedThreads threadId={thread.id} />
                </LaneItem>
              )}
              {report && report.threadId === thread?.id && (
                <LaneItem over>
                  <ReportPanel text={report.text} />
                </LaneItem>
              )}
              {ownRequest && !offline && (
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
              className="rc-lane-inset absolute bottom-0 mx-auto flex max-w-[1100px] flex-col items-center gap-2 pt-2"
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
                  // Nothing on this box animates: it holds the cards' glass, and a fade or a slide
                  // here would take the blur with it. The classes pick the motion their contents
                  // play (index.css, `rc-overview`).
                  className={cx(
                    "min-h-0 w-full overflow-y-auto pb-3",
                    overviewLeaving ? "rc-overview-out" : "rc-overview-in",
                  )}
                  onAnimationEnd={(event) => {
                    // The motion runs inside the cards, whose animation ends here (index.css,
                    // `rc-overview`): only the exit's own end unmounts the box, and a widget's own
                    // animation is filtered out by its name.
                    if (!overviewLeaving || !event.animationName.startsWith("rc-overview")) return;
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
