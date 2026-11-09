import { fileManagerName, primaryModifier } from "../lib/platform";
// The sidebar: a toolbar clear of the traffic lights (the sidebar toggle only),
// the app's brand under it (its mark, name and version), then one list: pinned
// threads first, then every active thread (by project when asked), then the
// Working, Snoozed and Settled shelves at the foot, each folding on its own
// header. Threads are searched from ⌘K, which also holds the buttons that used
// to sit here (new thread, add project, settings, pull requests, usage).
// A row is one 32px line: the mark of where it runs, its title, and one trailing
// slot — `Needs you`, `Working 2m`, `Waiting`, `Failed`, `Review`, the time it
// wakes, or its age (state/sidebarRows.ts `rowStatus`), with the harness mark and
// a lone diff beside it. A thread with a branch, a diff, a pull request or a stack
// of delegates adds one quiet second line for them (48px); a request from the
// thread adds its words under the title. The row layout preference (Settings ›
// Appearance) keeps the older two-line row as `comfortable`, which puts the title
// first and the branch under it.

import {
  Check,
  ChevronDown,
  ChevronRight,
  CircleAlert,
  CircleCheck,
  CircleDashed,
  Clock,
  Copy,
  Ellipsis,
  Eye,
  FileDiff,
  FileText,
  Folder,
  FolderOpen,
  FolderX,
  GitBranch,
  GitFork,
  GitMerge,
  GitPullRequestArrow,
  GitPullRequestClosed,
  GitPullRequestDraft,
  MessageCircleQuestion,
  MessageSquareQuote,
  PanelLeft,
  Pencil,
  Pin,
  PinOff,
  ShieldQuestion,
  Sparkles,
  SquarePen,
  SquareTerminal,
  Trash2,
  Wrench,
  X,
} from "lucide-react";
import { ContextMenu, DropdownMenu } from "radix-ui";
import {
  Fragment,
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type { PullRequest, SubagentInfo } from "../acp/rustcode";
import { modelName, projectName, type SessionSummary } from "../acp/types";
import type { ThreadStatus } from "../acp/v2";
import { formatBinding, listCommandFor } from "../lib/keys";
import { openTerminalAt, openThreadLog, revealPath } from "../lib/native";
import { hasShell } from "../lib/shell";
import { liveSubagents, type StackAgent } from "../state/agentStack";
import { type BranchRow, branchKindOf, branchRows } from "../state/branches";
import { type ChildInfo, childInfo } from "../state/children";
import {
  cachedPulls,
  loadPulls,
  projectFolders,
  pullForBranch,
  pullState,
  pullsVersion,
  subscribePulls,
} from "../state/pulls";
import { type PendingSetup, pendingSummary } from "../state/setup";
import { applyOrder, moveBy, reorder, sectionOrderKey } from "../state/sidebarOrder";
import { PREF, usePref, useRowLayout } from "../state/sidebarPrefs";
import {
  contextRow,
  isShelf,
  keepListed,
  listedPicks,
  nextRow,
  type OrderKey,
  type OrderRow,
  type OrderSection,
  opensRowMenu,
  placeThread,
  type RowStatus,
  type RowStatusKind,
  rangeRows,
  rowKeyAction,
  rowMeta,
  rowStatus,
  type SectionId,
  type ShelfId,
  type SidebarLayout,
  sectionsOf,
  shelfKey,
  shelfOpen,
  sortSection,
  threadDiff,
  titleFirstTrailing,
  visibleRows,
} from "../state/sidebarRows";
import { snoozePresets, wakeLabel } from "../state/snooze";
import {
  askConfirm,
  askCustomSnooze,
  autoCompactOffFor,
  dismissPendingSetup,
  errorText,
  featureMissing,
  markUnread,
  removeWorktree,
  renameThread,
  repairThread,
  selectThread,
  setAutoCompactForThread,
  setPinned,
  setSectionOrder,
  setThreadStatus,
  settleThread,
  settleThreads,
  setView,
  snoozeThread,
  startNewThread,
  statusOf,
  toast,
  togglePin,
  toggleSidebar,
  trashThread,
  unsettleThreads,
  useApp,
  useShortcutHint,
  wakeSnoozed,
  worktreeOf,
} from "../state/store";
import { formatClock, idleSettled, shortAge } from "../state/thread";
import { openChain, parentOf as parentOfThread, type SubRow, topLevel } from "../state/topLevel";
import { useWaiting, useWaitingIds, waitingLabel, waitingTitle } from "../state/waiting";
import { AgentStack } from "./AgentStack";
import { BrandHeader } from "./BrandHeader";
import { CustomSnooze } from "./CustomSnooze";
import { HarnessIcon } from "./HarnessIcon";
import { NeedsYouRow } from "./Inbox";
import { LinkBadge } from "./Links";
import { ProjectMark } from "./ProjectMark";
import {
  Cluster,
  ClusterButton,
  ContextMenuSurface,
  cx,
  MenuContent,
  menuItem,
  menuItemDanger,
  menuLabel,
  menuSeparator,
  menuSurface,
  Pill,
  Skeleton,
  Spinner,
  surfaceCollisionPadding,
  Tooltip,
} from "./primitives";
import { RollingNumber } from "./RollingNumber";
import { useThreadDrag } from "./sidebarDrag";
import { ThreadHoverCard } from "./ThreadHoverCard";
import { LIGHTS_INSET, TitlebarRow } from "./Titlebar";
import { UpdateRow } from "./Updates";

// The toolbar's buttons start past the traffic lights, so the sidebar cannot be narrower than
// that row: four 32 px buttons, the cluster's 6 px padding and its 10 px trailing gap.
const MIN = LIGHTS_INSET + 144;
const MAX = 460;
/** What the sidebar is wide before it is dragged, and what a double-click on its edge puts back. */
const DEFAULT_WIDTH = 256;

function useSidebarWidth() {
  const [width, setWidth] = useState(() => {
    try {
      return Math.max(MIN, Number(localStorage.getItem("rustcode.sidebarWidth")) || DEFAULT_WIDTH);
    } catch {
      return DEFAULT_WIDTH;
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem("rustcode.sidebarWidth", String(width));
    } catch {
      // Not persisted in a private window.
    }
  }, [width]);
  return [width, setWidth] as const;
}

function ThreadMenuItems({
  session,
  Menu,
  Item,
  Separator,
  onRename,
  bulk,
  onClear,
}: {
  session: SessionSummary;
  Menu: typeof DropdownMenu | typeof ContextMenu;
  Item: typeof DropdownMenu.Item | typeof ContextMenu.Item;
  Separator: typeof DropdownMenu.Separator | typeof ContextMenu.Separator;
  onRename: () => void;
  /** The sidebar's multi-selection, when this row is in it: the menu acts on all of them. */
  bulk?: SessionSummary[];
  onClear?: () => void;
}) {
  const pinned = useApp((s) => s.pinned.includes(session.id));
  const autoCompactOff = useApp((s) => autoCompactOffFor(s, session.id));
  const status = useApp((s) => statusOf(s, session.id));
  const worktree = useApp((s) => worktreeOf(s, session.id));
  const statusMissing = useApp((s) => (s.features.status ? null : featureMissing("status")));
  const snoozed = useApp((s) => session.id in s.snoozed);
  const allPinned = useApp((s) => (bulk ?? []).every((x) => s.pinned.includes(x.id)));
  // Settled threads in the selection get Unsettle beside Settle, the same as a single row.
  const bulkSettled = useApp((s) => (bulk ?? []).some((x) => statusOf(s, x.id) === "done"));
  const bulkAllSettled = useApp((s) => !!bulk?.length && bulk.every((x) => statusOf(s, x.id) === "done"));
  const canFiles = hasShell();
  // The selection's menu: every hook is read above, so the early return keeps the hooks in order.
  if (bulk && bulk.length > 1) {
    const n = bulk.length;
    const ids = bulk.map((x) => x.id);
    return (
      <>
        <div className={menuLabel}>{n} threads selected</div>
        {!bulkAllSettled && (
          <Item
            className={menuItem}
            onSelect={() => {
              settleThreads(ids);
              onClear?.();
            }}
          >
            <CircleCheck size={13} /> Settle {n} Threads
          </Item>
        )}
        {bulkSettled && (
          <Item
            className={menuItem}
            onSelect={() => {
              unsettleThreads(ids);
              onClear?.();
            }}
          >
            <CircleDashed size={13} /> Unsettle {n} Threads
          </Item>
        )}
        <Item
          className={menuItem}
          onSelect={() => {
            setPinned(ids, !allPinned);
            onClear?.();
          }}
        >
          {allPinned ? <PinOff size={13} /> : <Pin size={13} />}
          {allPinned ? "Unpin" : "Pin"} {n} Threads
        </Item>
        <Item
          className={menuItem}
          onSelect={() => {
            for (const id of ids) markUnread(id);
            onClear?.();
          }}
        >
          <CircleCheck size={13} /> Mark {n} Unread
        </Item>
      </>
    );
  }
  return (
    <>
      {status !== "review" && (
        <Item className={menuItem} onSelect={() => void setThreadStatus(session.id, "review")}>
          <Eye size={13} /> Mark for Review
        </Item>
      )}
      {status === "done" ? (
        <Item className={menuItem} onSelect={() => void setThreadStatus(session.id, "idle")}>
          <CircleDashed size={13} /> Unsettle
        </Item>
      ) : (
        <Item className={menuItem} onSelect={() => settleThread(session.id)}>
          <CircleCheck size={13} /> Settle
        </Item>
      )}
      {status === "review" && (
        <Item className={menuItem} onSelect={() => void setThreadStatus(session.id, "idle")}>
          <CircleDashed size={13} /> Clear Status
        </Item>
      )}
      {statusMissing && (
        <div className="px-2 pb-1 text-caption text-fg-faint">Kept by the app: {statusMissing}</div>
      )}
      {snoozed ? (
        <Item className={menuItem} onSelect={() => snoozeThread(session.id, null)}>
          <Clock size={13} /> Unsnooze
        </Item>
      ) : (
        <Menu.Sub>
          <Menu.SubTrigger className={menuItem}>
            <Clock size={13} /> Snooze
            <ChevronRight size={11} strokeWidth={2.5} className="ml-auto text-fg-faint" />
          </Menu.SubTrigger>
          <Menu.Portal>
            <Menu.SubContent
              sideOffset={4}
              alignOffset={-5}
              collisionPadding={surfaceCollisionPadding}
              className={menuSurface}
            >
              {snoozePresets(Date.now()).map((p) => (
                <Item key={p.label} className={menuItem} onSelect={() => snoozeThread(session.id, p.at)}>
                  <span className="flex-1">{p.label}</span>
                  <span className="text-caption text-fg-faint">
                    {wakeLabel(p.at, Date.now()).replace("Tomorrow ", "")}
                  </span>
                </Item>
              ))}
              <Separator className={menuSeparator} />
              <Item className={menuItem} onSelect={() => askCustomSnooze(session.id)}>
                Custom…
              </Item>
            </Menu.SubContent>
          </Menu.Portal>
        </Menu.Sub>
      )}
      <Item className={menuItem} onSelect={() => setView({ kind: "review", threadId: session.id })}>
        <FileDiff size={13} /> Review Changes
      </Item>
      <Item className={menuItem} onSelect={() => setAutoCompactForThread(session.id, autoCompactOff)}>
        <Sparkles size={13} /> {autoCompactOff ? "Turn On Auto-compact" : "Turn Off Auto-compact"}
      </Item>
      <Item className={menuItem} onSelect={() => markUnread(session.id)}>
        <CircleCheck size={13} /> Mark Unread
      </Item>
      <Separator className={menuSeparator} />
      <Item className={menuItem} onSelect={() => togglePin(session.id)}>
        {pinned ? <PinOff size={13} /> : <Pin size={13} />}
        {pinned ? "Unpin" : "Pin"}
      </Item>
      <Item className={menuItem} disabled={!canFiles} onSelect={onRename}>
        <Pencil size={13} /> Rename…
      </Item>
      <Item className={menuItem} onSelect={() => void repairThread(session.id)}>
        <Wrench size={13} /> Repair Thread
      </Item>
      <Item className={menuItem} onSelect={() => copyText(session.id, "Thread ID copied")}>
        <Copy size={13} /> Copy Thread ID
      </Item>
      <Item
        className={menuItem}
        disabled={!canFiles}
        onSelect={() =>
          void openThreadLog(session.id).catch((e) =>
            toast(`Could not open the thread log: ${errorText(e)}`, "error"),
          )
        }
      >
        <FileText size={13} /> Open Thread Log
      </Item>
      {worktree && (
        <>
          <Separator className={menuSeparator} />
          <div className="flex items-center gap-1.5 px-2 pt-0.5 pb-1 text-caption text-fg-muted">
            <GitBranch size={11} /> <span className="truncate font-mono">{worktree.branch}</span>
          </div>
          <Item className={menuItem} disabled={!canFiles} onSelect={() => revealPath(worktree.path)}>
            <FolderOpen size={13} /> Open in {fileManagerName()}
          </Item>
          <Item
            className={menuItem}
            disabled={!canFiles}
            onSelect={() => void openTerminalAt(worktree.path).catch((e) => toast(String(e), "error"))}
          >
            <SquareTerminal size={13} /> Open in Terminal
          </Item>
          <Item className={cx(menuItem, menuItemDanger)} onSelect={() => void removeWorktree(session.id)}>
            <FolderX size={13} /> Remove Worktree…
          </Item>
        </>
      )}
      <Separator className={menuSeparator} />
      <Item
        className={cx(menuItem, menuItemDanger)}
        disabled={!canFiles}
        onSelect={() =>
          askConfirm({
            title: "Move this thread to the Trash?",
            message: `"${session.title || "New thread"}" and its history move to the Trash, where you can still get them back.`,
            action: "Move to Trash",
            destructive: true,
            run: () => void trashThread(session.id),
          })
        }
      >
        <Trash2 size={13} /> Move to Trash
      </Item>
    </>
  );
}

function copyText(text: string, done: string) {
  void navigator.clipboard.writeText(text).then(
    () => toast(done, "info"),
    () => toast("Could not copy", "error"),
  );
}

/** A row's element wherever it is drawn: the list, a project's group, the settled shelf. */
function rowElement(id: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(`[data-row-id="${CSS.escape(id)}"]`);
}

/** A running turn's clock, ticking on its own so the row does not re-render every second. */
function Elapsed({ since }: { since: number }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  return <>{formatClock(now - since)}</>;
}

/** The tint a row's status chip takes, by what it says: the page's own tone tokens, never a new one. */
const CHIP_TINT: Record<RowStatusKind, string> = {
  needs: "bg-warning/14 text-warning",
  working: "bg-accent/12 text-accent",
  waiting: "bg-fill-subtle text-fg-muted",
  failed: "bg-danger/12 text-danger",
  completed: "bg-success/12 text-success",
  review: "bg-accent/16 text-accent",
};

/**
 * The one chip a row's first line ends with: what the thread is doing, or why it waits. Its
 * spinner is the row's only one, and the clock beside `Working` is the running turn's own; a row
 * that shows its age instead has no chip at all (state/sidebarRows.ts `rowStatus`).
 */
function RowStatusChip({ status, startedAt }: { status: RowStatus; startedAt?: number }) {
  const kind = status.kind;
  return (
    <span
      // The long form goes in the title, which the app reads out as the row's label
      // (lib/quietNative.ts); the words in the chip are what the eye gets.
      title={status.title}
      className={cx(
        "inline-flex h-[18px] shrink-0 items-center gap-1 rounded-full px-1.5 font-medium text-caption",
        CHIP_TINT[kind],
      )}
    >
      {kind === "working" ? (
        <Spinner size={9} />
      ) : kind === "needs" ? (
        status.approval ? (
          <ShieldQuestion size={10} />
        ) : (
          <MessageCircleQuestion size={10} />
        )
      ) : kind === "failed" ? (
        <CircleAlert size={10} />
      ) : kind === "completed" ? (
        <Check size={10} />
      ) : kind === "review" ? (
        <Eye size={10} />
      ) : null}
      <span>{status.label}</span>
      {kind === "working" && startedAt !== undefined && (
        <span className="tabular">
          <Elapsed since={startedAt} />
        </span>
      )}
    </span>
  );
}

/** A small icon button in a row's hover slot: 24px, so three of them take the 80px slot. */
function RowAction({
  label,
  hint,
  onClick,
  children,
}: {
  label: string;
  /** The key that does the same thing, shown on the tooltip. */
  hint?: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <Tooltip label={hint ? `${label} (${hint})` : label}>
      <button
        type="button"
        aria-label={label}
        onClick={(e) => {
          e.stopPropagation();
          onClick();
        }}
        className="flex size-6 items-center justify-center rounded-sm text-fg-muted hover:bg-fill hover:text-fg"
      >
        {children}
      </button>
    </Tooltip>
  );
}

/**
 * The room a row gives its trailing state: the mark of the harness it runs on and the diff when
 * they have nowhere else to go, and the state itself — the age, a chip, or the running turn's
 * clock. It is as wide as what it says, so the title takes every pixel the row does not need, and
 * it keeps that width on hover (invisible) while the actions are drawn over it, so nothing reflows.
 * It never wraps: its parts sit side by side on the row's one line, so a second glyph never drops
 * under the state and reads as a line of its own.
 */
const TRAILING_SLOT =
  "flex shrink-0 flex-nowrap items-center justify-end gap-1.5 whitespace-nowrap text-sub text-fg-muted tabular";

/**
 * Hovering swaps the row's trailing state for its actions — Pin, Settle (or Unsettle) and the menu,
 * three 24px buttons 8px in from the row's end. They are drawn over the right end rather than in
 * their own room, so the title never moves: the line fades out under them instead. The fade is
 * taken from the row's right edge in pixels, so a narrow sidebar does not lose more of a title than
 * a wide one, and the actions' 88px is what it covers.
 */
const FADE_UNDER_ACTIONS = cx(
  "group-hover:[mask-image:linear-gradient(to_right,black_calc(100%_-_96px),transparent_calc(100%_-_84px))]",
  "group-hover:[-webkit-mask-image:linear-gradient(to_right,black_calc(100%_-_96px),transparent_calc(100%_-_84px))]",
  "group-has-[:focus-visible]:[mask-image:linear-gradient(to_right,black_calc(100%_-_96px),transparent_calc(100%_-_84px))]",
  "group-has-[:focus-visible]:[-webkit-mask-image:linear-gradient(to_right,black_calc(100%_-_96px),transparent_calc(100%_-_84px))]",
  "group-has-[[data-state=open]]:[mask-image:linear-gradient(to_right,black_calc(100%_-_96px),transparent_calc(100%_-_84px))]",
  "group-has-[[data-state=open]]:[-webkit-mask-image:linear-gradient(to_right,black_calc(100%_-_96px),transparent_calc(100%_-_84px))]",
);

/** What the hover actions are drawn over: the state whose place they take. */
const HIDDEN_ON_HOVER =
  "group-hover:invisible group-has-[:focus-visible]:invisible group-has-[[data-state=open]]:invisible";

/** What a thread's calls changed, small enough for the row's second line or its trailing slot. */
function DiffStat({ diff }: { diff: { added: number; removed: number } }) {
  return (
    <span className="shrink-0 font-mono text-[10px] tabular">
      {diff.added > 0 && <span className="text-success">+{diff.added}</span>}
      {diff.added > 0 && diff.removed > 0 && " "}
      {diff.removed > 0 && <span className="text-danger">−{diff.removed}</span>}
    </span>
  );
}

/**
 * A row's fill, drawn as its own child under the row's content.
 *
 * The row is a `<button>`, and a fill set on one (a `bg-fill` from a `color-mix` token, under the
 * base layer's background transition) never lands: WebKit holds the value the button had before
 * the state changed. A child element mounts with the fill it is given, so the selected, picked
 * and hover states show.
 */
function RowFill({ selected, picked, radius }: { selected: boolean; picked?: boolean; radius: string }) {
  return (
    <span
      aria-hidden
      className={cx(
        "pointer-events-none absolute inset-0",
        radius,
        picked
          ? "bg-accent/12"
          : selected
            ? "bg-fill"
            : "bg-transparent group-hover:bg-fill-subtle group-active:bg-fill",
      )}
    />
  );
}

/** The mark of what a pull request is: open, merged, closed, or still a draft. */
function PullMark({ pull }: { pull: PullRequest }) {
  const state = pullState(pull);
  const Icon =
    state === "merged"
      ? GitMerge
      : state === "closed"
        ? GitPullRequestClosed
        : state === "draft"
          ? GitPullRequestDraft
          : GitPullRequestArrow;
  return (
    <span
      role="img"
      aria-label={`Pull request #${pull.number}: ${state}`}
      title={`#${pull.number} ${state} · ${pull.title}`}
      className={cx(
        "flex shrink-0 items-center",
        state === "merged"
          ? "text-accent"
          : state === "closed"
            ? "text-danger"
            : state === "draft"
              ? "text-fg-muted"
              : "text-success",
      )}
    >
      <Icon size={12} />
    </span>
  );
}

// Memoized, and reads only the flags it shows: a streamed chunk must not re-render 500 rows.
// Compact: one line, a mark, the title drawn by what the thread wants, and the state in a fixed
// slot at its end, with a second line only for a branch, a diff, a pull request or a stack.
// Comfortable: the title first, the project's mark and the branch under it. Hovering swaps the
// slot for the row's actions in both, so nothing shifts.
const ThreadRow = memo(function ThreadRow({
  session,
  selected,
  context,
  grouped,
  now,
  hint,
  pull,
  agents,
  navActive,
  picked,
  bulk,
  dragging,
  pinKey,
  settleKey,
  onClearSelection,
}: {
  session: SessionSummary;
  selected: boolean;
  /** The open thread is one this row started: the row is kept lit as its context, not opened. */
  context?: boolean;
  /** Being dragged to another place in its section: no hover card, and no pointer of its own. */
  dragging?: boolean;
  /** Under its project's heading: the row leaves out the mark and the name it would repeat. */
  grouped: boolean;
  now: number;
  /** No parent row above it (a search result, or a filtered-out parent): the thread it belongs under. */
  hint?: string;
  /** The thread's branch's pull request, from the project's read (state/pulls.ts). */
  pull?: PullRequest;
  /** Threads this one started and still runs: the stack on its second line, in place of child rows. */
  agents?: StackAgent[];
  /** The row the keyboard is on: it holds the list's one tab stop, ↑ ↓ move it. */
  navActive?: boolean;
  /** In the sidebar's multi-selection (⌘-click), which is not the same as the thread being open. */
  picked?: boolean;
  /** The picked threads, when there is more than one: this row's menu then acts on all of them. */
  bulk?: SessionSummary[];
  /** Pin's and Settle's keys (Settings › Keyboard), on the hover actions' tooltips. */
  pinKey?: string;
  settleKey?: string;
  onClearSelection?: () => void;
}) {
  const [rowLayout] = useRowLayout();
  const request = useApp((s) => s.threads[session.id]?.permissions[0]);
  const running = useApp((s) => s.threads[session.id]?.running ?? false);
  const startedAt = useApp((s) => s.threads[session.id]?.turnStartedAt);
  // No turn runs, but the threads it started or messaged still work (state/waiting.ts).
  const wait = useWaiting(session.id);
  const wake = useApp((s) => s.snoozed[session.id]);
  const status = useApp((s) => statusOf(s, session.id));
  const worktree = useApp((s) => worktreeOf(s, session.id));
  const harnesses = useApp((s) => s.harnesses);
  const pinned = useApp((s) => s.pinned.includes(session.id));
  const finishedAt = useApp((s) => s.finished[session.id]);
  const failedAt = useApp((s) => s.failed[session.id]);
  // What the thread's calls changed, from the diffs the app already read: a thread never opened
  // here has no items, and shows nothing rather than a made-up zero.
  const items = useApp((s) => s.threads[session.id]?.items);
  const diff = useMemo(() => threadDiff(items), [items]);
  // Its own subagents count with the threads it started: one stack, whichever way they were spawned.
  const ownAgents = useApp((s) => s.agents[session.id] ?? NO_AGENTS);
  const stack = useMemo(() => {
    const own = liveSubagents(items, ownAgents, running, session.harness);
    return own.length ? [...own, ...(agents ?? [])] : agents;
  }, [items, ownAgents, running, session.harness, agents]);
  // Done or Failed while you were elsewhere, until you open it.
  const fresh = !!finishedAt && !selected;
  const failed = !!failedAt && !selected;
  // What the row's one status chip says, or nothing when it shows its age instead
  // (state/sidebarRows.ts `rowStatus`).
  const chip = rowStatus({
    request,
    running,
    waiting: wait && { label: waitingLabel(wait), title: waitingTitle(wait) },
    failed,
    review: status === "review",
  });
  const [renaming, setRenaming] = useState(false);
  const [title, setTitle] = useState("");
  /** The menu was opened with Shift+F10 or the ContextMenu key: the row takes the keyboard back. */
  const menuFromKeys = useRef(false);
  const external = !!session.harness && session.harness !== "rustcode";
  const harness = external ? harnesses?.harnesses.find((h) => h.id === session.harness) : undefined;
  // Where the row runs: the worktree's repository, else the thread's own folder.
  const project = projectName(worktree?.repo || session.cwd || "");
  // What the row cannot spell out, on its hover card (ui/ThreadHoverCard.tsx).
  const harnessName = harness?.name ?? (external ? session.harness : undefined);
  const startRename = () => {
    setTitle(session.title ?? "");
    // After the menu closes and gives focus back.
    setTimeout(() => setRenaming(true), 0);
  };
  const commit = () => {
    setRenaming(false);
    if (title.trim() && title.trim() !== session.title) void renameThread(session.id, title);
  };
  if (renaming) {
    return (
      <div className="rounded-md bg-fill px-1.5 py-[5px]">
        <input
          autoFocus
          value={title}
          onFocus={(e) => e.currentTarget.select()}
          onChange={(e) => setTitle(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit();
            if (e.key === "Escape") setRenaming(false);
          }}
          aria-label="Thread title"
          className="w-full rounded-sm bg-surface-2 px-1.5 py-0.5 text-body shadow-1"
        />
      </div>
    );
  }
  const done = status === "done";
  const trailing = titleFirstTrailing({ needs: !!request, running, done, status: !!chip });
  // A fork or a side chat says so with a small glyph, whether it is nested or a row of its own.
  const branch = session.parent ? branchKindOf(session.parent) : undefined;
  // The open thread is a delegated one with no row of its own: its parent stands in as the context.
  const lit = selected || !!context;
  // One line by default (Settings › Appearance): the second is for a row that has something to put
  // there. What it carries is the branch, its pull request, and who else works beside it; a plain
  // chat thread has none of it, and a diff or the mark of its harness alone rides in the trailing
  // slot rather than opening a line for one glyph (state/sidebarRows.ts `rowMeta`).
  const compact = rowLayout === "compact";
  const meta = rowMeta({
    branch: !!worktree,
    pull: !!pull,
    stack: !!stack?.length,
    harness: external,
    diff,
    layout: rowLayout,
  });
  // What wants you reads strong; what works, waits or has been read recedes until hovered.
  const wantsYou = !!request || fresh || failed || lit || picked;
  const recedes = !wantsYou && (running || !!wait || done);
  const markTint = cx(
    "transition-opacity duration-150 ease-snappy",
    wantsYou ? "opacity-100" : "opacity-60 group-hover:opacity-100",
  );
  // The title's own weight: what wants you is medium and full; what works, waits or is settled
  // recedes to regular at 75% until hovered.
  const titleTint = cx(
    "min-w-0 flex-1 truncate text-body",
    wantsYou
      ? "font-medium text-fg"
      : recedes
        ? "font-normal text-fg/75 group-hover:text-fg"
        : "font-medium text-fg",
  );
  // A fork or a side chat, and a thread that works for another one: the glyphs before the title.
  const glyphs = (
    <>
      {pinned && <Pin size={11} className="shrink-0 rotate-45 text-fg-faint" />}
      {branch && (
        <span title={branch === "fork" ? "A fork" : "A side chat"} className="shrink-0 text-fg-faint">
          {branch === "fork" ? <GitFork size={11} /> : <MessageSquareQuote size={11} />}
        </span>
      )}
      <LinkBadge threadId={session.id} />
    </>
  );
  // The one thing a row says at its end: what its branch changed when it has no line of its own for
  // it, and what the thread is doing, else the hour a snoozed one wakes, else its age. The two-line
  // row draws a chip as a pill and a settled check bare; the compact row keeps one icon and word,
  // so a column of rows reads as one line of glyphs.
  const age = (
    <span className="min-w-0 truncate">{wake ? wakeLabel(wake, now) : shortAge(session.updatedAt, now)}</span>
  );
  const compactState = chip ? (
    chip.kind === "needs" ? (
      request?.question ? (
        <MessageCircleQuestion size={13} />
      ) : (
        <ShieldQuestion size={13} />
      )
    ) : chip.kind === "working" ? (
      <span className="flex items-center gap-1">
        <Spinner size={9} />
        {startedAt !== undefined && <Elapsed since={startedAt} />}
      </span>
    ) : chip.kind === "waiting" ? (
      <span className="flex items-center gap-1">
        <Clock size={11} className="shrink-0" />
        {chip.label}
      </span>
    ) : (
      <span
        className={cx(
          "flex items-center gap-1",
          chip.kind === "failed" ? "text-danger" : chip.kind === "completed" ? "text-success" : "text-accent",
        )}
      >
        {chip.kind === "failed" ? (
          <CircleAlert size={11} />
        ) : chip.kind === "completed" ? (
          <Check size={11} />
        ) : (
          <Eye size={11} />
        )}
        {chip.label}
      </span>
    )
  ) : (
    age
  );
  const twoLineState =
    trailing === "needs" && request ? (
      request.question ? (
        <MessageCircleQuestion size={13} />
      ) : (
        <ShieldQuestion size={13} />
      )
    ) : trailing === "running" ? (
      <>
        <Spinner size={9} />
        {startedAt !== undefined && <Elapsed since={startedAt} />}
      </>
    ) : trailing === "done" ? (
      <Check size={13} />
    ) : chip ? (
      <RowStatusChip status={chip} startedAt={startedAt} />
    ) : (
      age
    );
  const stateSlot = (pill: boolean) => (
    <span title={chip?.title} className={cx(TRAILING_SLOT, HIDDEN_ON_HOVER)}>
      {/* What the row has no line for: the harness it runs on, then the diff. */}
      {meta === "trailing" && external && (
        <HarnessIcon id={session.harness!} name={harness?.name} size={12} className="shrink-0" />
      )}
      {meta === "trailing" && diff && <DiffStat diff={diff} />}
      {pill ? twoLineState : compactState}
    </span>
  );
  return (
    <ContextMenu.Root
      onOpenChange={(open) => {
        // A menu opened from the keyboard hands it back to the row when it closes, so ↑ ↓, P and
        // D keep working; one the pointer opened returns focus where the pointer went.
        if (open || !menuFromKeys.current) return;
        menuFromKeys.current = false;
        setTimeout(() => rowElement(session.id)?.focus(), 0);
      }}
    >
      <ContextMenu.Trigger asChild>
        <div className={cx("group relative", dragging && "cursor-grabbing")}>
          {/* A moment's hover says what the row has no room for: its project, harness and model,
              what it is doing, its branch and folder, and the last thing said in it. */}
          <ThreadHoverCard
            session={session}
            project={project}
            harness={harnessName}
            model={modelName(session.model)}
            status={chip?.label ?? (done ? "Settled" : "Idle")}
            now={now}
            items={items}
            disabled={dragging}
          >
            <button
              type="button"
              data-row-id={session.id}
              tabIndex={navActive ? 0 : -1}
              onClick={() => selectThread(session.id)}
              onDoubleClick={startRename}
              onKeyDown={(e) => {
                if (e.key === "F2") {
                  e.preventDefault();
                  startRename();
                  return;
                }
                // Shift+F10 and the ContextMenu key open the menu the row's right-click opens,
                // anchored on the row itself rather than wherever the pointer last was.
                if (!opensRowMenu(e)) return;
                e.preventDefault();
                menuFromKeys.current = true;
                const rect = e.currentTarget.getBoundingClientRect();
                e.currentTarget.dispatchEvent(
                  new MouseEvent("contextmenu", {
                    bubbles: true,
                    cancelable: true,
                    clientX: rect.left + 16,
                    clientY: rect.top + Math.min(rect.height / 2, 24),
                  }),
                );
              }}
              aria-current={selected ? "page" : undefined}
              aria-pressed={picked || undefined}
              className={cx(
                "relative flex w-full flex-col justify-center gap-0.5 text-left",
                compact ? "rounded-lg px-2" : "rounded-xl py-2 pr-2 pl-2",
                compact && (meta === "line" || request || hint)
                  ? "min-h-12"
                  : compact
                    ? "min-h-8"
                    : undefined,
              )}
            >
              <RowFill selected={lit} picked={picked} radius={compact ? "rounded-lg" : "rounded-xl"} />
              {compact ? (
                <>
                  {/* One line: the mark of where it runs, its title, and the one thing it says at
                      the end. The project's name is not repeated here, since the mark carries it
                      and the hover card and the folder's heading spell it out. */}
                  <span className={cx("relative flex w-full min-w-0 items-center gap-2", FADE_UNDER_ACTIONS)}>
                    {!grouped && <ProjectMark name={project} size={14} className={markTint} />}
                    {glyphs}
                    <span className={titleTint}>{session.title || "New thread"}</span>
                    {stateSlot(false)}
                  </span>
                  {/* The second line, only when there is something to say: the branch it works in,
                      its pull request, and who else works beside it. The harness mark is not on it
                      unless those are: alone it would be a 12px glyph on an empty line, so it rides
                      in the trailing slot instead. */}
                  {meta === "line" && (
                    <span className="relative flex w-full min-w-0 items-center gap-2 text-sub text-fg-muted">
                      {worktree && (
                        <span className="flex min-w-0 items-center gap-1">
                          <GitBranch size={10} className="shrink-0" />
                          <span className="min-w-0 truncate font-mono text-[10px]" title={worktree.branch}>
                            {worktree.branch}
                          </span>
                        </span>
                      )}
                      {diff && <DiffStat diff={diff} />}
                      {pull && <PullMark pull={pull} />}
                      <span className="ml-auto flex shrink-0 items-center gap-1">
                        {stack?.length ? (
                          <AgentStack agents={stack} />
                        ) : (
                          external && (
                            <HarnessIcon
                              id={session.harness!}
                              name={harness?.name}
                              size={12}
                              className="shrink-0"
                            />
                          )
                        )}
                      </span>
                    </span>
                  )}
                </>
              ) : (
                <span className={cx("title-first-row relative block w-full min-w-0", FADE_UNDER_ACTIONS)}>
                  <span className="flex min-w-0 items-center gap-1.5">
                    {!grouped && <ProjectMark name={project} size={14} className={markTint} />}
                    {glyphs}
                    <span className={titleTint}>{session.title || "New thread"}</span>
                    {stateSlot(true)}
                  </span>
                  {/* The line under the title, only when it has something besides the mark that now
                      sits before the title: the branch, its pull request, and who else works beside
                      it. A diff, or the harness mark alone, is in the trailing slot above. */}
                  {meta === "line" && (
                    <span className="flex min-w-0 items-center gap-1 text-sub text-fg-muted">
                      {worktree && (
                        <span className="title-first-branch min-w-0 flex-1 truncate font-mono text-[10px]">
                          <GitBranch size={9} className="mr-0.5 inline" />
                          {worktree.branch}
                        </span>
                      )}
                      <span className="title-first-badges ml-auto flex shrink-0 items-center gap-1">
                        {stack?.length ? (
                          <AgentStack agents={stack} />
                        ) : (
                          external && (
                            <HarnessIcon
                              id={session.harness!}
                              name={harness?.name}
                              size={13}
                              className="shrink-0"
                            />
                          )
                        )}
                        {diff && <DiffStat diff={diff} />}
                        {pull && <PullMark pull={pull} />}
                      </span>
                    </span>
                  )}
                  {worktree && (
                    <span className="title-first-branch-wrap hidden truncate font-mono text-[10px] text-fg-muted">
                      <GitBranch size={9} className="mr-0.5 inline" />
                      {worktree.branch}
                    </span>
                  )}
                </span>
              )}
              {/* What it asks of you, or which thread a delegated one without a parent row above it
                belongs under. */}
              {(request || hint) && (
                <span className="relative truncate text-fg-muted text-sub">
                  {request ? (request.question ? request.question.text : request.title) : hint}
                </span>
              )}
            </button>
          </ThreadHoverCard>
          <div
            data-row-actions
            // Hidden while the row is dragged: the actions sit beside the row, not on it.
            className={cx(
              "absolute top-1 right-2 hidden items-center",
              !dragging &&
                "group-hover:flex group-has-[:focus-visible]:flex group-has-[[data-state=open]]:flex",
            )}
          >
            <RowAction label={pinned ? "Unpin" : "Pin"} hint={pinKey} onClick={() => togglePin(session.id)}>
              {pinned ? <PinOff size={13} /> : <Pin size={13} />}
            </RowAction>
            {done ? (
              <RowAction
                label="Unsettle"
                hint={settleKey}
                onClick={() => void setThreadStatus(session.id, "idle")}
              >
                <CircleDashed size={14} />
              </RowAction>
            ) : (
              <RowAction label="Settle" hint={settleKey} onClick={() => settleThread(session.id)}>
                <Check size={14} />
              </RowAction>
            )}
            <DropdownMenu.Root modal={false}>
              <DropdownMenu.Trigger asChild>
                <button
                  type="button"
                  aria-label={`Actions for ${session.title || "thread"}`}
                  title={`Actions · ${formatBinding("shift+f10")}`}
                  className="flex size-6 items-center justify-center rounded-sm text-fg-muted hover:bg-fill hover:text-fg"
                >
                  <Ellipsis size={15} />
                </button>
              </DropdownMenu.Trigger>
              <MenuContent align="start" sideOffset={4}>
                <ThreadMenuItems
                  session={session}
                  Menu={DropdownMenu}
                  Item={DropdownMenu.Item}
                  Separator={DropdownMenu.Separator}
                  onRename={startRename}
                  bulk={bulk}
                  onClear={onClearSelection}
                />
              </MenuContent>
            </DropdownMenu.Root>
          </div>
        </div>
      </ContextMenu.Trigger>
      <ContextMenuSurface>
        <ThreadMenuItems
          session={session}
          Menu={ContextMenu}
          Item={ContextMenu.Item}
          Separator={ContextMenu.Separator}
          onRename={startRename}
          bulk={bulk}
          onClear={onClearSelection}
        />
      </ContextMenuSurface>
    </ContextMenu.Root>
  );
});

/**
 * A thread sent before the engine has its session (state/setup.ts): the message's title, where it
 * will run, and what it is doing with its own clock. Failed, it keeps the error and the one action
 * it has, Dismiss; there is no session yet for a menu to act on.
 */
const PendingRow = memo(function PendingRow({
  pending,
  selected,
  navActive,
}: {
  pending: PendingSetup;
  selected: boolean;
  navActive?: boolean;
}) {
  const project = projectName(pending.worktree?.repo || pending.cwd);
  const branch = pending.worktree?.branch;
  const failure = pending.failure;
  return (
    <div className="group relative">
      <button
        type="button"
        data-row-id={pending.id}
        tabIndex={navActive ? 0 : -1}
        onClick={() => selectThread(pending.id)}
        aria-current={selected ? "page" : undefined}
        title={failure ? `${pending.title} · ${failure.text}` : pending.title}
        className="relative flex min-h-12 w-full flex-col justify-center gap-0.5 rounded-lg px-2 text-left"
      >
        <RowFill selected={selected} radius="rounded-lg" />
        <span className="relative flex w-full min-w-0 items-center gap-2">
          <ProjectMark name={project} size={14} className="opacity-60 group-hover:opacity-100" />
          <span className="min-w-0 flex-1 truncate text-body font-medium text-fg">{pending.title}</span>
          {/* The same trailing slot a thread's row has, as wide as what it says, so the state
              lands on the same edge and the title keeps the rest. Dismiss sits over the slot's end
              when the setup failed, so the words keep its room. */}
          <span className={cx(TRAILING_SLOT, failure && "pr-6")}>
            {failure ? (
              <>
                <CircleAlert size={11} className="text-danger" />
                <span className="font-medium text-danger">Failed</span>
              </>
            ) : (
              <>
                <Spinner size={9} />
                <Elapsed since={pending.startedAt} />
              </>
            )}
          </span>
        </span>
        <span className="relative flex w-full min-w-0 items-center gap-2 text-sub text-fg-muted">
          {failure ? (
            <span className="min-w-0 truncate text-danger" title={failure.text}>
              {failure.text}
            </span>
          ) : (
            <span className="shrink-0 text-accent">{pending.worktree ? "Setting up" : "Starting"}</span>
          )}
          {branch && (
            <span className="flex min-w-0 items-center gap-1">
              <GitBranch size={10} className="shrink-0" />
              <span className="min-w-0 truncate font-mono text-[10px]">{branch}</span>
            </span>
          )}
        </span>
      </button>
      <div className="absolute top-1 right-2 flex items-center">
        {failure && (
          <RowAction label="Dismiss" onClick={dismissPendingSetup}>
            <X size={12} />
          </RowAction>
        )}
      </div>
    </div>
  );
});

function useNow() {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(t);
  }, []);
  return now;
}

/** The list's sections by name: Working leads the default layout, and the shelves close it. */
const LABELS: Record<SectionId, string> = {
  working: "Working",
  pinned: "Pinned",
  // The threads that ask for an approval or an answer. Not "Needs you": that is the inbox row
  // above, which is one row per item (a failure is not a thread) and opens the page for all of them.
  needs: "Asking you",
  threads: "Active",
  snoozed: "Snoozed",
  settled: "Settled",
};

/** The folds `PREF.folded` keeps (a project's folder), in one array that never changes identity. */
const NO_FOLDS: string[] = [];

interface Section {
  id: SectionId;
  label: string;
  items: SessionSummary[];
  /** What a folded section hides: a thread that needs you, else one that works, else one just finished. */
  hidden?: HiddenKind;
  /** Threads, split by project when there are several. */
  projects?: { key: string; label: string; items: SessionSummary[] }[];
}

/**
 * A section's rows, unfolding with a height transition. Folded rows are not
 * rendered at all, so a long Settled section costs nothing until opened.
 */
function Fold({ open, children }: { open: boolean; children: React.ReactNode }) {
  const [mounted, setMounted] = useState(open);
  const [shown, setShown] = useState(open);
  useEffect(() => {
    if (open) {
      setMounted(true);
      const frame = requestAnimationFrame(() => setShown(true));
      return () => cancelAnimationFrame(frame);
    }
    setShown(false);
    const t = setTimeout(() => setMounted(false), 200);
    return () => clearTimeout(t);
  }, [open]);
  if (!mounted) return null;
  return (
    <div
      className="grid transition-[grid-template-rows,opacity] duration-200 ease-snappy"
      style={{ gridTemplateRows: shown ? "1fr" : "0fr", opacity: shown ? 1 : 0 }}
    >
      <div className="min-h-0 overflow-hidden">{children}</div>
    </div>
  );
}

/**
 * The open delegated thread as an indented row directly under the row of the thread that started
 * it, and the chain of delegates under it. It unfolds in with the list's fold motion and folds
 * away when the user opens another thread or goes back, instead of vanishing: the last row and
 * what hung under it are held for the fold.
 */
function OpenChild({ row, deeper }: { row: React.ReactNode | null; deeper: React.ReactNode }) {
  const held = useRef({ row, deeper });
  if (row) held.current = { row, deeper };
  return (
    <Fold open={!!row}>
      {held.current.row && (
        // `rc-row-in` is the list's own arrival motion: the fold above it starts open, so the row
        // would otherwise appear in place when the thread is opened.
        <div className="rc-row-in relative pl-4">
          {/* The subtle line saying this row belongs to the one above it. */}
          <span aria-hidden className="pointer-events-none absolute inset-y-1 left-1.5 w-px bg-border" />
          {held.current.row}
          {held.current.deeper}
        </div>
      )}
    </Fold>
  );
}

/** The most forks and side chats drawn under one thread's row. */
const MAX_BRANCH_ROWS = 6;

/** The forks and side chats under a row, each indented by how deep it is, then "+N more". */
function BranchRows({
  under,
  render,
  onMore,
}: {
  under: { rows: BranchRow[]; more: number };
  render: (session: SessionSummary) => React.ReactNode;
  onMore: () => void;
}) {
  return (
    <>
      {under.rows.map(({ session, depth }) => (
        <div key={session.id} className="relative" style={{ paddingLeft: depth * 16 }}>
          <span
            aria-hidden
            className="pointer-events-none absolute inset-y-1 w-px bg-border"
            style={{ left: (depth - 1) * 16 + 6 }}
          />
          {render(session)}
        </div>
      ))}
      {under.more > 0 && (
        <button
          type="button"
          onClick={onMore}
          className="ml-4 rounded-sm px-2 py-1 text-left text-caption text-fg-muted hover:text-fg"
        >
          +{under.more} more
        </button>
      )}
    </>
  );
}

type HiddenKind = "needs" | "working" | "finished";

/** The dot a folded group shows for what it hides. */
function HiddenDot({ kind }: { kind?: HiddenKind }) {
  if (!kind) return null;
  return (
    <span
      role="img"
      aria-label={
        kind === "needs"
          ? "A thread here needs you"
          : kind === "working"
            ? "A thread here is working"
            : "A thread here finished"
      }
      className={cx(
        "size-1.5 shrink-0 rounded-full",
        kind === "needs" ? "bg-warning" : kind === "working" ? "bg-accent animate-pulse" : "bg-success",
      )}
    />
  );
}

function SectionHeader({
  section,
  open,
  onToggle,
}: {
  section: Section;
  open: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onToggle}
      aria-expanded={open}
      className={cx(
        "group/h flex h-7 w-full items-center gap-2 rounded-lg px-2 text-left font-medium text-fg-muted text-sub hover:bg-fill-subtle",
      )}
    >
      <span className="min-w-0 flex-1 truncate">{section.label}</span>
      {!open && <HiddenDot kind={section.hidden} />}
      {/* One slot: an open section shows its count, and the chevron in its place on hover (macOS). */}
      <span className="grid shrink-0 items-center justify-items-end">
        <span
          className={cx(
            "font-normal text-fg-faint [grid-area:1/1]",
            open && "group-hover/h:opacity-0 group-focus-visible/h:opacity-0",
            !open && "hidden",
          )}
        >
          <RollingNumber value={section.items.length} />
        </span>
        {!open && (
          <span className="flex items-center gap-1.5 [grid-area:1/1]">
            <RollingNumber value={section.items.length} className="font-normal text-fg-faint" />
            <ChevronRight size={12} strokeWidth={2.5} className="shrink-0 text-fg-faint" />
          </span>
        )}
        {open && (
          <ChevronRight
            size={12}
            strokeWidth={2.5}
            className="shrink-0 rotate-90 text-fg-faint opacity-0 [grid-area:1/1] group-hover/h:opacity-100 group-focus-visible/h:opacity-100"
          />
        )}
      </span>
    </button>
  );
}

/** The heading of Pinned or Active: it names the group and never folds. */
function SectionLabel({ section }: { section: Section }) {
  return (
    <div className="flex h-7 w-full items-center gap-2 px-2 font-medium text-fg-muted text-sub">
      <span className="min-w-0 flex-1 truncate">{section.label}</span>
      <span className="shrink-0 font-normal text-fg-faint">
        <RollingNumber value={section.items.length} />
      </span>
    </div>
  );
}

/**
 * A project's folder: the folder and its name, a dot for
 * what it hides and its count only while folded, the chevron at the end, and
 * a new-thread button on hover. It reads as its section's own labels do: the same 28px, the same
 * quiet 12px type, so a list of projects is one quiet column of headings.
 */
function ProjectHeader({
  label,
  count,
  open,
  hidden,
  onToggle,
  onNew,
}: {
  label: string;
  count: number;
  open: boolean;
  hidden?: HiddenKind;
  onToggle: () => void;
  onNew?: () => void;
}) {
  return (
    <div className="group/p relative flex h-7 items-center">
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        className="flex h-full min-w-0 flex-1 items-center gap-2 rounded-md px-2 text-left font-medium text-fg-muted text-sub hover:bg-fill-subtle"
      >
        {open ? <FolderOpen size={12} className="shrink-0" /> : <Folder size={12} className="shrink-0" />}
        <span className="min-w-0 flex-1 truncate">{label}</span>
        {!open && <HiddenDot kind={hidden} />}
        {!open && <RollingNumber value={count} className="font-normal text-fg-faint text-sub" />}
        <ChevronDown
          size={12}
          className={cx(
            "shrink-0 text-fg-faint transition-transform duration-200 ease-snappy",
            !open && "-rotate-90",
            onNew && "group-hover/p:invisible group-has-[:focus-visible]/p:invisible",
          )}
        />
      </button>
      {onNew && (
        <span className="absolute right-1.5 hidden group-hover/p:flex group-has-[:focus-visible]/p:flex">
          <RowAction label={`New thread in ${label}`} onClick={onNew}>
            <SquarePen size={12} />
          </RowAction>
        </span>
      )}
    </div>
  );
}

const NO_MARKS: Record<string, ThreadStatus> = {};

/** Nothing picked: one set, so clearing an empty selection never re-renders the list. */
const NO_PICKS: ReadonlySet<string> = new Set();
const NO_AGENTS: SubagentInfo[] = [];

export function Sidebar() {
  const sessions = useApp((s) => s.sessions);
  const pending = useApp((s) => s.pendingSetup);
  const sessionsLoaded = useApp((s) => s.sessionsLoaded);
  const selectedId = useApp((s) => s.selectedId);
  // Which threads need you or work, as a string so a streamed chunk does not change it.
  const flags = useApp((s) =>
    Object.values(s.threads)
      .map((t) => (t.permissions.length ? `${t.id}:n` : t.running ? `${t.id}:w` : ""))
      .join(" "),
  );
  // When each request reached the app and each running turn started, as one string for the same
  // reason: the Needs you and Working sections order by them (state/sidebarRows.ts `sortSection`).
  const clocked = useApp((s) =>
    Object.values(s.threads)
      .flatMap((t) => {
        const asked = t.permissions[0]?.at;
        const sent = t.turnStartedAt;
        return asked === undefined && sent === undefined ? [] : [`${t.id}:${asked ?? ""}:${sent ?? ""}`];
      })
      .join(" "),
  );
  const timings = useMemo(() => {
    const askedAt = new Map<string, number>();
    const sentAt = new Map<string, number>();
    for (const row of clocked.split(" ")) {
      if (!row) continue;
      // A thread id never holds a colon: the two times follow the first one.
      const at = row.indexOf(":");
      const id = at < 0 ? row : row.slice(0, at);
      const [asked, sent] = row.slice(at + 1).split(":");
      if (asked) askedAt.set(id, Number(asked));
      if (sent) sentAt.set(id, Number(sent));
    }
    return { askedAt, sentAt };
  }, [clocked]);
  const marks = useApp((s) => s.statusMarks ?? NO_MARKS);
  const finished = useApp((s) => s.finished);
  const failed = useApp((s) => s.failed);
  const projectCwd = useApp((s) => s.projectCwd);
  const hidden = useApp((s) => s.sidebarHidden);
  const shortcuts = useApp((s) => s.shortcuts);
  const toggleKey = useShortcutHint("toggle-sidebar");
  // The row marks' keys, from the same table: they follow a remap and show on the row's tooltips.
  const pinKey = useShortcutHint("pin-thread");
  const settleKey = useShortcutHint("settle-thread");
  const pinned = useApp((s) => s.pinned);
  // The order each section draws its rows in, as the user arranged it (state/sidebarOrder.ts).
  const layout = useApp((s) => s.sidebarOrder);
  // Each thread's branch's pull request, from the projects' one read (state/pulls.ts).
  const pullsOn = useApp((s) => !!s.agent?.pullRequests);
  // The cache's version: a landed read re-renders the rows that read it.
  useSyncExternalStore(subscribePulls, pullsVersion);
  // By state, with the project on each row; grouping by project is an option.
  // The view options are changed from ⌘K (ui/CommandPalette.tsx), so they are shared, not local.
  const [byProject] = usePref(PREF.groupByProject, false);
  // Working leads the list, or folds into the shelf at its foot with the preference off: the layout
  // the sidebar had before it put running work first (state/sidebarRows.ts `SidebarLayout`).
  const [showWorking] = usePref(PREF.workingSection, true);
  const layoutMode: SidebarLayout = showWorking ? "top" : "shelf";
  const snoozed = useApp((s) => s.snoozed);
  useEffect(() => {
    wakeSnoozed();
    const t = setInterval(() => wakeSnoozed(), 15_000);
    return () => clearInterval(t);
  }, []);
  const [onlyProject] = usePref(PREF.onlyProject, false);
  // The projects folded under "group by project" (`project:<path>`); the shelves keep their own.
  const [folded, setFolded] = usePref<string[]>(PREF.folded, NO_FOLDS);
  // Each shelf's open state is its own, kept per section: `undefined` until the user chooses, so
  // it takes the shelf's default (Working open, Snoozed and Settled shut).
  const [workingStored, setWorkingStored] = usePref<boolean | undefined>(shelfKey("working"), undefined);
  const [snoozedStored, setSnoozedStored] = usePref<boolean | undefined>(shelfKey("snoozed"), undefined);
  const [settledStored, setSettledStored] = usePref<boolean | undefined>(shelfKey("settled"), undefined);
  // The groups of child rows are gone, and with them the folds older releases kept open: drop
  // the record once, so a stale preference does not sit in localStorage for ever.
  useEffect(() => {
    try {
      localStorage.removeItem("rustcode.sidebar.childrenOpen");
    } catch {
      // Nothing stored in a private window.
    }
  }, []);
  const [width, setWidth] = useSidebarWidth();
  // The slot takes its new width at once, so the transcript reflows once per toggle, not every
  // frame (docs/design.md › Motion). The column beside it then slides from where it was with a
  // transform, in step with the sidebar's own slide, so the sidebar pushes the content along.
  const slot = useRef<HTMLElement>(null);
  const shown = useRef(!hidden);
  useLayoutEffect(() => {
    if (shown.current === !hidden) return;
    shown.current = !hidden;
    const column = slot.current?.nextElementSibling;
    if (!column || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const from = hidden ? width : -width;
    column.animate([{ transform: `translateX(${from}px)` }, { transform: "translateX(0)" }], {
      duration: 200,
      easing: "cubic-bezier(0.2, 0.8, 0.2, 1)",
    });
  }, [hidden, width]);
  const dragging = useRef(false);
  const now = useNow();
  // Settled lists 5 threads, then 25 more at a time.
  const [settledShown, setSettledShown] = useState(5);
  // design-lint: allow (mask alpha, not a colour)
  const fade = "linear-gradient(to bottom, #000 calc(100% - 12px), transparent)";

  const filtered = useMemo(
    () =>
      sessions.filter(
        (s) => !onlyProject || !projectCwd || s.cwd === projectCwd || s.worktree?.repo === projectCwd,
      ),
    [sessions, onlyProject, projectCwd],
  );
  // The threads waiting on work they handed out, though no turn of theirs runs: the Working section
  // holds them beside the ones running (state/waiting.ts).
  const waiting = useWaitingIds(useMemo(() => filtered.map((s) => s.id), [filtered]));

  const links = useApp((s) => s.links.links);
  const messaged = useApp((s) => s.messaged);
  // Where a thread an agent started goes: under the thread that started it, and nowhere else
  // (state/topLevel.ts) — the parent survives link pruning in the thread's own metadata. Every
  // session is asked, not the filtered list: a project filter or a search changes what is drawn,
  // never who a thread belongs to.
  const parentOf = useMemo(() => {
    const known = new Set(sessions.map((s) => s.id));
    const parentOf = new Map<string, string>();
    for (const s of sessions) {
      const placement = topLevel(
        {
          id: s.id,
          pinned: pinned.includes(s.id),
          messaged: messaged[s.id] !== undefined,
          parent: s.parent,
        },
        links,
        known,
      );
      const parent = parentOfThread(s, links);
      if (placement === "nested" && parent) parentOf.set(s.id, parent);
    }
    return parentOf;
  }, [sessions, links, pinned, messaged]);
  const byId = useMemo(() => new Map(sessions.map((s) => [s.id, s] as const)), [sessions]);
  // The threads with a row of their own, before the open delegate's chain is drawn under its parent.
  const topRows = useMemo(
    () => new Set(filtered.filter((s) => !parentOf.has(s.id)).map((s) => s.id)),
    [filtered, parentOf],
  );
  // The open delegated thread and the threads between it and its row of its own: drawn as indented
  // rows under the parent, and folded away when the user opens another thread or goes back.
  const chain = useMemo(() => openChain(selectedId, parentOf, topRows), [selectedId, parentOf, topRows]);
  // A nested thread the filter left with no drawn row above it: it keeps a row of its own rather
  // than going missing, saying which thread it belongs under.
  const orphanOpen =
    selectedId && parentOf.has(selectedId) && !chain.length && filtered.some((s) => s.id === selectedId)
      ? selectedId
      : undefined;
  // Each open delegate by the row it hangs under. The chain is one path, so a parent has one child.
  const subOf = useMemo(() => {
    const map = new Map<string, SubRow>();
    for (const sub of chain) map.set(sub.parent, sub);
    return map;
  }, [chain]);
  // The last delegate drawn under each row, kept while it folds away: an unmount would skip the
  // animation, and the session is no longer in the chain by then.
  const lastSubs = useRef(new Map<string, SessionSummary>());
  const childrenByParent = useMemo(() => {
    const map: Record<string, SessionSummary[]> = {};
    for (const s of filtered) {
      const parent = parentOf.get(s.id);
      if (parent) map[parent] = [...(map[parent] ?? []), s];
    }
    // Ids, so a list that reorders on an update does not reorder the stack's icons.
    for (const list of Object.values(map)) list.sort((a, b) => a.id.localeCompare(b.id));
    return map;
  }, [filtered, parentOf]);

  // The forks and side chats below a row, drawn as indented rows so they can be found again (a
  // delegate is not drawn: it is the agent's, and shows as its parent's stack). Branches of
  // branches and of delegates come too, and the open one is drawn by the chain. Capped to the
  // newest few, with a "+N more" row that opens the rest.
  const [allBranches, setAllBranches] = useState<ReadonlySet<string>>(new Set());
  const chainIds = useMemo(() => new Set(chain.map((c) => c.id)), [chain]);
  const branchesUnder = useCallback(
    (rootId: string) =>
      branchRows(
        rootId,
        (id) => childrenByParent[id] ?? [],
        chainIds,
        allBranches.has(rootId) ? Number.POSITIVE_INFINITY : MAX_BRANCH_ROWS,
      ),
    [childrenByParent, chainIds, allBranches],
  );

  // The projects whose branches a pull request could name: a thread in a worktree has one to look
  // up, and its pulls live in the repository the worktree came from.
  const branchProjects = useMemo(
    () => projectFolders(sessions.filter((s) => s.worktree?.branch).map((s) => s.worktree?.repo || s.cwd)),
    [sessions],
  );
  // One read per project, a moment after the list settles: `gh` is slow, and a row is not worth it.
  useEffect(() => {
    if (!pullsOn || !branchProjects.length) return;
    const t = setTimeout(() => branchProjects.forEach(loadPulls), 400);
    return () => clearTimeout(t);
  }, [pullsOn, branchProjects]);
  // Each listed thread's own pull request. The cache keeps the objects, so a row whose branch has
  // no pull request gets nothing new to render, and one that has keeps the same object.
  const pullOf = (session: SessionSummary) =>
    pullForBranch(cachedPulls(session.worktree?.repo || session.cwd), session.worktree?.branch);

  const sections = useMemo(() => {
    const flagged = new Set(flags.split(" "));
    const isFresh = (id: string) => !!finished[id];
    // A thread that waits on work it handed out is drawn with the ones that run, so it is not lost
    // among the idle rows. The shelf layout keeps today's list, where it stays in Active.
    const works = (id: string) => flagged.has(`${id}:w`) || (layoutMode === "top" && waiting.has(id));
    const placed: Record<SectionId, SessionSummary[]> = {
      working: [],
      pinned: [],
      needs: [],
      threads: [],
      snoozed: [],
      settled: [],
    };
    // The thread setting up is a row of the list until the session exists: first, where its real
    // row will be, so it keeps its place when it becomes one.
    const pendingRow =
      pending &&
      (!onlyProject || !projectCwd || pending.cwd === projectCwd || pending.worktree?.repo === projectCwd)
        ? pendingSummary(pending)
        : null;
    for (const s of filtered) {
      // A delegated thread has no row of its own, unless the filter left the open one with no row
      // to hang under (`orphanOpen`).
      if (parentOf.has(s.id) && s.id !== orphanOpen) continue;
      const listed = s.status === "done" || s.status === "review" ? s.status : "idle";
      // Settled: threads settled (by the user, or the engine for a merged branch), and
      // threads untouched for three days unless they wait for review.
      const status = marks[s.id] ?? listed;
      placed[
        placeThread({
          pinned: pinned.includes(s.id),
          settled: status === "done",
          aged: status !== "review" && idleSettled(s.updatedAt, now),
          snoozed: !!snoozed[s.id],
          needs: flagged.has(`${s.id}:n`),
          working: works(s.id),
          fresh: isFresh(s.id),
          layout: layoutMode,
        })
      ].push(s);
    }
    const { threads: active } = placed;
    if (pendingRow) active.unshift(pendingRow);
    // What a slot in the list reads of one thread when the user has not arranged it: the marks the
    // app already keeps, never a running clock, so no section reorders as a turn streams.
    const pinAt = new Map(pinned.map((id, at) => [id, at] as const));
    const keyOf = (s: SessionSummary): OrderKey => ({
      sentAt: timings.sentAt.get(s.id) ?? s.updatedAt,
      askedAt: timings.askedAt.get(s.id),
      backAt: Math.max(s.updatedAt ?? 0, finished[s.id] ?? 0, failed[s.id] ?? 0),
      unseen: !!(finished[s.id] || failed[s.id]),
      pinRank: pinAt.get(s.id),
      wakeAt: snoozed[s.id],
    });
    // The order the user arranged comes over the section's own (state/sidebarOrder.ts): a row that
    // was dragged or ⌥↑/↓'d is drawn where it was put, and the rows nobody arranged keep the order
    // their section gives them (state/sidebarRows.ts `sortSection`).
    const arranged = (id: SectionId, items: SessionSummary[], project?: string) =>
      applyOrder(sortSection(id, items, keyOf), layout[sectionOrderKey(id, project)]);
    // The active threads by project: a worktree thread belongs to its repository.
    const byKey = new Map<string, SessionSummary[]>();
    for (const s of active) {
      const key = s.worktree?.repo || s.cwd || "";
      byKey.set(key, [...(byKey.get(key) ?? []), s]);
    }
    const projects =
      byProject && byKey.size > 1
        ? [...byKey.entries()].map(([key, items]) => ({
            key,
            label: key ? projectName(key) : "No folder",
            items,
          }))
        : undefined;
    // Each project's own rows are arranged on their own: two projects never share one order.
    for (const project of projects ?? []) {
      project.items = arranged("threads", project.items, project.key);
    }
    // The sections this layout draws, in its order: Working, Pinned, Asking you and Active lead, and
    // the shelves close the list (state/sidebarRows.ts `sectionsOf`).
    const all: Section[] = sectionsOf(layoutMode).map((id) => ({
      id,
      label: LABELS[id],
      items: arranged(id, placed[id]),
      projects: id === "threads" ? projects : undefined,
    }));
    for (const g of all) {
      g.hidden = g.items.some((x) => flagged.has(`${x.id}:n`))
        ? "needs"
        : g.items.some((x) => works(x.id))
          ? "working"
          : g.items.some((x) => isFresh(x.id))
            ? "finished"
            : undefined;
    }
    return all.filter((g) => g.items.length);
  }, [
    filtered,
    flags,
    waiting,
    timings,
    marks,
    finished,
    failed,
    pinned,
    parentOf,
    byProject,
    now,
    snoozed,
    layoutMode,
    pending,
    onlyProject,
    projectCwd,
    orphanOpen,
    layout,
  ]);

  const settledSection = sections.find((g) => g.id === "settled");
  // The first shelf starts the foot of the list; the sections above it are labelled once there is
  // more than one of them.
  const firstShelf = sections.find((g) => isShelf(g.id, layoutMode))?.id;
  const labelled = sections.filter((g) => !isShelf(g.id, layoutMode)).length > 1;
  // The open delegate's parent may have been settled: the shelf opens for it and its slice of the
  // shelf grows to hold the row, so the row it hangs under is reachable rather than out of sight.
  const settledAt = settledSection
    ? Math.max(
        -1,
        ...settledSection.items.map((s, index) => (chain.some((c) => c.parent === s.id) ? index : -1)),
      )
    : -1;
  const settledForChain = settledAt >= 0;
  // Every shelf folds on its own header, and the choice is kept per shelf. The sections above them
  // are the list itself and never fold; a project's folder (group by project) folds with `folded`.
  const shelfIsOpen: Record<ShelfId, boolean> = {
    working: shelfOpen("working", workingStored),
    snoozed: shelfOpen("snoozed", snoozedStored),
    settled: shelfOpen("settled", settledStored, { forced: settledForChain }),
  };
  const isOpen = (id: string) => (isShelf(id, layoutMode) ? shelfIsOpen[id] : !folded.includes(id));
  const toggle = (id: string) => {
    if (id === "working") setWorkingStored(!isOpen(id));
    else if (id === "snoozed") setSnoozedStored(!isOpen(id));
    else if (id === "settled") setSettledStored(!isOpen(id));
    else setFolded(folded.includes(id) ? folded.filter((x) => x !== id) : [...folded, id]);
  };
  const settledLimit = Math.max(settledShown, settledAt + 1);

  // Which list each drawn row arranges, and the rows drawn in it: a drag and ⌥↑/↓ both move a row
  // inside the one list it was drawn in — a section, or one project's run of rows under its heading
  // — and nowhere else (state/sidebarOrder.ts `reorder`).
  const homes = useMemo(() => {
    const homes = new Map<string, { key: string; ids: string[] }>();
    // Only threads with a session: the row a message shows before the engine has it is not one to
    // arrange, and it leaves the list as soon as the session exists.
    const add = (key: string, items: SessionSummary[]) => {
      const ids = items.map((s) => s.id).filter((id) => byId.has(id));
      for (const id of ids) homes.set(id, { key, ids });
    };
    for (const section of sections) {
      // Settled draws a slice of its rows, and a folded section draws none: the rows on screen are
      // the ones a drag can move.
      const items = section.id === "settled" ? section.items.slice(0, settledLimit) : section.items;
      if (section.projects) {
        for (const p of section.projects) add(sectionOrderKey(section.id, p.key), p.items);
      } else add(section.id, items);
    }
    return homes;
  }, [byId, sections, settledLimit]);

  // ⌥↑/↓ on a focused row takes it one place up or down the list it is drawn in, and keeps the
  // keyboard on it: the row is the same element, moved (ui/Sidebar.tsx `onListKey`).
  const moveRow = (id: string, delta: number) => {
    const home = homes.get(id);
    if (!home) return;
    const next = moveBy(layout[home.key] ?? [], home.ids, id, delta);
    if (next) setSectionOrder(home.key, next);
  };

  // ---------------------------------------------------------------- the list's rows, selection and keys
  // What is picked, and which row the keyboard is on, belong to the sidebar while it is drawn:
  // nothing outside it reads them, and a thread that leaves the list leaves the selection.
  const [picked, setPicked] = useState<ReadonlySet<string>>(NO_PICKS);
  const [activeId, setActiveId] = useState<string | null>(null);
  const anchor = useRef<string | null>(null);
  const list = useRef<HTMLElement>(null);
  // Dragging a row rearranges the list it was drawn in and nothing else; a search narrows the rows,
  // so a drag is off while one is on and ⌥↑/↓ still arranges (ui/sidebarDrag.tsx).
  const drag = useThreadDrag({
    list,
    homeOf: (id) => homes.get(id),
    enabled: true,
    drop: (home, id, index) =>
      setSectionOrder(home.key, reorder(layout[home.key] ?? [], home.ids, id, index)),
  });
  // Every delegated thread's state, as its parent's stack reads it: one string, so a streamed
  // chunk that changes nothing about them does not redraw the list.
  const childStates = useApp((s) => {
    const linked = new Map(s.links.links.filter((l) => l.started).map((l) => [l.target, l.state] as const));
    return filtered
      .map((c) => {
        if (!parentOf.has(c.id)) return "";
        const info = childInfo(c.id, {
          status: statusOf(s, c.id),
          link: linked.get(c.id),
          failed: !!s.failed[c.id],
        });
        return `${info.id}:${info.state}`;
      })
      .join(" ");
  });
  const infos = useMemo(() => {
    const by = new Map<string, ChildInfo>();
    for (const part of childStates.split(" ")) {
      const [id, state] = part.split(":");
      if (id) by.set(id, { id, state: state as ChildInfo["state"] });
    }
    return by;
  }, [childStates]);

  // A row's stack: every descendant that works or waits on the user, grandchildren flattened,
  // in a stable order so an update that changes no state does not move the icons. A finished or
  // failed delegate adds nothing: the stack is work under way, not the tree's history.
  const stacks = useMemo(() => {
    const out = new Map<string, StackAgent[]>();
    // A nested thread's start time is its link's: the row says how long it has run, and
    // the mark's kind says it is a thread to open, not a subagent inside this one.
    const linkOf = new Map(links.filter((l) => l.started).map((l) => [l.target, l] as const));
    const gather = (parentId: string): StackAgent[] => {
      const found: StackAgent[] = [];
      const walk = (id: string) => {
        for (const child of childrenByParent[id] ?? []) {
          const info = infos.get(child.id);
          if (info && (info.state === "needs" || info.state === "working"))
            found.push({
              id: child.id,
              state: info.state,
              kind: "thread",
              harness: child.harness,
              label: child.title || "New thread",
              since: linkOf.get(child.id)?.createdAt,
            });
          walk(child.id);
        }
      };
      walk(parentId);
      return found;
    };
    for (const s of filtered) {
      if (parentOf.has(s.id)) continue;
      const agents = gather(s.id);
      if (agents.length) out.set(s.id, agents);
    }
    return out;
  }, [childrenByParent, filtered, infos, links, parentOf]);

  // The order the list draws its rows in, so ↑ ↓ move through what is on screen: a shut section
  // or project contributes nothing. A delegated thread has no row of its own, so it is not here
  // (state/sidebarRows.ts).
  const { working: workingOpen, snoozed: snoozedOpen, settled: settledShelfOpen } = shelfIsOpen;
  const order = useMemo<OrderSection[]>(() => {
    const shelfShut: Record<ShelfId, boolean> = {
      working: !workingOpen,
      snoozed: !snoozedOpen,
      settled: !settledShelfOpen,
    };
    // A row with the open delegate under it draws its chain next: ↑ ↓ and the tab stop reach the
    // sub-rows, because the list draws them.
    const toOrder = (items: SessionSummary[]): OrderRow[] => {
      const rows: OrderRow[] = [];
      const add = (s: SessionSummary, depth: number) => {
        rows.push({ id: s.id });
        const sub = subOf.get(s.id);
        const child = sub && depth < 8 ? byId.get(sub.id) : undefined;
        if (child) add(child, depth + 1);
        if (depth === 0) for (const kid of branchesUnder(s.id).rows) rows.push({ id: kid.session.id });
      };
      for (const s of items) add(s, 0);
      return rows;
    };
    return sections.map((section) =>
      section.projects
        ? {
            id: section.id,
            groups: section.projects.map((p) => ({
              key: p.key,
              shut: folded.includes(`project:${p.key}`),
              rows: toOrder(p.items),
            })),
          }
        : {
            id: section.id,
            shut: isShelf(section.id, layoutMode) && shelfShut[section.id],
            rows: toOrder(section.items),
            limit: section.id === "settled" ? settledLimit : undefined,
          },
    );
  }, [
    branchesUnder,
    byId,
    folded,
    sections,
    settledLimit,
    workingOpen,
    snoozedOpen,
    settledShelfOpen,
    subOf,
    layoutMode,
  ]);
  const orderIds = useMemo(() => visibleRows(order), [order]);

  // The row the keyboard is on: where it was left, else the open thread, else the first row. A
  // delegated thread has no row, so the open one lands on its parent, which is the row kept lit
  // as its context (state/sidebarRows.ts).
  const contextSelected = contextRow(selectedId, orderIds, parentOf);
  const currentId =
    activeId && orderIds.includes(activeId) ? activeId : (contextSelected ?? orderIds[0] ?? null);

  useEffect(() => {
    setPicked((prev) => (prev.size ? keepListed(prev, orderIds) : prev));
  }, [orderIds]);

  const pickedSessions = useMemo(() => listedPicks(sessions, picked), [sessions, picked]);
  const allPickedPinned = pickedSessions.length > 0 && pickedSessions.every((s) => pinned.includes(s.id));
  // Whether any or every picked thread is settled, read from the same list the bar acts on.
  // Unsettle moves only the settled ones, so a Review mark on the rest of the selection stays.
  const pickedSettled = useApp((s) =>
    listedPicks(s.sessions, picked).some((x) => statusOf(s, x.id) === "done"),
  );
  const allPickedSettled = useApp((s) => {
    const picks = listedPicks(s.sessions, picked);
    return picks.length > 0 && picks.every((x) => statusOf(s, x.id) === "done");
  });
  const clearSelection = useCallback(() => setPicked(NO_PICKS), []);
  const focusRow = (id: string) => {
    const row = list.current?.querySelector<HTMLElement>(`[data-row-id="${CSS.escape(id)}"]`);
    row?.focus();
    return !!row;
  };

  // P pins and D settles the rows the keyboard acts on: a multi-selection when there is one, else
  // the row the keyboard is on. Both are the calls the hover actions and the selection bar make,
  // so a mark means the same thing wherever it is asked for, and both offer the same toast.
  const markRows = (command: string, focused: string) => {
    // The pending row has no thread yet: there is nothing to pin or settle, as a click and the
    // row's own menu already know.
    if (pending && focused === pending.id) return;
    const ids = pickedSessions.length > 1 ? pickedSessions.map((s) => s.id) : [focused];
    if (command === "pin-thread") {
      // The same rule as the selection bar's button: unpin when every row is pinned, else pin.
      const wasPinned = ids.filter((id) => pinned.includes(id));
      const on = wasPinned.length !== ids.length;
      setPinned(ids, on);
      const title = byId.get(ids[0]!)?.title || "New thread";
      toast(
        ids.length > 1
          ? `${on ? "Pinned" : "Unpinned"} ${ids.length} threads`
          : `${on ? "Pinned" : "Unpinned"} "${title}"`,
        "info",
        {
          // Each row goes back the way it was, however mixed the selection was.
          action: {
            label: "Undo",
            run: () => {
              const rest = ids.filter((id) => !wasPinned.includes(id));
              if (wasPinned.length) setPinned(wasPinned, true);
              if (rest.length) setPinned(rest, false);
            },
          },
        },
      );
    } else if (command === "settle-thread") {
      const state = useApp.getState();
      // D settles; on threads already settled it puts them back, like the row's own button.
      if (ids.every((id) => statusOf(state, id) === "done")) unsettleThreads(ids);
      else settleThreads(ids);
    } else return;
    // A mark can move the row to another section, where it is drawn anew: keep the keyboard on it.
    // D also takes a row out of the list itself, folding it into the shut Settled shelf, and the
    // row then unmounts with the keyboard on it, which leaves focus on <body>. So read the rows
    // beside it first, while the list still draws them, and let the row that took its place take
    // the keyboard when the mark's row is gone: the next one, else the one before.
    const at = orderIds.indexOf(focused);
    const beside = at < 0 ? [] : [...orderIds.slice(at + 1), ...orderIds.slice(0, at).reverse()];
    setActiveId(focused);
    setTimeout(() => {
      const on = [focused, ...beside].find((id) => focusRow(id));
      if (on && on !== focused) setActiveId(on);
    }, 0);
  };

  // ↑ ↓ ⏎ Esc over the rows, and P/D to mark the row the keyboard is on; a section or group
  // control keeps its own keys, and a field inside the list (the rename box) keeps every key.
  const onListKey = (e: React.KeyboardEvent) => {
    const target = e.target as HTMLElement | null;
    const editable = !!target?.closest("input, textarea, [contenteditable]");
    // ⌥↑/↓ arranges the row the keyboard is on in the list it is drawn in — the settings list's own
    // gesture (ui/HeaderButtonsSetting.tsx) — so the order is reachable without a pointer. A field
    // inside the list keeps every key.
    if (!editable && e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
      if (!target?.closest("[data-row-id]") || !currentId) return;
      e.preventDefault();
      moveRow(currentId, e.key === "ArrowUp" ? -1 : 1);
      return;
    }
    const action = rowKeyAction({
      key: e.key,
      metaKey: e.metaKey,
      ctrlKey: e.ctrlKey,
      altKey: e.altKey,
      editable,
    });
    // The list's marking keys never fire from the window (lib/keys.ts): they are read here, where
    // a row has the keyboard, so typing one into any field stays typing. A key held down repeats,
    // and a repeated P or D would mark and unmark the row in turns: only the first press counts.
    const mark = editable || e.repeat ? null : listCommandFor(shortcuts, e);
    if (!action && !mark) return;
    if (action?.kind === "clear") {
      e.preventDefault();
      clearSelection();
      return;
    }
    if (!target?.closest("[data-row-id]")) return;
    e.preventDefault();
    if (mark) {
      if (currentId) markRows(mark, currentId);
      return;
    }
    if (action?.kind === "open") {
      if (currentId) selectThread(currentId);
      clearSelection();
      return;
    }
    if (action?.kind !== "move") return;
    const to = nextRow(orderIds, currentId, action.delta);
    if (!to) return;
    setActiveId(to);
    focusRow(to);
  };

  const onListFocus = (e: React.FocusEvent) => {
    const id = (e.target as HTMLElement | null)?.closest<HTMLElement>("[data-row-id]")?.dataset.rowId;
    if (id) setActiveId(id);
  };

  // ⌘-click toggles a row, ⇧-click takes the range from the last row clicked, and a plain click
  // opens the thread and lets the selection go (the row's own onClick does the opening).
  const onListClick = (e: React.MouseEvent) => {
    const id = (e.target as HTMLElement | null)?.closest<HTMLElement>("[data-row-id]")?.dataset.rowId;
    if (!id) return;
    // The pending row has no session to pin, settle or rename: it opens, and never joins a bulk pick.
    if (pending && id === pending.id) {
      if (primaryModifier(e) || e.shiftKey) {
        e.preventDefault();
        e.stopPropagation();
      }
      setActiveId(id);
      anchor.current = id;
      clearSelection();
      return;
    }
    const mod = primaryModifier(e);
    if (!mod && !e.shiftKey) {
      setActiveId(id);
      anchor.current = id;
      clearSelection();
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    setActiveId(id);
    if (e.shiftKey) {
      setPicked(new Set(rangeRows(orderIds, anchor.current ?? id, id)));
      return;
    }
    anchor.current = id;
    setPicked((prev) => {
      const next = new Set(prev);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  };

  // The width is dragged from the sidebar's edge and kept in app storage as it goes
  // (`useSidebarWidth`); a double-click on that edge puts the default back.
  const onResize = (e: React.PointerEvent) => {
    dragging.current = true;
    const startX = e.clientX;
    const start = width;
    const move = (ev: PointerEvent) => setWidth(Math.min(MAX, Math.max(MIN, start + ev.clientX - startX)));
    const up = () => {
      dragging.current = false;
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  // A row that has no parent row above it says which thread it belongs under: a search result, or
  // the open delegate whose parent the filter left out.
  const hintFor = (s: SessionSummary): string | undefined => {
    const above = parentOf.get(s.id);
    if (!above) return undefined;
    return `in ${byId.get(above)?.title || "another thread"}`;
  };
  const rowFor = (s: SessionSummary, grouped: boolean, hint = true) => (
    <ThreadRow
      session={s}
      selected={s.id === selectedId}
      context={s.id === contextSelected && s.id !== selectedId}
      // A row being dragged shows no hover card and takes no pointer.
      dragging={drag.draggedId === s.id}
      grouped={grouped}
      pull={pullOf(s)}
      now={now}
      hint={hint ? hintFor(s) : undefined}
      agents={stacks.get(s.id)}
      navActive={s.id === currentId}
      picked={picked.has(s.id)}
      bulk={picked.size > 1 && picked.has(s.id) ? pickedSessions : undefined}
      pinKey={pinKey}
      settleKey={settleKey}
      onClearSelection={clearSelection}
    />
  );
  // The open delegate and its descendants, each an indented row under the one above it. The last
  // one drawn under a row is kept while it folds away (`OpenChild` holds it for the animation).
  const openBelow = (parentId: string, grouped: boolean, depth = 0): React.ReactNode => {
    const sub = subOf.get(parentId);
    const current = sub ? byId.get(sub.id) : undefined;
    if (current && sub) lastSubs.current.set(parentId, current);
    const child = current ?? lastSubs.current.get(parentId);
    if (!child || depth > 8) return null;
    return (
      <OpenChild
        row={current ? rowFor(current, grouped, false) : null}
        deeper={openBelow(child.id, grouped, depth + 1)}
      />
    );
  };
  const rows = (items: SessionSummary[], grouped: boolean) => (
    <div className="flex flex-col gap-px">
      {items.map((s) => (
        <Fragment key={s.id}>
          {pending && s.id === pending.id ? (
            <PendingRow pending={pending} selected={!selectedId} navActive={s.id === currentId} />
          ) : (
            rowFor(s, grouped)
          )}
          {openBelow(s.id, grouped)}
          <BranchRows
            under={branchesUnder(s.id)}
            render={(kid) => rowFor(kid, grouped, false)}
            onMore={() => setAllBranches((prev) => new Set(prev).add(s.id))}
          />
        </Fragment>
      ))}
    </div>
  );

  return (
    <aside
      ref={slot}
      aria-label="Threads"
      // Not clipped: while it slides out of a slot already 0 wide, the column beside it covers it.
      className="relative flex shrink-0 flex-col"
      style={{ width: hidden ? 0 : width }}
      aria-hidden={hidden}
    >
      <div
        className={cx(
          "flex h-full flex-col bg-sidebar-glass transition-[translate] duration-200 ease-snappy",
          hidden ? "-translate-x-full" : "translate-x-0",
        )}
        style={{ width, minWidth: width }}
      >
        <TitlebarRow className="justify-end pr-3">
          <Cluster>
            <ClusterButton
              label="Hide sidebar"
              tooltip={`Hide sidebar (${toggleKey})`}
              onClick={toggleSidebar}
            >
              <PanelLeft size={14} />
            </ClusterButton>
          </Cluster>
        </TitlebarRow>
        <BrandHeader />
        <NeedsYouRow />
        <nav
          ref={list}
          // One tab stop for the whole list (the row the keyboard is on), ↑ ↓ through its rows,
          // ⏎ to open and Esc to let the selection go.
          onKeyDown={onListKey}
          onFocus={onListFocus}
          onClickCapture={onListClick}
          // A press on a row is a drag if the pointer moves (ui/sidebarDrag.tsx), and a click if not.
          onPointerDown={drag.start}
          aria-label="Thread list"
          className="relative min-h-0 flex-1 overflow-y-auto px-3 pb-2"
          style={{ maskImage: fade, WebkitMaskImage: fade }}
        >
          {!sessionsLoaded && <ThreadRowsSkeleton />}
          {sessionsLoaded && !filtered.length && (
            <div className="px-2 py-3 text-callout text-fg-muted">
              {onlyProject ? "No threads in this project yet." : "No threads yet."}
            </div>
          )}
          <div className="flex min-h-full flex-col">
            {sections.map((section) => {
              const open = isOpen(section.id);
              const shelf = isShelf(section.id, layoutMode);
              return (
                <Fragment key={section.id}>
                  {/* The shelves sit at the foot, under one hairline, apart from Pinned and Active. */}
                  {section.id === firstShelf && (
                    <div aria-hidden className="mt-2 min-h-2 flex-1 border-border border-b" />
                  )}
                  <section
                    className={cx("mt-2 first:mt-0", section.id === firstShelf && "mt-0")}
                    aria-label={section.label}
                    data-shelf={shelf ? section.id : undefined}
                  >
                    {/* Pinned and Active carry a label (they never fold) once there are two of them;
                      a shelf's header folds it. */}
                    {shelf ? (
                      <SectionHeader section={section} open={open} onToggle={() => toggle(section.id)} />
                    ) : (
                      labelled && <SectionLabel section={section} />
                    )}
                    <Fold open={open}>
                      {section.projects
                        ? section.projects.map((p) => {
                            const projectOpen = isOpen(`project:${p.key}`);
                            return (
                              <div key={p.key} className="mb-1">
                                <ProjectHeader
                                  label={p.label}
                                  count={p.items.length}
                                  hidden={
                                    p.items.some((x) => flags.includes(`${x.id}:n`))
                                      ? "needs"
                                      : p.items.some((x) => flags.includes(`${x.id}:w`))
                                        ? "working"
                                        : p.items.some((x) => !!finished[x.id])
                                          ? "finished"
                                          : undefined
                                  }
                                  open={projectOpen}
                                  onToggle={() => toggle(`project:${p.key}`)}
                                  onNew={p.key ? () => startNewThread(p.key) : undefined}
                                />
                                {/* Under its project's heading the row drops the mark: its title then
                                    lines up under the folder's name. */}
                                <Fold open={projectOpen}>
                                  <div className="pl-5">{rows(p.items, true)}</div>
                                </Fold>
                              </div>
                            );
                          })
                        : rows(
                            section.id === "settled" ? section.items.slice(0, settledLimit) : section.items,
                            // Under its project's heading the row drops the mark and the name.
                            !!section.projects,
                          )}
                      {section.id === "settled" && section.items.length > settledLimit && (
                        <button
                          type="button"
                          onClick={() => setSettledShown((n) => n + 25)}
                          className="flex h-7 w-full items-center gap-2 rounded-lg px-2 text-left text-fg-muted text-sub hover:bg-fill-subtle hover:text-fg"
                        >
                          <ChevronDown size={14} className="shrink-0" />
                          Show {Math.min(25, section.items.length - settledShown)} more
                        </button>
                      )}
                    </Fold>
                  </section>
                </Fragment>
              );
            })}
          </div>
          {/* Where a dragged row would land: one rule, over the rows, never in the layout. */}
          {drag.line && (
            <div
              aria-hidden
              className="pointer-events-none absolute z-30 h-0.5 rounded-full bg-accent"
              style={{ top: drag.line.top - 1, left: drag.line.left, width: drag.line.width }}
            />
          )}
        </nav>
        {pickedSessions.length > 1 && (
          <div className="flex shrink-0 items-center gap-1 px-2 pb-1">
            <span
              className="min-w-0 flex-1 truncate px-1 font-medium text-caption text-fg-muted tabular"
              aria-live="polite"
            >
              {pickedSessions.length} selected
            </span>
            <Pill
              size="sm"
              quiet
              onClick={() => {
                setPinned(
                  pickedSessions.map((s) => s.id),
                  !allPickedPinned,
                );
                clearSelection();
              }}
            >
              {allPickedPinned ? "Unpin" : "Pin"}
            </Pill>
            {!allPickedSettled && (
              <Pill
                size="sm"
                quiet
                onClick={() => {
                  settleThreads(pickedSessions.map((s) => s.id));
                  clearSelection();
                }}
              >
                Settle
              </Pill>
            )}
            {pickedSettled && (
              <Pill
                size="sm"
                quiet
                onClick={() => {
                  unsettleThreads(pickedSessions.map((s) => s.id));
                  clearSelection();
                }}
              >
                Unsettle
              </Pill>
            )}
            <Pill size="sm" quiet onClick={clearSelection}>
              Clear
            </Pill>
          </div>
        )}
        <UpdateRow />
      </div>
      <CustomSnooze />
      {!hidden && (
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize sidebar"
          title="Drag to resize · Double-click to reset"
          onPointerDown={onResize}
          // Back to the width a fresh window has, kept like any other width.
          onDoubleClick={() => setWidth(DEFAULT_WIDTH)}
          className="group absolute top-0 right-[-3px] bottom-0 z-10 w-[7px] cursor-col-resize"
        >
          <div className="mx-auto h-full w-px bg-transparent group-hover:bg-fill" />
        </div>
      )}
    </aside>
  );
}

/** Where the thread list will be: a section's label, then rows of one 32px line each. */
const SKELETON_ROWS = [72, 56, 84, 64, 48, 76, 60];
function ThreadRowsSkeleton() {
  return (
    <div className="flex flex-col pt-1" role="status" aria-label="Loading threads">
      <Skeleton className="mx-2 mt-2 mb-3 h-2.5 w-16" />
      {SKELETON_ROWS.map((w, i) => (
        <div key={i} className="flex h-8 items-center px-2">
          <Skeleton className="h-3" style={{ width: `${w}%` }} />
        </div>
      ))}
    </div>
  );
}
