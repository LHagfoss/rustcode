// The sidebar: a toolbar clear of the traffic lights, a search row with the
// filter and the buttons that make a thread or a project, then one list:
// pinned threads first, then every active thread (by project when asked). A row
// runs two lines: where it runs, its branch and what the branch's pull request
// became, above its title and the mark of the harness that runs it. What it is
// doing (Working, Approval, Input, Failed, Done until you open it) takes the
// time's slot on the top line, the threads it started fold under it as slim
// rows, and Settled (threads marked Done) opens from the foot. The app's own
// pages (settings, pull requests) are buttons in the toolbar's titlebar row.

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
  Folder,
  FolderOpen,
  FolderPlus,
  FolderX,
  GitBranch,
  GitMerge,
  GitPullRequest,
  GitPullRequestArrow,
  GitPullRequestClosed,
  GitPullRequestDraft,
  KanbanSquare,
  ListFilter,
  MessageCircleQuestion,
  PanelLeft,
  Pencil,
  Pin,
  PinOff,
  Search,
  ShieldQuestion,
  SquarePen,
  SquareTerminal,
  Trash2,
  Wrench,
  X,
} from "lucide-react";
import { ContextMenu, Dialog, DropdownMenu } from "radix-ui";
import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type { PullRequest } from "../acp/rustcode";
import { displayPath, modelName, projectName, type SessionSummary } from "../acp/types";
import type { ThreadStatus } from "../acp/v2";
import { openTerminalAt, pickFolder, revealPath } from "../lib/native";
import { hasShell } from "../lib/shell";
import {
  type ChildInfo,
  childInfo,
  childrenListed,
  childSummary,
  flatChildren,
  foldFinished,
  GRANDCHILD_INSET,
  orderChildren,
  pruneOpenGroups,
  unseenChild,
} from "../state/children";
import {
  cachedPulls,
  loadPulls,
  projectFolders,
  pullForBranch,
  pullState,
  pullsVersion,
  subscribePulls,
} from "../state/pulls";
import {
  keepListed,
  nextRow,
  type OrderRow,
  type OrderSection,
  rangeRows,
  rowKeyAction,
  threadDiff,
  visibleRows,
} from "../state/sidebarRows";
import { snoozePresets, wakeLabel } from "../state/snooze";
import {
  askConfirm,
  askCustomSnooze,
  featureMissing,
  markUnread,
  removeWorktree,
  renameThread,
  repairThread,
  searchThreads,
  selectThread,
  setPanel,
  setPinned,
  setThreadStatus,
  setThreadsStatus,
  setView,
  snoozeThread,
  startNewThread,
  statusOf,
  toast,
  togglePin,
  toggleSidebar,
  trashThread,
  useApp,
  useShortcutHint,
  wakeSnoozed,
  worktreeOf,
} from "../state/store";
import { formatClock, idleSettled, shortAge } from "../state/thread";
import { useWaiting, waitingLabel, waitingTitle } from "../state/waiting";
import { HarnessIcon } from "./HarnessIcon";
import { LinkBadge } from "./Links";
import { SettingsButton } from "./MainHeader";
import { ProjectMark } from "./ProjectMark";
import {
  Cluster,
  ClusterButton,
  cx,
  inputClass,
  menuItem,
  menuItemDanger,
  menuLabel,
  menuSeparator,
  menuSurface,
  Pill,
  Skeleton,
  Spinner,
  Tooltip,
} from "./primitives";
import { StatusChip } from "./Status";
import { LIGHTS_INSET, TitlebarRow } from "./Titlebar";
import { UpdateRow } from "./Updates";

// The toolbar's buttons start past the traffic lights, so the sidebar cannot be narrower than
// that row: four 32 px buttons, the cluster's 6 px padding and its 10 px trailing gap.
const MIN = LIGHTS_INSET + 144;
const MAX = 460;

function useSidebarWidth() {
  const [width, setWidth] = useState(() => {
    try {
      return Math.max(MIN, Number(localStorage.getItem("rustcode.sidebarWidth")) || 256);
    } catch {
      return 256;
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

/** Choose a wake time when no preset fits. */
function CustomSnooze() {
  const id = useApp((s) => s.snoozeAsk);
  const [value, setValue] = useState("");
  const at = value ? new Date(value).getTime() : Number.NaN;
  const valid = at > Date.now();
  return (
    <Dialog.Root
      open={!!id}
      onOpenChange={(open) => {
        if (!open) askCustomSnooze(null);
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <Dialog.Content
          aria-describedby={undefined}
          className="elevated fixed top-1/2 left-1/2 z-50 flex w-[min(360px,calc(100vw-48px))] -translate-x-1/2 -translate-y-1/2 flex-col gap-3 p-5 rc-in"
        >
          <Dialog.Title className="font-semibold text-body">Snooze until</Dialog.Title>
          <input
            type="datetime-local"
            aria-label="Wake time"
            value={value}
            onChange={(e) => setValue(e.target.value)}
            className={inputClass}
          />
          <div className="flex justify-end gap-2">
            <Pill onClick={() => askCustomSnooze(null)}>Cancel</Pill>
            <Pill
              prominent
              disabled={!valid}
              onClick={() => {
                if (id && valid) snoozeThread(id, at);
                setValue("");
                askCustomSnooze(null);
              }}
            >
              Snooze
            </Pill>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
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
  const status = useApp((s) => statusOf(s, session.id));
  const worktree = useApp((s) => worktreeOf(s, session.id));
  const statusMissing = useApp((s) => (s.features.status ? null : featureMissing("status")));
  const snoozed = useApp((s) => session.id in s.snoozed);
  const allPinned = useApp((s) => (bulk ?? []).every((x) => s.pinned.includes(x.id)));
  const canFiles = hasShell();
  // The selection's menu: every hook is read above, so the early return keeps the hooks in order.
  if (bulk && bulk.length > 1) {
    const n = bulk.length;
    const ids = bulk.map((x) => x.id);
    return (
      <>
        <div className={menuLabel}>{n} threads selected</div>
        <Item
          className={menuItem}
          onSelect={() => {
            settleThreads(bulk);
            onClear?.();
          }}
        >
          <CircleCheck size={13} /> Settle {n} Threads
        </Item>
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
      {status !== "done" && (
        <Item className={menuItem} onSelect={() => settleThread(session, status)}>
          <CircleCheck size={13} /> Settle
        </Item>
      )}
      {(status === "review" || status === "done") && (
        <Item className={menuItem} onSelect={() => void setThreadStatus(session.id, "idle")}>
          <CircleDashed size={13} /> Clear Status
        </Item>
      )}
      {statusMissing && (
        <div className="px-2.5 pb-1 text-caption text-fg-faint">Kept by the app: {statusMissing}</div>
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
            <Menu.SubContent sideOffset={4} className={menuSurface}>
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
      <Menu.Sub>
        <Menu.SubTrigger className={menuItem}>
          <Copy size={13} /> Copy
          <ChevronRight size={11} strokeWidth={2.5} className="ml-auto text-fg-faint" />
        </Menu.SubTrigger>
        <Menu.Portal>
          <Menu.SubContent sideOffset={4} className={menuSurface}>
            {session.cwd && (
              <Item
                className={menuItem}
                onSelect={() => copyText(worktree?.path ?? session.cwd!, "Path copied")}
              >
                Path
              </Item>
            )}
            <Item className={menuItem} onSelect={() => copyText(session.id, "Thread ID copied")}>
              Thread ID
            </Item>
          </Menu.SubContent>
        </Menu.Portal>
      </Menu.Sub>
      {worktree && (
        <>
          <Separator className={menuSeparator} />
          <div className="flex items-center gap-1.5 px-2.5 pt-0.5 pb-1 text-caption text-fg-muted">
            <GitBranch size={11} /> <span className="truncate font-mono">{worktree.branch}</span>
          </div>
          <Item className={menuItem} disabled={!canFiles} onSelect={() => revealPath(worktree.path)}>
            <FolderOpen size={13} /> Open in Finder
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

/** A running turn's clock, ticking on its own so the row does not re-render every second. */
function Elapsed({ since }: { since: number }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  return <>{formatClock(now - since)}</>;
}

/** A small icon button in a row's hover slot. */
function RowAction({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <Tooltip label={label}>
      <button
        type="button"
        aria-label={label}
        onClick={(e) => {
          e.stopPropagation();
          onClick();
        }}
        className="flex size-[22px] items-center justify-center rounded-sm text-fg-muted hover:bg-fill hover:text-fg"
      >
        {children}
      </button>
    </Tooltip>
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

/**
 * Marks several threads Done at once: one toast, and an Undo that puts each one back the way it
 * was (review stays review, the rest go back to idle).
 */
function settleThreads(sessions: SessionSummary[]) {
  const state = useApp.getState();
  const before = sessions
    .map((s) => ({ id: s.id, status: statusOf(state, s.id) }))
    .filter((x) => x.status !== "done");
  if (!before.length) {
    toast("Already settled", "info");
    return;
  }
  void setThreadsStatus(
    before.map((x) => x.id),
    "done",
  );
  toast(`Settled ${before.length} threads`, "info", {
    action: {
      label: "Undo",
      run: () => {
        const review = before.filter((x) => x.status === "review").map((x) => x.id);
        const rest = before.filter((x) => x.status !== "review").map((x) => x.id);
        if (review.length) void setThreadsStatus(review, "review");
        if (rest.length) void setThreadsStatus(rest, "idle");
      },
    },
  });
}

/** Marks a thread Done, with an Undo that puts back the status it had. */
function settleThread(session: SessionSummary, before: ThreadStatus | undefined) {
  void setThreadStatus(session.id, "done");
  toast(`Settled "${session.title || "New thread"}"`, "info", {
    action: {
      label: "Undo",
      run: () => void setThreadStatus(session.id, before === "review" ? "review" : "idle"),
    },
  });
}

// Memoized, and reads only the flags it shows: a streamed chunk must not re-render 500 rows.
// Two lines: where it runs, its branch, what the branch's pull request became and what the thread
// is doing above; the title and the harness mark below. Hovering swaps the top line's right slot
// for the row's actions, so nothing shifts.
const ThreadRow = memo(function ThreadRow({
  session,
  selected,
  grouped,
  now,
  snippet,
  slim,
  state,
  inset,
  pull,
  fold,
  navActive,
  picked,
  bulk,
  onClearSelection,
}: {
  session: SessionSummary;
  selected: boolean;
  /** Under its project's heading: the row leaves out the mark and the name it would repeat. */
  grouped: boolean;
  now: number;
  /** While searching: the words around the match in what was said. */
  snippet?: string;
  /** A child of the thread above it: one line, no meta line, what it is doing and its age. */
  slim?: boolean;
  /** A child's state as its parent's group reads it (state/children.ts), for its dot. */
  state?: ChildInfo["state"];
  /** A grandchild's extra inset under its own group's rows (state/children.ts). */
  inset?: string;
  /** The thread's branch's pull request, from the project's read (state/pulls.ts). */
  pull?: PullRequest;
  /**
   * Threads this one started, which the sidebar lists under it: the chevron that
   * folds them into the row, and the dot for what the fold hides.
   */
  fold?: {
    open: boolean;
    /** How the group reads in one line, for the chevron's tooltip. */
    summary: string;
    /** Children waiting to be read, when the fold is closed over them. */
    dot?: "failed";
    onToggle: () => void;
  };
  /** The row the keyboard is on: it holds the list's one tab stop, ↑ ↓ move it. */
  navActive?: boolean;
  /** In the sidebar's multi-selection (⌘-click), which is not the same as the thread being open. */
  picked?: boolean;
  /** The picked threads, when there is more than one: this row's menu then acts on all of them. */
  bulk?: SessionSummary[];
  onClearSelection?: () => void;
}) {
  const request = useApp((s) => s.threads[session.id]?.permissions[0]);
  const running = useApp((s) => s.threads[session.id]?.running ?? false);
  const startedAt = useApp((s) => s.threads[session.id]?.turnStartedAt);
  const waiting = useApp((s) => s.threads[session.id]?.waiting ?? 0);
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
  // Done or Failed while you were elsewhere, until you open it.
  const fresh = !!finishedAt && !selected;
  const failed = !!failedAt && !selected;
  const [renaming, setRenaming] = useState(false);
  const [title, setTitle] = useState("");
  const external = !!session.harness && session.harness !== "rustcode";
  const harness = external ? harnesses?.harnesses.find((h) => h.id === session.harness) : undefined;
  // Where the row runs: the worktree's repository, else the thread's own folder.
  const project = projectName(worktree?.repo || session.cwd || "");
  // The path, the harness and the model the row no longer spells out, on its tooltip.
  const details = [
    session.cwd ? displayPath(session.cwd) : undefined,
    harness?.name ?? (external ? session.harness : undefined),
    modelName(session.model),
  ].filter(Boolean);
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
  if (slim) {
    // A child's row: one line and no meta line, a dot (or a spinner) where a harness mark would
    // be, its title in the secondary label and its age at the end.
    const mark =
      state ??
      (request || status === "needs_input"
        ? "needs"
        : running || waiting
          ? "working"
          : failedAt
            ? "failed"
            : "finished");
    return (
      <ContextMenu.Root>
        <ContextMenu.Trigger asChild>
          <div className="group relative">
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
                }
              }}
              aria-current={selected ? "page" : undefined}
              aria-pressed={picked || undefined}
              className={cx(
                "relative flex h-8 w-full items-center gap-1.5 rounded-lg pr-2.5 text-left",
                // A grandchild sits one inset step further in, in the same group (state/children.ts).
                inset ?? "pl-8",
              )}
            >
              <RowFill selected={selected} picked={picked} radius="rounded-lg" />
              <span className="relative flex size-2 shrink-0 items-center justify-center">
                {mark === "working" ? (
                  <Spinner size={10} className="text-accent" />
                ) : (
                  <span
                    role="img"
                    aria-label={mark === "needs" ? "Needs you" : mark === "failed" ? "Failed" : "Finished"}
                    className={cx(
                      "rounded-full",
                      mark === "finished" ? "size-1 bg-fg-faint" : "size-1.5",
                      mark === "needs" ? "bg-warning" : mark === "failed" ? "bg-danger" : "bg-fg-faint",
                    )}
                  />
                )}
              </span>
              <span
                className={cx(
                  "relative min-w-0 flex-1 truncate text-body",
                  selected || picked || mark === "needs" || mark === "working"
                    ? "font-medium text-fg"
                    : "text-fg-muted",
                )}
              >
                {session.title || "New thread"}
              </span>
              <span className="relative shrink-0 text-fg-faint text-sub tabular">
                {running && startedAt ? <Elapsed since={startedAt} /> : shortAge(session.updatedAt, now)}
              </span>
            </button>
          </div>
        </ContextMenu.Trigger>
        <ContextMenu.Portal>
          <ContextMenu.Content className={menuSurface}>
            <ThreadMenuItems
              session={session}
              Menu={ContextMenu}
              Item={ContextMenu.Item}
              Separator={ContextMenu.Separator}
              onRename={startRename}
              bulk={bulk}
              onClear={onClearSelection}
            />
          </ContextMenu.Content>
        </ContextMenu.Portal>
      </ContextMenu.Root>
    );
  }
  return (
    <ContextMenu.Root>
      <ContextMenu.Trigger asChild>
        <div className="group relative">
          {fold && (
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation();
                fold.onToggle();
              }}
              aria-expanded={fold.open}
              aria-label={
                fold.open ? "Hide the threads this one started" : "Show the threads this one started"
              }
              title={fold.summary}
              tabIndex={-1}
              className="absolute top-[7px] left-1.5 z-10 flex size-4 items-center justify-center rounded-sm text-fg-muted hover:bg-fill hover:text-fg"
            >
              {/* The mark says which way the group goes; a turned one keeps the transform it had. */}
              {fold.open ? (
                <ChevronDown size={12} strokeWidth={2.5} className="shrink-0" />
              ) : (
                <ChevronRight size={12} strokeWidth={2.5} className="shrink-0" />
              )}
            </button>
          )}
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
              }
              // → opens what the row holds and ← closes it, the same two keys a file tree takes.
              if (fold && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
                if ((e.key === "ArrowRight") !== fold.open) {
                  e.preventDefault();
                  fold.onToggle();
                }
              }
            }}
            aria-current={selected ? "page" : undefined}
            aria-pressed={picked || undefined}
            title={details.join(" · ")}
            className={cx(
              "relative flex w-full flex-col justify-center gap-0.5 rounded-xl py-2 pr-2.5 text-left",
              fold ? "pl-6" : "pl-2.5",
            )}
          >
            <RowFill selected={selected} picked={picked} radius="rounded-xl" />
            {/* Where it runs, what its branch carries, and what it is doing. */}
            <span className="relative flex h-[17px] w-full min-w-0 items-center gap-1 group-hover:pr-11 group-has-[:focus-visible]:pr-11 group-has-[[data-state=open]]:pr-11">
              {!grouped && (
                <>
                  <ProjectMark name={project} />
                  {/* The name keeps its room; the branch, the counts and the pull request give way. */}
                  <span className="min-w-0 max-w-[50%] shrink-0 truncate text-fg-muted text-sub">
                    {project}
                  </span>
                </>
              )}
              {worktree && (
                <span className="flex min-w-0 shrink items-center gap-0.5 rounded-xs bg-fill-subtle px-1 font-mono text-[10px] text-fg-muted">
                  <GitBranch size={9} className="shrink-0" />
                  <span className="truncate">{worktree.branch}</span>
                </span>
              )}
              {diff && (
                <span className="shrink-0 font-mono text-[10px] tabular">
                  {diff.added > 0 && <span className="text-success">+{diff.added}</span>}
                  {diff.added > 0 && diff.removed > 0 && " "}
                  {diff.removed > 0 && <span className="text-danger">−{diff.removed}</span>}
                </span>
              )}
              {pull && <PullMark pull={pull} />}
              <span className="ml-auto flex shrink-0 items-center gap-1 text-sub tabular group-hover:invisible group-has-[:focus-visible]:invisible group-has-[[data-state=open]]:invisible">
                {request ? (
                  <>
                    {request.question ? (
                      <MessageCircleQuestion size={12} className="text-accent" />
                    ) : (
                      <ShieldQuestion size={12} className="text-warning" />
                    )}
                    <span className={cx("font-medium", request.question ? "text-accent" : "text-warning")}>
                      {request.question ? "Input" : "Approval"}
                    </span>
                  </>
                ) : running ? (
                  <>
                    <Spinner size={11} />
                    <span className="text-accent">
                      {startedAt ? <Elapsed since={startedAt} /> : "Working"}
                    </span>
                  </>
                ) : wait ? (
                  // The titles go in the title, which the app reads out as the row's label (lib/quietNative.ts).
                  <span
                    className="flex items-center gap-1 text-fg-muted"
                    title={
                      waitingTitle(wait) ? `${waitingLabel(wait)}: ${waitingTitle(wait)}` : waitingLabel(wait)
                    }
                  >
                    <span className="relative flex size-2 items-center justify-center">
                      <span className="absolute size-2 rounded-full bg-accent/40 rc-breathe" />
                      <span className="size-1.5 rounded-full bg-accent" />
                    </span>
                    Waiting
                  </span>
                ) : failed ? (
                  <>
                    <CircleAlert size={12} className="text-danger" />
                    <span className="font-medium text-danger">Failed</span>
                  </>
                ) : fresh ? (
                  <>
                    <CircleCheck size={12} className="text-success" />
                    <span className="font-medium text-success">Done</span>
                  </>
                ) : (
                  <>
                    {status === "review" && <StatusChip status={status} compact />}
                    <span className="text-fg-muted">
                      {wake ? wakeLabel(wake, now) : shortAge(session.updatedAt, now)}
                    </span>
                  </>
                )}
              </span>
            </span>
            {/* Its title, and the harness that runs it. */}
            <span className="relative flex w-full min-w-0 items-center gap-1.5">
              {pinned && <Pin size={11} className="shrink-0 rotate-45 text-fg-faint" />}
              <span
                className={cx(
                  "min-w-0 flex-1 truncate text-body",
                  // What wants you reads strong; what works, waits or has been read recedes.
                  selected || picked || request || fresh || failed
                    ? "font-medium text-fg"
                    : done || running || wait
                      ? "font-normal text-fg/75 group-hover:text-fg"
                      : "font-medium text-fg",
                )}
              >
                {session.title || "New thread"}
              </span>
              {fold?.dot && (
                <span
                  role="img"
                  aria-label="A child thread failed"
                  className="size-1.5 shrink-0 rounded-full bg-danger"
                />
              )}
              <LinkBadge threadId={session.id} childrenListed={!!fold} />
              {external && (
                <HarnessIcon
                  id={session.harness!}
                  name={harness?.name}
                  size={13}
                  className="shrink-0 text-fg-muted"
                />
              )}
            </span>
            {/* What it asks of you, or while searching, the words around the match. */}
            {(snippet || request) && (
              <span className="relative truncate text-fg-muted text-sub">
                {snippet ? `“${snippet}”` : request!.question ? request!.question.text : request!.title}
              </span>
            )}
          </button>
          <div className="absolute top-1 right-1.5 hidden items-center group-hover:flex group-has-[:focus-visible]:flex group-has-[[data-state=open]]:flex">
            <RowAction label={pinned ? "Unpin" : "Pin"} onClick={() => togglePin(session.id)}>
              {pinned ? <PinOff size={12} /> : <Pin size={12} />}
            </RowAction>
            {!done && (
              <RowAction label="Settle" onClick={() => settleThread(session, status)}>
                <Check size={13} />
              </RowAction>
            )}
            <DropdownMenu.Root modal={false}>
              <DropdownMenu.Trigger asChild>
                <button
                  type="button"
                  aria-label={`Actions for ${session.title || "thread"}`}
                  className="flex size-[22px] items-center justify-center rounded-sm text-fg-muted hover:bg-fill hover:text-fg"
                >
                  <Ellipsis size={13} />
                </button>
              </DropdownMenu.Trigger>
              <DropdownMenu.Portal>
                <DropdownMenu.Content align="start" sideOffset={4} className={menuSurface}>
                  <ThreadMenuItems
                    session={session}
                    Menu={DropdownMenu}
                    Item={DropdownMenu.Item}
                    Separator={DropdownMenu.Separator}
                    onRename={startRename}
                    bulk={bulk}
                    onClear={onClearSelection}
                  />
                </DropdownMenu.Content>
              </DropdownMenu.Portal>
            </DropdownMenu.Root>
          </div>
        </div>
      </ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenu.Content className={menuSurface}>
          <ThreadMenuItems
            session={session}
            Menu={ContextMenu}
            Item={ContextMenu.Item}
            Separator={ContextMenu.Separator}
            onRename={startRename}
            bulk={bulk}
            onClear={onClearSelection}
          />
        </ContextMenu.Content>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
});

/** A parent's children as the list draws them: its rows, the fold and what it hides. */
interface ChildGroup {
  /** The children's threads, and the grandchildren drawn flat under them, for their rows. */
  items: SessionSummary[];
  /** What the group lists, in draw order: what wants you, what works, what you opened. */
  rows: ChildInfo[];
  /** The finished ones behind the group's last line. */
  folded: ChildInfo[];
  /** Whether that line is drawn at all. */
  fold: boolean;
  /** Whether the reader opened it. */
  foldOpen: boolean;
  /** Whether the group lists anything: what its parent row's chevron says. */
  listed: boolean;
  /** A child failed (or is about to report that it failed) while you were reading elsewhere. */
  dot?: "failed";
  /** How the group reads in one line, for the chevron's tooltip. */
  summary: string;
  /** Folds the whole group away, or lists it again. */
  toggle: () => void;
  onFold: (open: boolean) => void;
}

/**
 * Threads this one started (start_session, fan_out), under it, as slim rows: what wants you,
 * then what works, then the one you opened, and the finished ones behind one quiet last line
 * ("Show 3 finished") once folding them saves something. The parent row's chevron folds the lot
 * away (state/children.ts).
 */
function ChildThreads({
  group,
  selectedId,
  now,
  activeId,
  picked,
  bulk,
  onClearSelection,
}: {
  group: ChildGroup;
  selectedId: string | null;
  now: number;
  /** The row the keyboard is on, the picked threads and how to clear them (ui/Sidebar.tsx). */
  activeId: string | null;
  picked: ReadonlySet<string>;
  bulk: SessionSummary[];
  onClearSelection: () => void;
}) {
  const row = (child: ChildInfo) => {
    const session = group.items.find((c) => c.id === child.id);
    return session ? (
      <ThreadRow
        key={child.id}
        session={session}
        selected={child.id === selectedId}
        grouped={false}
        now={now}
        slim
        state={child.state}
        inset={child.level === 1 ? GRANDCHILD_INSET : undefined}
        navActive={child.id === activeId}
        picked={picked.has(child.id)}
        bulk={bulk.length > 1 && picked.has(child.id) ? bulk : undefined}
        onClearSelection={onClearSelection}
      />
    ) : null;
  };
  return (
    <div className="flex flex-col gap-0.5 pb-1">
      {group.rows.map(row)}
      {group.fold && (
        <button
          type="button"
          onClick={() => group.onFold(!group.foldOpen)}
          aria-expanded={group.foldOpen}
          className="flex h-7 w-full items-center rounded-lg pr-2.5 pl-8 text-left text-fg-faint text-sub hover:bg-fill-subtle hover:text-fg-muted"
        >
          {group.foldOpen ? "Hide finished" : `Show ${group.folded.length} finished`}
        </button>
      )}
      {group.fold && group.foldOpen && group.folded.map(row)}
    </div>
  );
}

function useNow() {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(t);
  }, []);
  return now;
}

/** A small preference kept in localStorage; it reads back empty in a private window. */
function useStored<T>(key: string, initial: T) {
  const [value, setValue] = useState<T>(() => {
    try {
      const raw = localStorage.getItem(key);
      return raw === null ? initial : (JSON.parse(raw) as T);
    } catch {
      return initial;
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem(key, JSON.stringify(value));
    } catch {
      // Not kept in a private window.
    }
  }, [key, value]);
  return [value, setValue] as const;
}

type SectionId = "working" | "pinned" | "threads" | "snoozed" | "settled";

/** Sections with a header; the rest are simply part of the list. */
const HEADED: SectionId[] = ["working", "snoozed"];

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
        "group/h flex h-8 w-full items-center gap-2 rounded-lg px-2.5 text-left font-medium text-fg-muted text-sub hover:bg-fill-subtle",
      )}
    >
      <span className="min-w-0 flex-1 truncate">{section.label}</span>
      {!open && <HiddenDot kind={section.hidden} />}
      {/* One slot: an open section shows its count, and the chevron in its place on hover (macOS). */}
      <span className="grid shrink-0 items-center justify-items-end">
        <span
          className={cx(
            "font-normal text-fg-faint tabular [grid-area:1/1]",
            open && "group-hover/h:opacity-0 group-focus-visible/h:opacity-0",
            !open && "hidden",
          )}
        >
          {section.items.length}
        </span>
        {!open && (
          <span className="flex items-center gap-1.5 [grid-area:1/1]">
            <span className="font-normal text-fg-faint tabular">{section.items.length}</span>
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

/**
 * A project's folder: the folder and its name, a dot for
 * what it hides and its count only while folded, the chevron at the end, and
 * a new-thread button on hover.
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
    <div className="group/p relative flex h-8 items-center">
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        className="flex h-full min-w-0 flex-1 items-center gap-2 rounded-md px-2.5 text-left font-medium text-body text-fg/90 hover:bg-fill-subtle"
      >
        {open ? (
          <FolderOpen size={14} className="shrink-0 text-fg-muted" />
        ) : (
          <Folder size={14} className="shrink-0 text-fg-muted" />
        )}
        <span className="min-w-0 flex-1 truncate">{label}</span>
        {!open && <HiddenDot kind={hidden} />}
        {!open && <span className="font-normal text-fg-faint text-sub tabular">{count}</span>}
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

function FilterMenu({
  byProject,
  setByProject,
  showWorking,
  setShowWorking,
  onlyProject,
  setOnlyProject,
}: {
  byProject: boolean;
  setByProject: (v: boolean) => void;
  showWorking: boolean;
  setShowWorking: (v: boolean) => void;
  onlyProject: boolean;
  setOnlyProject: (v: boolean) => void;
}) {
  const projectCwd = useApp((s) => s.projectCwd);
  const view = useApp((s) => s.view.kind);
  const on = onlyProject;
  const check = (value: boolean) => (
    <span className="flex w-3.5 justify-center">{value && <Check size={12} strokeWidth={2.5} />}</span>
  );
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <ClusterButton label="Filter and view" tooltip="Filter and view" on={on}>
          <ListFilter size={14} />
        </ClusterButton>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content align="end" sideOffset={6} className={menuSurface}>
          <DropdownMenu.Item className={menuItem} onSelect={() => setByProject(!byProject)}>
            {check(byProject)} Group by project
          </DropdownMenu.Item>
          <DropdownMenu.Item
            className={menuItem}
            disabled={byProject}
            onSelect={() => setShowWorking(!showWorking)}
          >
            {check(showWorking)} Working section
          </DropdownMenu.Item>
          <DropdownMenu.Item
            className={menuItem}
            disabled={!projectCwd}
            onSelect={() => setOnlyProject(!onlyProject)}
          >
            {check(onlyProject)}
            <span className="truncate">Only {projectCwd ? projectName(projectCwd) : "this project"}</span>
          </DropdownMenu.Item>
          <DropdownMenu.Separator className={menuSeparator} />
          <DropdownMenu.Item
            className={menuItem}
            onSelect={() => setView(view === "board" ? { kind: "chat" } : { kind: "board" })}
          >
            <KanbanSquare size={13} /> {view === "board" ? "Show the Chat" : "Show the Board"}
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

const NO_MARKS: Record<string, ThreadStatus> = {};

/** Nothing picked: one set, so clearing an empty selection never re-renders the list. */
const NO_PICKS: ReadonlySet<string> = new Set();

/**
 * The sidebar's search: a row that reads "Search" until it is clicked (or ⌘F
 * while the sidebar has the keyboard), then the field itself. Never a pill: it
 * is a row among the icon buttons beside it, the same height and radius.
 */
function SidebarSearch({
  query,
  setQuery,
  open,
  setOpen,
}: {
  query: string;
  setQuery: (query: string) => void;
  open: boolean;
  setOpen: (open: boolean) => void;
}) {
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (open) input.current?.focus();
  }, [open]);
  if (!open && !query) {
    return (
      <button
        type="button"
        onClick={() => setOpen(true)}
        aria-label="Search threads"
        className="flex h-7 min-w-0 flex-1 items-center gap-2 rounded-md px-2 text-left text-callout text-fg-muted hover:bg-fill hover:text-fg"
      >
        <Search size={14} className="shrink-0" />
        <span className="truncate">Search</span>
      </button>
    );
  }
  const clear = () => (query ? setQuery("") : setOpen(false));
  return (
    <div className="flex h-7 min-w-0 flex-1 items-center gap-2 rounded-md bg-fill px-2">
      <Search size={14} className="shrink-0 text-fg-muted" />
      <input
        ref={input}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") clear();
        }}
        // An empty search closes with the keyboard: a field with nothing in it is a label.
        onBlur={() => {
          if (!query) setOpen(false);
        }}
        placeholder="Search"
        aria-label="Search threads"
        className="min-w-0 flex-1 bg-transparent text-callout outline-none placeholder:text-fg-faint"
      />
      <button
        type="button"
        aria-label={query ? "Clear search" : "Close search"}
        onClick={clear}
        className="shrink-0 text-fg-faint hover:text-fg-muted"
      >
        <X size={12} />
      </button>
    </div>
  );
}

/**
 * The shelf at the foot of the sidebar: Settled says how many threads it holds
 * and opens them where they lie, at the end of the list above it.
 */
function SettledRow({
  count,
  open,
  hidden,
  onToggle,
}: {
  count: number;
  open: boolean;
  hidden?: HiddenKind;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onToggle}
      aria-expanded={open}
      disabled={!count}
      className="flex h-8 w-full items-center gap-2 rounded-md px-2.5 text-left text-callout text-fg-muted hover:bg-fill-subtle hover:text-fg disabled:cursor-default disabled:hover:bg-transparent disabled:hover:text-fg-muted"
    >
      <span className="min-w-0 flex-1 truncate">Settled</span>
      {!open && <HiddenDot kind={hidden} />}
      <span className="shrink-0 text-fg-faint text-sub tabular">{count}</span>
      {/* Two marks rather than one turned by a class: a rotated icon keeps the transform it had. */}
      {open ? (
        <ChevronDown size={12} strokeWidth={2.5} className="shrink-0 text-fg-faint" />
      ) : (
        <ChevronRight size={12} strokeWidth={2.5} className="shrink-0 text-fg-faint" />
      )}
    </button>
  );
}

export function Sidebar() {
  const sessions = useApp((s) => s.sessions);
  const sessionsLoaded = useApp((s) => s.sessionsLoaded);
  const selectedId = useApp((s) => s.selectedId);
  // Which threads need you or work, as a string so a streamed chunk does not change it.
  const flags = useApp((s) =>
    Object.values(s.threads)
      .map((t) => (t.permissions.length ? `${t.id}:n` : t.running ? `${t.id}:w` : ""))
      .join(" "),
  );
  const marks = useApp((s) => s.statusMarks ?? NO_MARKS);
  const finished = useApp((s) => s.finished);
  const projectCwd = useApp((s) => s.projectCwd);
  const hidden = useApp((s) => s.sidebarHidden);
  const toggleKey = useShortcutHint("toggle-sidebar");
  const newKey = useShortcutHint("new-thread");
  const pinned = useApp((s) => s.pinned);
  const [query, setQuery] = useState("");
  // The search row is a label until it is used: ⌘F while the sidebar has the keyboard opens it.
  const [searchOpen, setSearchOpen] = useState(false);
  // Each thread's branch's pull request, from the projects' one read (state/pulls.ts).
  const pullsOn = useApp((s) => !!s.agent?.pullRequests);
  // The titlebar's icon buttons open the same places the header's do.
  const panel = useApp((s) => s.panel);
  // The cache's version: a landed read re-renders the rows that read it.
  useSyncExternalStore(subscribePulls, pullsVersion);
  // By state, with the project on each row; grouping by project is an option.
  const [byProject, setByProject] = useStored("rustcode.sidebar.groupByProject", false);
  const [showWorking, setShowWorking] = useStored("rustcode.sidebar.workingSection", true);
  const snoozed = useApp((s) => s.snoozed);
  useEffect(() => {
    wakeSnoozed();
    const t = setInterval(() => wakeSnoozed(), 15_000);
    return () => clearInterval(t);
  }, []);
  const [onlyProject, setOnlyProject] = useStored("rustcode.sidebar.onlyProject", false);
  const [folded, setFolded] = useStored<string[]>("rustcode.sidebar.folded", []);
  // The shelf at the foot has its own switch: it is a row of the sidebar's, not of the list.
  const [settledOpen, setSettledOpen] = useStored("rustcode.sidebar.settledOpen", false);
  // Which child groups are open, by parent: one record for the whole sidebar, written in one
  // place. A copy per group wrote the whole record back, so opening one group erased another.
  const [openGroups, setOpenGroups] = useStored<Record<string, boolean>>("rustcode.sidebar.childrenOpen", {});
  const listedIds = useMemo(() => sessions.map((s) => s.id), [sessions]);
  const setGroupOpen = useCallback(
    (id: string, open: boolean) =>
      setOpenGroups((groups) => ({ ...pruneOpenGroups(groups, listedIds), [id]: open })),
    [listedIds, setOpenGroups],
  );
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
  // Settled lists 10 threads, then 25 more at a time.
  const [settledShown, setSettledShown] = useState(10);
  // design-lint: allow (mask alpha, not a colour)
  const fade = "linear-gradient(to bottom, #000 calc(100% - 12px), transparent)";

  // What was said in a thread matches too: the engine's index answers once the query has three
  // characters, a moment after typing stops.
  const [contentHits, setContentHits] = useState<Record<string, string>>({});
  useEffect(() => {
    const q = query.trim();
    if (q.length < 3) {
      setContentHits({});
      return;
    }
    let live = true;
    const t = setTimeout(() => void searchThreads(q).then((hits) => live && setContentHits(hits)), 200);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [query]);
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return sessions.filter(
      (s) =>
        (!onlyProject || !projectCwd || s.cwd === projectCwd || s.worktree?.repo === projectCwd) &&
        (!q || `${s.title ?? ""} ${s.cwd}`.toLowerCase().includes(q) || s.id in contentHits),
    );
  }, [sessions, query, onlyProject, projectCwd, contentHits]);

  const links = useApp((s) => s.links.links);
  // Threads another thread started (start_session, fan_out) sit under it.
  const parentOf = useMemo(() => {
    const listed = new Set(filtered.map((s) => s.id));
    const map: Record<string, string> = {};
    for (const l of links) if (l.started && listed.has(l.orchestrator)) map[l.target] = l.orchestrator;
    return map;
  }, [links, filtered]);
  const childrenOf = useMemo(() => {
    const map: Record<string, SessionSummary[]> = {};
    for (const s of filtered) {
      const parent = parentOf[s.id];
      if (parent) map[parent] = [...(map[parent] ?? []), s];
    }
    return map;
  }, [filtered, parentOf]);

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
    const pins: SessionSummary[] = [];
    const isFresh = (id: string) => !!finished[id];
    const active: SessionSummary[] = [];
    const settled: SessionSummary[] = [];
    const working: SessionSummary[] = [];
    const sleeping: SessionSummary[] = [];
    for (const s of filtered) {
      if (parentOf[s.id]) continue;
      const listed = s.status === "done" || s.status === "review" ? s.status : "idle";
      // Settled: threads marked Done (by the user, or the engine for a merged branch), and
      // threads untouched for three days unless they wait for review.
      const status = marks[s.id] ?? listed;
      const done = status === "done" || (status !== "review" && idleSettled(s.updatedAt, now));
      // A pinned thread always shows, whatever it is doing.
      // One list, each row saying what it does; only settled threads leave it.
      if (pinned.includes(s.id)) pins.push(s);
      // Snoozed until its time, unless it needs you or works in the meantime.
      else if (snoozed[s.id] && !flagged.has(`${s.id}:n`) && !flagged.has(`${s.id}:w`)) sleeping.push(s);
      // Folders already separate the work, so grouping by project has no Working section.
      else if (showWorking && !byProject && flagged.has(`${s.id}:w`)) working.push(s);
      else if (done && !flagged.has(`${s.id}:n`) && !flagged.has(`${s.id}:w`) && !isFresh(s.id))
        settled.push(s);
      else active.push(s);
    }
    pins.sort((a, b) => pinned.indexOf(a.id) - pinned.indexOf(b.id));
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
    sleeping.sort((a, b) => (snoozed[a.id] ?? 0) - (snoozed[b.id] ?? 0));
    const all: Section[] = [
      { id: "working", label: "Working", items: working },
      { id: "pinned", label: "Pinned", items: pins },
      { id: "threads", label: "Threads", items: active, projects },
      { id: "snoozed", label: "Snoozed", items: sleeping },
      { id: "settled", label: "Settled", items: settled },
    ];
    for (const g of all) {
      g.hidden = g.items.some((x) => flagged.has(`${x.id}:n`))
        ? "needs"
        : g.items.some((x) => flagged.has(`${x.id}:w`))
          ? "working"
          : g.items.some((x) => isFresh(x.id))
            ? "finished"
            : undefined;
    }
    return all.filter((g) => g.items.length);
  }, [filtered, flags, marks, finished, pinned, parentOf, byProject, now, snoozed, showWorking]);

  const footShelf = sections.some((g) => g.id === "snoozed") ? "snoozed" : "settled";
  // A search shows every match, folded sections too.
  const isOpen = (id: string) =>
    id === "settled" ? !!query.trim() || settledOpen : !!query.trim() || !folded.includes(id);
  const toggle = (id: string) =>
    id === "settled"
      ? setSettledOpen(!settledOpen)
      : setFolded((f) => (f.includes(id) ? f.filter((x) => x !== id) : [...f, id]));
  // Settled is the shelf at the foot (ui/Sidebar.tsx SettledRow): its header sits in the footer,
  // and its rows stay at the end of the list above it.
  const settledSection = sections.find((g) => g.id === "settled");

  // ---------------------------------------------------------------- the list's rows, selection and keys
  // What is picked, and which row the keyboard is on, belong to the sidebar while it is drawn:
  // nothing outside it reads them, and a thread that leaves the list leaves the selection.
  const [picked, setPicked] = useState<ReadonlySet<string>>(NO_PICKS);
  const [activeId, setActiveId] = useState<string | null>(null);
  // The finished children a group shows, by parent: here, so the keyboard order can include them.
  const [finishedOpen, setFinishedOpen] = useState<Record<string, boolean>>({});
  const anchor = useRef<string | null>(null);
  const list = useRef<HTMLElement>(null);
  // Every child's state, as its group reads it: one string, so a streamed chunk that changes
  // nothing about them does not redraw the list.
  const childStates = useApp((s) => {
    const linked = new Map(s.links.links.filter((l) => l.started).map((l) => [l.target, l.state] as const));
    return filtered
      .map((c) => {
        if (!parentOf[c.id]) return "";
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

  // A parent's children as the list draws them: one place, so the parent row's chevron, the rows
  // under it and the keyboard order all agree on what is on screen.
  const group = useCallback(
    (parentId: string): ChildGroup => {
      const items = childrenOf[parentId] ?? [];
      // A grandchild's own group would be a second chevron under a row that is already small, so
      // it is drawn flat in this group, one inset further in (state/children.ts).
      const under = (id: string) =>
        orderChildren(
          (childrenOf[id] ?? []).flatMap((c) => infos.get(c.id) ?? []),
          selectedId,
        );
      // A child whose own child still works stays a row, so that grandchild is not folded away.
      const children: ChildInfo[] = [];
      for (const c of items) {
        const info = infos.get(c.id);
        if (!info) continue;
        const below = under(c.id);
        const urgent = below.some((g) => g.state === "needs")
          ? "needs"
          : below.some((g) => g.state === "working")
            ? "working"
            : undefined;
        children.push({ ...info, below: urgent });
      }
      const all = flatChildren(children, under);
      const listed = childrenListed(all, {
        expanded: openGroups[parentId] === true,
        collapsed: openGroups[parentId] === false,
        parentSelected: parentId === selectedId,
        selectedId,
      });
      const closed = openGroups[parentId] === false;
      const ordered = orderChildren(children, selectedId);
      const { rows, folded, fold } = foldFinished(ordered, selectedId, closed);
      return {
        // A grandchild's thread, so its row can find what it draws.
        items: [...items, ...items.flatMap((c) => childrenOf[c.id] ?? [])],
        rows: flatChildren(rows, under),
        folded: flatChildren(folded, under),
        fold,
        foldOpen: !!finishedOpen[parentId],
        listed,
        dot: unseenChild(all),
        summary: childSummary(all),
        toggle: () => setGroupOpen(parentId, !listed),
        onFold: (open) => setFinishedOpen((m) => ({ ...m, [parentId]: open })),
      };
    },
    [childrenOf, finishedOpen, infos, openGroups, selectedId, setGroupOpen],
  );

  // The order the list draws its rows in, so ↑ ↓ move through what is on screen: a shut section
  // or group contributes nothing, and a parent's children follow it (state/sidebarRows.ts).
  const order = useMemo<OrderSection[]>(() => {
    const open = (id: string) => !!query.trim() || !folded.includes(id);
    const kids = (parentId: string): string[] => {
      const g = group(parentId);
      if (!g.listed) return [];
      const under = g.fold && g.foldOpen ? [...g.rows, ...g.folded] : g.rows;
      return under.map((c) => c.id);
    };
    const toOrder = (items: SessionSummary[]): OrderRow[] =>
      items.map((s) => ({ id: s.id, children: kids(s.id) }));
    return sections.map((section) =>
      section.projects
        ? {
            id: section.id,
            groups: section.projects.map((p) => ({
              key: p.key,
              shut: !open(`project:${p.key}`),
              rows: toOrder(p.items),
            })),
          }
        : {
            id: section.id,
            shut: section.id === "settled" && !open(section.id),
            rows: toOrder(section.items),
            limit: section.id === "settled" && !query.trim() ? settledShown : undefined,
          },
    );
  }, [folded, group, query, sections, settledShown]);
  const orderIds = useMemo(() => visibleRows(order), [order]);

  // The row the keyboard is on: where it was left, else the open thread, else the first row.
  const currentId =
    activeId && orderIds.includes(activeId)
      ? activeId
      : selectedId && orderIds.includes(selectedId)
        ? selectedId
        : (orderIds[0] ?? null);

  useEffect(() => {
    setPicked((prev) => (prev.size ? keepListed(prev, orderIds) : prev));
  }, [orderIds]);

  const pickedSessions = useMemo(() => sessions.filter((s) => picked.has(s.id)), [sessions, picked]);
  const allPickedPinned = pickedSessions.length > 0 && pickedSessions.every((s) => pinned.includes(s.id));
  const clearSelection = useCallback(() => setPicked(NO_PICKS), []);
  const focusRow = (id: string) =>
    list.current?.querySelector<HTMLElement>(`[data-row-id="${CSS.escape(id)}"]`)?.focus();

  // ↑ ↓ ⏎ Esc over the rows; a section or group control keeps its own keys, and a field inside
  // the list (the rename box) keeps every key it is given.
  const onListKey = (e: React.KeyboardEvent) => {
    const target = e.target as HTMLElement | null;
    const action = rowKeyAction({
      key: e.key,
      metaKey: e.metaKey,
      ctrlKey: e.ctrlKey,
      altKey: e.altKey,
      editable: !!target?.closest("input, textarea, [contenteditable]"),
    });
    if (!action) return;
    if (action.kind === "clear") {
      e.preventDefault();
      clearSelection();
      return;
    }
    if (!target?.closest("[data-row-id]")) return;
    e.preventDefault();
    if (action.kind === "open") {
      if (currentId) selectThread(currentId);
      clearSelection();
      return;
    }
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
    const mod = e.metaKey || e.ctrlKey;
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

  const rows = (items: SessionSummary[], grouped: boolean) => (
    <div className="flex flex-col gap-0.5">
      {items.map((s) => {
        const kids = childrenOf[s.id]?.length ? group(s.id) : undefined;
        return (
          <div key={s.id} className="flex flex-col gap-0.5">
            <ThreadRow
              session={s}
              selected={s.id === selectedId}
              grouped={grouped}
              pull={pullOf(s)}
              now={now}
              snippet={query.trim() ? contentHits[s.id] : undefined}
              fold={
                kids && {
                  open: kids.listed,
                  summary: kids.summary,
                  dot: kids.dot,
                  onToggle: kids.toggle,
                }
              }
              navActive={s.id === currentId}
              picked={picked.has(s.id)}
              bulk={picked.size > 1 && picked.has(s.id) ? pickedSessions : undefined}
              onClearSelection={clearSelection}
            />
            {kids?.listed && (
              <ChildThreads
                group={kids}
                selectedId={selectedId}
                now={now}
                activeId={currentId}
                picked={picked}
                bulk={pickedSessions}
                onClearSelection={clearSelection}
              />
            )}
          </div>
        );
      })}
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
      // ⌘F searches threads while the sidebar has the keyboard, and the transcript's find bar
      // everywhere else: the event is stopped here, before it reaches the window.
      onKeyDownCapture={(e) => {
        if (!(e.metaKey || e.ctrlKey) || e.key.toLowerCase() !== "f") return;
        e.preventDefault();
        e.stopPropagation();
        setSearchOpen(true);
      }}
    >
      <div
        className={cx(
          "flex h-full flex-col bg-sidebar transition-[translate] duration-200 ease-snappy",
          hidden ? "-translate-x-full" : "translate-x-0",
        )}
        style={{ width, minWidth: width }}
      >
        <TitlebarRow className="justify-end pr-2.5">
          <Cluster>
            <ClusterButton
              label="Hide sidebar"
              tooltip={`Hide sidebar (${toggleKey})`}
              onClick={toggleSidebar}
            >
              <PanelLeft size={14} />
            </ClusterButton>
            <ClusterButton
              label="New thread"
              tooltip={`New thread (${newKey})`}
              onClick={() => startNewThread()}
            >
              <SquarePen size={14} />
            </ClusterButton>
            <SettingsButton />
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
          </Cluster>
        </TitlebarRow>
        <div className="mx-2 mt-2 mb-1.5 flex shrink-0 items-center gap-0.5">
          <SidebarSearch query={query} setQuery={setQuery} open={searchOpen} setOpen={setSearchOpen} />
          <FilterMenu
            byProject={byProject}
            setByProject={setByProject}
            showWorking={showWorking}
            setShowWorking={setShowWorking}
            onlyProject={onlyProject}
            setOnlyProject={setOnlyProject}
          />
          <ClusterButton
            label="Add project"
            tooltip="Add a project folder"
            onClick={() => void pickFolder(projectCwd || undefined).then((p) => p && startNewThread(p))}
          >
            <FolderPlus size={14} />
          </ClusterButton>
          <ClusterButton
            label="New thread"
            tooltip={`New thread (${newKey})`}
            onClick={() => startNewThread()}
          >
            <SquarePen size={14} />
          </ClusterButton>
        </div>
        <nav
          ref={list}
          // One tab stop for the whole list (the row the keyboard is on), ↑ ↓ through its rows,
          // ⏎ to open and Esc to let the selection go.
          onKeyDown={onListKey}
          onFocus={onListFocus}
          onClickCapture={onListClick}
          aria-label="Thread list"
          className="min-h-0 flex-1 overflow-y-auto px-2 pb-2"
          style={{ maskImage: fade, WebkitMaskImage: fade }}
        >
          {!sessionsLoaded && <ThreadRowsSkeleton />}
          {sessionsLoaded && !filtered.length && (
            <div className="px-2.5 py-3 text-callout text-fg-muted">
              {query
                ? "No matching threads."
                : onlyProject
                  ? "No threads in this project yet."
                  : "No threads yet."}
            </div>
          )}
          <div className="flex min-h-full flex-col">
            {sections.map((section) => {
              const open = section.id !== "settled" || isOpen(section.id);
              return (
                <section
                  key={section.id}
                  // Snoozed and Settled are shelves at the foot of the list.
                  className={cx("mt-2 first:mt-0", section.id === footShelf && "mt-auto pt-2")}
                  aria-label={section.label}
                >
                  {/* One list: only the working group and the shelves have a header. */}
                  {HEADED.includes(section.id) && (
                    <SectionHeader section={section} open={open} onToggle={() => toggle(section.id)} />
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
                              <Fold open={projectOpen}>
                                <div className="pl-3">{rows(p.items, true)}</div>
                              </Fold>
                            </div>
                          );
                        })
                      : rows(
                          section.id === "settled" && !query.trim()
                            ? section.items.slice(0, settledShown)
                            : section.items,
                          // Under its project's heading the row drops the mark and the name.
                          !!section.projects,
                        )}
                    {section.id === "settled" && !query.trim() && section.items.length > settledShown && (
                      <button
                        type="button"
                        onClick={() => setSettledShown((n) => n + 25)}
                        className="flex h-8 w-full items-center gap-2 rounded-lg px-2.5 text-left text-callout text-fg-muted hover:bg-fill-subtle hover:text-fg"
                      >
                        <ChevronDown size={14} className="shrink-0" />
                        Show {Math.min(25, section.items.length - settledShown)} more
                      </button>
                    )}
                    {section.id === "settled" && (
                      <p className="px-2.5 pt-1 pb-2 text-caption text-fg-faint">
                        Done, or untouched for 3 days. A new message brings one back.
                      </p>
                    )}
                  </Fold>
                </section>
              );
            })}
          </div>
        </nav>
        {picked.size > 1 && (
          <div className="flex shrink-0 items-center gap-1 px-2 pb-1">
            <span
              className="min-w-0 flex-1 truncate px-1 font-medium text-caption text-fg-muted tabular"
              aria-live="polite"
            >
              {picked.size} selected
            </span>
            <Pill
              size="sm"
              quiet
              onClick={() => {
                setPinned([...picked], !allPickedPinned);
                clearSelection();
              }}
            >
              {allPickedPinned ? "Unpin" : "Pin"}
            </Pill>
            <Pill
              size="sm"
              quiet
              onClick={() => {
                settleThreads(pickedSessions);
                clearSelection();
              }}
            >
              Settle
            </Pill>
            <Pill size="sm" quiet onClick={clearSelection}>
              Clear
            </Pill>
          </div>
        )}
        <div className="shrink-0 px-2 pb-1">
          <SettledRow
            count={settledSection?.items.length ?? 0}
            open={!!settledSection && isOpen("settled")}
            hidden={settledSection?.hidden}
            onToggle={() => toggle("settled")}
          />
        </div>
        <UpdateRow />
      </div>
      <CustomSnooze />
      {!hidden && (
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize sidebar"
          onPointerDown={onResize}
          className="group absolute top-0 right-[-3px] bottom-0 z-10 w-[7px] cursor-col-resize"
        >
          <div className="mx-auto h-full w-px bg-transparent group-hover:bg-fill" />
        </div>
      )}
    </aside>
  );
}

/** Where the thread list will be: a section header and rows of a title and a meta line. */
const SKELETON_ROWS = [72, 56, 84, 64, 48, 76, 60];
function ThreadRowsSkeleton() {
  return (
    <div className="flex flex-col pt-1" role="status" aria-label="Loading threads">
      <Skeleton className="mx-2.5 mt-2 mb-3 h-2.5 w-16" />
      {SKELETON_ROWS.map((w, i) => (
        <div key={i} className="flex flex-col gap-1.5 px-2.5 py-[7px]">
          <Skeleton className="h-3" style={{ width: `${w}%` }} />
          <Skeleton className="h-2" style={{ width: `${Math.round(w * 0.55)}%` }} />
        </div>
      ))}
    </div>
  );
}
