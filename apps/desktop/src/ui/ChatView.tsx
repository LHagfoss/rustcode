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
  type LucideIcon,
  PanelRight,
  SquareTerminal,
} from "lucide-react";
import { DropdownMenu, Popover } from "radix-ui";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { displayPath, projectName } from "../acp/types";
import { STATUS_LABEL } from "../acp/v2";
import { useBanner } from "../lib/banner";
import { CHAT_WIDTH_REM } from "../lib/chatWidth";
import { pickFolder } from "../lib/native";
import { observeSize } from "../lib/resize";
import { chooseFolder } from "../state/folders";
import type { HeaderButtonId } from "../state/headerButtons";
import type { PendingSetup } from "../state/setup";
import {
  dismissPendingSetup,
  refreshSessions,
  repairThread,
  setPanel,
  setProjectCwd,
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
import { AttentionCard } from "./AttentionCards";
import { Composer } from "./Composer";
import { HarnessIcon } from "./HarnessIcon";
import { LinkedThreads } from "./Links";
import { HEADER_HEIGHT, MainHeader } from "./MainHeader";
import { NewThreadBanner } from "./NewThreadBanner";
import { ProjectMark } from "./ProjectMark";
import {
  Callout,
  Cluster,
  ClusterButton,
  cx,
  menuItem,
  menuSeparator,
  Pill,
  Skeleton,
  Spinner,
  Tooltip,
} from "./primitives";
import { ReportPanel } from "./ReportPanel";
import { DETAILS_CARD } from "./RightPanel";
import { UserRow } from "./Rows";
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
          <DropdownMenu.Item className={menuItem} onSelect={() => setView({ kind: "review", threadId })}>
            <FileDiff size={13} /> Review Changes
          </DropdownMenu.Item>
          <DropdownMenu.Separator className={menuSeparator} />
          <DropdownMenu.Item className={menuItem} onSelect={() => void setThreadStatus(threadId, "review")}>
            <Eye size={13} /> Mark for Review
          </DropdownMenu.Item>
          <DropdownMenu.Item className={menuItem} onSelect={() => settleThread(threadId)}>
            <CircleCheck size={13} /> Mark Done
          </DropdownMenu.Item>
          <DropdownMenu.Item className={menuItem} onSelect={() => void setThreadStatus(threadId, "idle")}>
            <CircleDashed size={13} /> Clear Status
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
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

function DetailsHeaderButton() {
  const on = useApp((s) => s.panel === "details");
  return (
    <ClusterButton
      label="Thread details"
      tooltip="Thread details"
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
  // The buttons the user keeps, in their order. Each one decides whether this
  // thread shows it, and the icon buttons share one cluster wherever they land.
  const right: ReactNode[] = [];
  let cluster: ReactNode[] = [];
  const closeCluster = () => {
    if (!cluster.length) return;
    right.push(<Cluster key={`cluster-${right.length}`}>{cluster}</Cluster>);
    cluster = [];
  };
  for (const id of shown) {
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
        </>
      }
      right={right.length ? right : undefined}
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
      'button, a, input, textarea, select, label, [role="button"], [contenteditable], .rc-composer, .rc-composer-tray, .rc-composer-drawer',
    )
  )
    return;
  active.blur();
}

/** The chat lane's cap at its default setting: `--rc-chat-content-max-width` (52rem). */
const LANE_MAX = 52 * 16;

/**
 * How far the lane moves left to clear the details card, and no further than
 * the room beside it: the card floats over the pane's end, so a lane that
 * would run under it is shifted, never narrowed. When the pane has no room to
 * give, the lane stays put and the card overlays it.
 */
export function laneShift(pane: number, card = DETAILS_CARD, laneMax = LANE_MAX): number {
  if (pane <= 0) return 0;
  const lane = Math.min(laneMax, pane);
  const needed = (lane - pane) / 2 + card.width + card.gap * 2;
  const slack = Math.max(0, (pane - lane) / 2);
  return Math.round(Math.max(0, Math.min(needed, slack)));
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
 * The message a thread was sent with, while its worktree is made: the same user row the transcript
 * draws, then Setting up with its clock. Failed, the error takes its place with the way to dismiss.
 */
function PendingSetupView({ pending }: { pending: PendingSetup }) {
  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div
        className="mx-auto flex w-full max-w-[var(--rc-chat-content-max-width)] flex-col gap-3.5 px-3 pb-4 sm:px-5"
        style={{ paddingTop: HEADER_HEIGHT + 20 }}
      >
        <UserRow item={{ kind: "user", id: pending.id, text: pending.text || pending.title }} />
        {pending.failure ? (
          <Callout action={<Pill onClick={dismissPendingSetup}>Dismiss</Pill>}>
            {pending.failure.text}
          </Callout>
        ) : (
          <div className="flex items-center gap-2 text-body text-fg-muted" role="status">
            <Spinner size={14} />
            <span>Setting up worktree…</span>
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
 * A stack child that rides the lane's shift clear of the details card (`--rc-lane-inset-end`). It
 * is per child, not on the stack, because a transform would make one stacking context of the hero
 * and the footer: the composer's slash drawer has to paint above a floating right panel (z-20)
 * while a new thread's hero stays under it. `over` lifts a footer control — the composer with the
 * drawer over it, an approval card, an error's actions — above that panel.
 */
function LaneItem({ over = false, children }: { over?: boolean; children: ReactNode }) {
  return (
    <div
      // `empty:hidden`: a child that renders nothing (no linked threads, no report, no approval)
      // leaves no flex item behind, so the stack's gap does not turn into a phantom one.
      className={cx("empty:hidden", over && "relative z-30")}
      style={{
        translate: "calc(var(--rc-lane-inset-end, 0px) * -1)",
        transition: "translate 200ms var(--ease-snappy)",
      }}
    >
      {children}
    </div>
  );
}

export function ChatView() {
  const selectedId = useApp((s) => s.selectedId);
  const pending = useApp((s) => s.pendingSetup);
  const thread = useApp((s) => (s.selectedId ? s.threads[s.selectedId] : undefined));
  const stackRef = useRef<HTMLDivElement>(null);
  const [stackHeight, setStackHeight] = useState(140);
  const [dismissed, setDismissed] = useState<string | undefined>();
  const [width, setWidth] = useState(1000);
  const [height, setHeight] = useState(800);
  const banner = useBanner();
  const detailsOpen = useApp((s) => s.panel === "details");
  // The transcript's rows and the composer both read this in ui/Transcript.tsx and below.
  const chatWidth = useApp((s) => s.chatWidth);
  // "Full" hands the lane the pane and the details card overlays it, as it does on a narrow one.
  const laneRem = CHAT_WIDTH_REM[chatWidth];
  const laneInset = detailsOpen ? laneShift(width, DETAILS_CARD, laneRem === null ? width : laneRem * 16) : 0;
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
    // Measured once on mount as well as on resize: the lane clears the details
    // card from the first frame, not only after the first resize.
    const measure = () => {
      setWidth(el.offsetWidth);
      setHeight(el.offsetHeight);
    };
    measure();
    return observeSize(el, measure);
  }, []);

  // A thread setting up owns the new-thread screen until its session exists: the chat keeps its
  // message and the composer sits at the foot, where the thread's own will be.
  const showsPending = !!pending && !selectedId;
  const isNew = !selectedId && !showsPending;

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

  // How far the composer (its box and tray) reaches up from the foot of the pane: the right panel
  // floats over the transcript and stops above it (ui/RightPanel.tsx). Measured on the composer
  // rather than the stack, whose hero fills an empty thread, and from the foot of the column the
  // panel is placed in — not this chat's own, which the terminal shortens from below. Its own size
  // covers a drawer opening; the stack's `bottom` transition, an empty thread settling into a
  // thread, republishes at its end, and so does the chat column resizing.
  useEffect(() => {
    const root = rootRef.current;
    const stack = stackRef.current;
    const composer = stack?.querySelector<HTMLElement>(".rc-composer-shell");
    if (!root || !stack || !composer) return;
    const doc = document.documentElement.style;
    const publish = () => {
      // The chat is `relative`, so its offset parent is the column that holds it and the terminal:
      // the same box the floating panel is positioned against, whose foot the terminal moves.
      const host = root.offsetParent instanceof HTMLElement ? root.offsetParent : root;
      const foot = host.getBoundingClientRect().bottom;
      doc.setProperty("--rc-composer-top", `${Math.round(foot - composer.getBoundingClientRect().top)}px`);
    };
    publish();
    stack.addEventListener("transitionend", publish);
    const stops = [observeSize(composer, publish), observeSize(stack, publish), observeSize(root, publish)];
    // No chat column, no composer to stop above: a floating panel takes the pane height again.
    return () => {
      for (const stop of stops) stop();
      stack.removeEventListener("transitionend", publish);
      doc.removeProperty("--rc-composer-top");
    };
  }, []);
  const ownRequest = thread?.permissions[0];
  const report = useApp((s) => s.report);
  const error = thread?.error && thread.error !== dismissed ? thread.error : undefined;
  // The lane's gutter is `px-3 sm:px-5` on the row itself, the same classes the transcript's rows use,
  // so the field's edges and a message's edges line up at every width.
  const showsBanner = isNew && !!banner.name;

  return (
    <main
      ref={rootRef}
      className="relative flex min-h-0 min-w-0 flex-1 flex-col bg-bg"
      aria-label="Chat"
      onMouseDown={releaseComposer}
      style={{ "--rc-lane-inset-end": `${laneInset}px` } as React.CSSProperties}
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
      <Header thread={thread} pending={showsPending ? pending : undefined} overBanner={showsBanner} />
      {thread && !thread.loading ? (
        // Keyed: each thread gets a fresh virtualizer that starts at its end (Transcript.tsx).
        <Transcript key={thread.id} thread={thread} bottomInset={stackHeight + 32} topInset={HEADER_HEIGHT} />
      ) : thread?.loading ? (
        <TranscriptSkeleton />
      ) : showsPending && pending ? (
        <PendingSetupView pending={pending} />
      ) : (
        <div className="flex-1" />
      )}
      {/* The transcript scrolls the whole pane height and passes under the composer's glass.
          The strip the tray sits on is scrimmed, so small footer labels never fight the text
          behind them; the box above it keeps the text coming through. */}
      {!isNew && (
        <div
          className="pointer-events-none absolute inset-x-0 bottom-0 h-[72px]"
          style={{ background: "linear-gradient(to bottom, transparent 0, var(--rc-bg) 16px)" }}
        />
      )}
      <div
        ref={stackRef}
        // Centred by `mx-auto` and not by a transform, so its children can set their own z-index
        // (LaneItem). An empty thread sits half the stack up from the middle of the pane, measured.
        style={{
          transition: "bottom 500ms var(--ease-snappy)",
          ...(isNew ? { bottom: `max(16px, calc(50% - ${Math.round(stackHeight / 2)}px))` } : undefined),
        }}
        className="absolute inset-x-0 bottom-4 mx-auto flex w-full max-w-[var(--rc-chat-content-max-width)] flex-col gap-2.5 px-3 sm:px-5"
      >
        {isNew && (
          <LaneItem>
            <EmptyState banner={showsBanner} />
          </LaneItem>
        )}
        {error && (
          <LaneItem over>
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
            />
          </LaneItem>
        )}
        <LaneItem over>
          <Composer thread={thread} compact={width < 600} />
        </LaneItem>
      </div>
    </main>
  );
}
