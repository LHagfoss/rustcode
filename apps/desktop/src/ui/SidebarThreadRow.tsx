import { ContextMenu, DropdownMenu } from "radix-ui";
import { memo, useEffect, useMemo, useRef, useState } from "react";
import type { PullRequest, SubagentInfo } from "../acp/rustcode";
import { projectName, type SessionSummary } from "../acp/types";
import { formatBinding } from "../lib/keys";
import { catalogModelTitle } from "../lib/modelNames";
import { openTerminalAt, openThreadLog, revealPath } from "../lib/native";
import { appPlatform, fileManagerName, trashName } from "../lib/platform";
import { hasShell } from "../lib/shell";
import { delegateCount, liveSubagents, type StackAgent } from "../state/agentStack";
import { autoCompactOffFor, setAutoCompactForThread } from "../state/autoCompactRun";
import { branchKindOf } from "../state/branches";
import { useCheckoutBranch } from "../state/checkoutBranch";
import { copyThreadId } from "../state/deepLinks";
import { type DelegateOrigin, delegateOriginOf, originText } from "../state/delegateOrigin";
import { hasPendingChange, useOutbox } from "../state/machineOutbox";
import {
  machineGlyph,
  machineName,
  machineTint,
  platformIcon,
  selfName,
  useLocalIdentity,
  useMachines,
} from "../state/machines";
import { registeredProjects } from "../state/projectBook";
import { pullState } from "../state/pulls";
import type { PendingSetup } from "../state/setup";
import {
  type CompactMark,
  compactMark,
  opensRowMenu,
  type RowStatus,
  type RowStatusKind,
  rowStatus,
} from "../state/sidebarRows";
import { snoozePresets, wakeLabel } from "../state/snooze";
import {
  askConfirm,
  askCustomSnooze,
  dismissPendingSetup,
  errorText,
  featureMissing,
  harnessesOn,
  markUnread,
  moveMachineBlockedReason,
  moveThreadToMachine,
  planMachineMoveDestination,
  removeWorktree,
  renameThread,
  repairThread,
  selectThread,
  setPinned,
  setThreadStatus,
  settleThread,
  settleThreads,
  setView,
  snoozeThread,
  statusOf,
  toast,
  togglePin,
  trashThread,
  unsettleThreads,
  useApp,
  useMovingMachine,
  worktreeOf,
} from "../state/store";
import { formatClock, isRunning, shortAge } from "../state/thread";
import { splitThreadKey } from "../state/threadKeys";
import { useWaiting, waitingLabel, waitingTitle } from "../state/waiting";
import { HarnessIcon } from "./HarnessIcon";
import {
  Bot,
  Check,
  ChevronRight,
  CircleAlert,
  CircleCheck,
  CircleDashed,
  CircleDot,
  Clock,
  CloudUpload,
  Copy,
  CornerUpLeft,
  Ellipsis,
  Eye,
  FileDiff,
  FileText,
  FolderOpen,
  FolderX,
  GitBranch,
  GitFork,
  GitMerge,
  GitPullRequestArrow,
  GitPullRequestClosed,
  GitPullRequestDraft,
  Laptop,
  MessageCircleQuestion,
  MessageSquareQuote,
  Pencil,
  Pin,
  PinOff,
  ShieldQuestion,
  Sparkles,
  SquareTerminal,
  Trash2,
  Wrench,
  X,
} from "./icons";
import { RowBadges } from "./LoopBadge";
import { DeviceGlyphIcon } from "./MachineGlyph";
import { RowAction } from "./ProjectHeader";
import { ProjectMark } from "./ProjectMark";
import {
  ContextMenuSurface,
  cx,
  MenuContent,
  MenuQuickActions,
  menuItem,
  menuItemDanger,
  menuLabel,
  menuSeparator,
  menuSurface,
  noTextHelpers,
  Skeleton,
  Spinner,
  surfaceCollisionPadding,
  Tooltip,
} from "./primitives";
import { SIDEBAR_ROW_CURRENT, SIDEBAR_ROW_LIFTED, SIDEBAR_ROW_RADIUS } from "./sidebarRowStyle";
import { ThreadHoverCard } from "./ThreadHoverCard";
import { ThreadQuickActions } from "./ThreadQuickActions";

// One thread row of the sidebar (ThreadRow, PendingRow), its state label and its
// context menu (ThreadMenuItems), split out of Sidebar.tsx, which lays the rows out.

export function ThreadMenuItems({
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
  // A thread on another computer: its folder, its git and its shell are there, so this menu drops
  // what would act on this computer's copy of the path (`revealPath`, `worktree_remove`, review).
  const machines = useMachines((s) => s.machines);
  const local = useLocalIdentity();
  const machineEntry = machines.find((m) => m.id === session.host);
  const machineNameText = machineEntry ? machineName(machineEntry) : undefined;
  const remote = !!session.host;
  const host = session.host ?? "";
  const rawId = splitThreadKey(session.id).id;
  const movingTo = useMovingMachine(session.id);
  const threadRunning = useApp(
    (s) => isRunning(s.threads[session.id]) || statusOf(s, session.id) === "running",
  );
  const moveBlocked = moveMachineBlockedReason({
    running: threadRunning,
    sourceHost: host,
    sourceConnected: !host || machineEntry?.state === "connected",
    sourceMachineName: machineNameText,
    moving: !!movingTo,
  });
  const localProjects = useMemo(() => (machines.length ? registeredProjects() : []), [machines.length]);
  const platform = appPlatform();
  const hereGlyph = platformIcon(platform === "mac" ? "macos" : platform);
  // The thread this one was started from, to open from here: the keyboard's way to the row's
  // "from" line, and the only one in compact rows.
  const sessions = useApp((s) => s.sessions);
  const links = useApp((s) => s.links.links);
  const origin = useMemo(
    () => (bulk && bulk.length > 1 ? undefined : delegateOriginOf(sessions, session.id, links)),
    [bulk, sessions, session.id, links],
  );
  // The selection's menu: every hook is read above, so the early return keeps the hooks in order.
  if (bulk && bulk.length > 1) {
    const n = bulk.length;
    const ids = bulk.map((x) => x.id);
    return (
      <>
        <div className={menuLabel}>{n} threads selected</div>
        <MenuQuickActions
          Item={Item}
          actions={[
            {
              label: allPinned ? "Unpin" : "Pin",
              description: `${allPinned ? "Unpin" : "Pin"} ${n} Threads`,
              icon: allPinned ? <PinOff /> : <Pin />,
              checked: allPinned,
              onSelect: () => {
                setPinned(ids, !allPinned);
                onClear?.();
              },
            },
            {
              label: "Unread",
              description: `Mark ${n} Unread`,
              icon: <CircleDot />,
              onSelect: () => {
                for (const id of ids) markUnread(id);
                onClear?.();
              },
            },
            ...(!bulkAllSettled
              ? [
                  {
                    label: "Settle",
                    description: `Settle ${n} Threads`,
                    icon: <CircleCheck />,
                    onSelect: () => {
                      settleThreads(ids);
                      onClear?.();
                    },
                  },
                ]
              : []),
            ...(bulkSettled
              ? [
                  {
                    label: "Unsettle",
                    description: `Unsettle ${n} Threads`,
                    icon: <CircleDashed />,
                    onSelect: () => {
                      unsettleThreads(ids);
                      onClear?.();
                    },
                  },
                ]
              : []),
          ]}
        />
      </>
    );
  }
  // Quick actions first, then rename and snooze, then the worktree's own actions; review and
  // the occasional tools sit under More, and Trash last.
  return (
    <>
      <ThreadQuickActions threadId={session.id} Item={Item} />
      {origin?.listed && origin.id && (
        <Item className={menuItem} onSelect={() => selectThread(origin.id!)}>
          <CornerUpLeft size={13} />
          <span className="min-w-0 truncate">
            Open “{origin.title}”{origin.project ? ` · ${origin.project}` : ""}
          </span>
        </Item>
      )}
      {!remote && (
        <Item className={menuItem} disabled={!canFiles} onSelect={onRename}>
          <Pencil size={13} /> Rename…
        </Item>
      )}
      {machines.length > 0 && (
        <Menu.Sub>
          <Menu.SubTrigger className={menuItem}>
            <Laptop size={13} /> Move to Machine
            <ChevronRight size={12} strokeWidth={2.5} className="ml-auto text-fg-faint" />
          </Menu.SubTrigger>
          <Menu.Portal>
            <Menu.SubContent
              sideOffset={4}
              alignOffset={-5}
              collisionPadding={surfaceCollisionPadding}
              className={cx(menuSurface, "min-w-[240px]")}
            >
              {moveBlocked && <div className="px-2 py-1 text-caption text-fg-muted">{moveBlocked}</div>}
              {[
                {
                  id: "",
                  name: local.displayName ?? selfName(),
                  glyph: local.icon ?? hereGlyph,
                  tint: local.tint,
                  reachable: true,
                },
                ...machines.map((m) => ({
                  id: m.id,
                  name: machineName(m),
                  glyph: machineGlyph(m),
                  tint: machineTint(m),
                  reachable: m.state === "connected",
                })),
              ]
                .filter((target) => target.id !== host)
                .map((target) => {
                  const dest = target.reachable
                    ? planMachineMoveDestination({
                        sessionId: rawId,
                        fromHost: host,
                        fromCwd: session.cwd,
                        toHost: target.id,
                        sessions,
                        localProjects,
                      })
                    : null;
                  return (
                    <Item
                      key={target.id || "local"}
                      className={cx(menuItem, "gap-2")}
                      disabled={!!moveBlocked || !target.reachable}
                      onSelect={() => void moveThreadToMachine(session.id, target.id)}
                    >
                      <DeviceGlyphIcon glyph={target.glyph} tint={target.tint} size={13} />
                      <span className="flex min-w-0 flex-1 flex-col">
                        <span className="truncate">{target.name}</span>
                        <span className="truncate text-caption text-fg-faint">
                          {dest
                            ? dest.returning
                              ? `Returns to ${dest.displayFolder}`
                              : dest.displayFolder
                            : "Offline"}
                        </span>
                      </span>
                    </Item>
                  );
                })}
            </Menu.SubContent>
          </Menu.Portal>
        </Menu.Sub>
      )}
      {snoozed ? (
        <Item className={menuItem} onSelect={() => snoozeThread(session.id, null)}>
          <Clock size={13} /> Unsnooze
        </Item>
      ) : (
        <Menu.Sub>
          <Menu.SubTrigger className={menuItem}>
            <Clock size={13} /> Snooze
            <ChevronRight size={12} strokeWidth={2.5} className="ml-auto text-fg-faint" />
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
      {worktree && (
        <>
          <Separator className={menuSeparator} />
          <div className="flex items-center gap-1.5 px-2 pt-0.5 pb-1 text-caption text-fg-muted">
            <GitBranch size={12} /> <span className="truncate font-mono">{worktree.branch}</span>
          </div>
          {!remote && (
            <>
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
        </>
      )}
      <Separator className={menuSeparator} />
      {/* What a thread needs now and then, not every day: kept one level down so the menu leads
          with what is used. */}
      <Menu.Sub>
        <Menu.SubTrigger className={menuItem}>
          <Ellipsis size={13} /> More
          <ChevronRight size={12} strokeWidth={2.5} className="ml-auto text-fg-faint" />
        </Menu.SubTrigger>
        <Menu.Portal>
          <Menu.SubContent
            sideOffset={4}
            alignOffset={-5}
            collisionPadding={surfaceCollisionPadding}
            className={menuSurface}
          >
            {status !== "review" && (
              <Item className={menuItem} onSelect={() => void setThreadStatus(session.id, "review")}>
                <Eye size={13} /> Mark for Review
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
            {!remote && (
              <Item className={menuItem} onSelect={() => setView({ kind: "review", threadId: session.id })}>
                <FileDiff size={13} /> Review Changes
              </Item>
            )}
            <Separator className={menuSeparator} />
            <Item className={menuItem} onSelect={() => setAutoCompactForThread(session.id, autoCompactOff)}>
              <Sparkles size={13} /> {autoCompactOff ? "Turn On Auto-compact" : "Turn Off Auto-compact"}
            </Item>
            <Item className={menuItem} onSelect={() => copyThreadId(session.id)}>
              <Copy size={13} /> Copy Thread ID
            </Item>
            {!remote && (
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
            )}
            <Item className={menuItem} onSelect={() => void repairThread(session.id)}>
              <Wrench size={13} /> Repair Thread
            </Item>
          </Menu.SubContent>
        </Menu.Portal>
      </Menu.Sub>
      {!remote && (
        <>
          <Separator className={menuSeparator} />
          <Item
            className={cx(menuItem, menuItemDanger)}
            disabled={!canFiles}
            onSelect={() =>
              askConfirm({
                title: `Move this thread to the ${trashName()}?`,
                message: `"${session.title || "New thread"}" and its history move to the ${trashName()}, where you can still get them back.`,
                action: `Move to ${trashName()}`,
                destructive: true,
                run: () => void trashThread(session.id),
              })
            }
          >
            <Trash2 size={13} /> Move to {trashName()}
          </Item>
        </>
      )}
      {remote && machineNameText && (
        <div className="px-2 pt-1 pb-0.5 text-caption text-fg-faint">
          Files, git and the shell run on {machineNameText}.
        </div>
      )}
    </>
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

/** The tone a row's status takes, by what it says: the page's own tone tokens, never a new one. */
const STATUS_TINT: Record<RowStatusKind, string> = {
  needs: "text-warning",
  working: "text-accent",
  waiting: "text-fg-muted",
  failed: "text-danger",
  completed: "text-success",
  review: "text-accent",
};

/**
 * What a row's first line says before its time, as T3 Code's sidebar does: an icon and a word in
 * the state's tone, medium weight. `Working` turns slowly (respecting reduced motion); `Approval`,
 * `Question`, `Waiting`, `Failed`, `Done` until you open it and `Review` hold still. The long form is the
 * tooltip. An idle row has none (state/sidebarRows.ts `rowStatus`).
 */
function RowStatusLabel({ status }: { status: RowStatus }) {
  const kind = status.kind;
  if (kind === "working") return null;
  const word = kind === "needs" ? (status.approval ? "Approval" : "Question") : status.label;
  return (
    <span
      title={status.title ?? status.label}
      className={cx("inline-flex shrink-0 items-center gap-1 font-medium", STATUS_TINT[kind])}
    >
      {kind === "needs" ? (
        status.approval ? (
          <ShieldQuestion size={12} className="shrink-0" />
        ) : (
          <MessageCircleQuestion size={12} className="shrink-0" />
        )
      ) : kind === "waiting" ? (
        <Clock size={12} className="shrink-0" />
      ) : kind === "failed" ? (
        <CircleAlert size={12} className="shrink-0" />
      ) : kind === "completed" ? (
        <CircleCheck size={12} className="shrink-0" />
      ) : (
        <Eye size={12} className="shrink-0" />
      )}
      {word}
    </span>
  );
}

/**
 * Hovering swaps the row's trailing state for its actions: Pin, Settle (or Unsettle) and the menu,
 * three 24px buttons in the slot the state held. The slot widens to hold them while they show
 * (`ACTIONS_ROOM`), so nothing is drawn over text.
 * The actions themselves sit beside the row's button (a button cannot hold buttons), placed on the
 * slot's line and fading in as the state fades out.
 */
/**
 * The slot's width while the actions show: their 76px, and only then. At rest the slot is as wide
 * as its icon or age, so the title takes the rest; on hover the title gives the actions room.
 */
const ACTIONS_ROOM = cx(
  "group-hover:min-w-[76px]",
  "group-has-[:focus-visible]:min-w-[76px]",
  "group-has-[[data-state=open]]:min-w-[76px]",
);
/** The state's slot while the actions show: its own state hidden under them. */
const SWAPPED_ON_HOVER = cx(
  "group-hover:*:invisible",
  "group-has-[:focus-visible]:*:invisible",
  "group-has-[[data-state=open]]:*:invisible",
);

const NO_AGENTS: SubagentInfo[] = [];

/**
 * A thread row's grid: two lines at one height whatever the row holds, so a column of rows reads
 * as a table. Each line is a left part that truncates and a right part against the row's edge that
 * takes what it needs (a running turn's "Working 12:07", the model), so the right side is never
 * cut and one line's right part never narrows another line's left one.
 */
export const ROW_HEIGHT = 54;
/** Two lines: the title with its state and time; where it runs, its marks and its model. */
const ROW_GRID = "grid h-[54px] grid-rows-[22px_18px] content-center px-3";
/** Line 2 starts where the title does, past the project mark (14px and its 6px gap). */
const ROW_INDENT = "pl-5";
/** `MouseEvent.button` of the middle button (a pressed scroll wheel): it settles the row. */
const MIDDLE_BUTTON = 1;
/** One line of the row: its left part and its right part. */
const ROW_LINE = "flex min-w-0 items-center gap-2";
/** A line's left part: truncated. */
const ROW_LEFT = "relative flex min-w-0 flex-1 items-center gap-1.5 overflow-hidden whitespace-nowrap";
/** A line's right part: against the row's right edge. */
const ROW_RIGHT =
  "relative flex flex-none items-center justify-end gap-1 overflow-hidden whitespace-nowrap tabular";

/**
 * The compact row: one line at 32px, so about two rows sit where one detailed row
 * did. It keeps the project's mark with the status dot on it, the title and the trailing state;
 * the computer, branch, agent and model move to the hover card and Thread info.
 */
// A grid, not a flex row: the sections fold inside an `auto` grid column (`Fold`), which grows to
// its rows' min-content. A flex row's min-content is its whole title, so every row widened to the
// longest one and pushed the age out of view; the title's `minmax(0,1fr)` track asks for nothing.
const COMPACT_ROW = "grid h-[34px] grid-cols-[20px_minmax(0,1fr)_auto] items-center gap-x-2 px-2";

/** The compact row's leading dot, in the tone the detailed row's chip takes for the same state. */
const COMPACT_DOT: Record<Exclude<CompactMark, "working">, string> = {
  needs: "bg-warning motion-safe:animate-pulse",
  failed: "bg-danger",
  unread: "bg-success",
};

function RowIcon({ mark, project, className }: { mark?: CompactMark; project: string; className?: string }) {
  return (
    <span aria-hidden className="relative flex size-5 items-center justify-center">
      <ProjectMark name={project} size={16} radius="rounded-inline" className={className} />
      {mark && mark !== "working" && (
        <span
          className={cx(
            "-right-0.5 -top-0.5 absolute size-2 rounded-full ring-2 ring-[var(--rc-sidebar-bg)]",
            COMPACT_DOT[mark],
          )}
        />
      )}
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
function RowFill({ selected, picked, lifted }: { selected: boolean; picked?: boolean; lifted?: boolean }) {
  return (
    <span
      aria-hidden
      className={cx(
        "pointer-events-none absolute inset-0",
        SIDEBAR_ROW_RADIUS,
        lifted && SIDEBAR_ROW_LIFTED,
        picked ? "bg-accent/12" : selected ? SIDEBAR_ROW_CURRENT : "group-active:bg-fill-strong",
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

function RowMachine({ hostId, unsynced }: { hostId?: string; unsynced?: boolean }) {
  const host = useMachines((s) => s.machines.find((m) => m.id === hostId));
  const local = useLocalIdentity();
  const platform = appPlatform();
  if (hostId && !host) {
    return (
      <span className="flex min-w-0 items-center gap-1">
        <Skeleton className="size-3 shrink-0 rounded-xs" />
        <Skeleton className="h-2.5 w-16 rounded-xs" />
        <span className="sr-only">Loading machine</span>
      </span>
    );
  }
  const name = host ? machineName(host) : (local.displayName ?? selfName());
  const glyph = host
    ? machineGlyph(host)
    : (local.icon ?? platformIcon(platform === "mac" ? "macos" : platform));
  const title = unsynced ? `Runs on ${name}. Your change syncs when ${name} is back.` : `Runs on ${name}`;
  return (
    <span className="flex min-w-0 items-center gap-1" title={title}>
      <DeviceGlyphIcon glyph={glyph} tint={host ? machineTint(host) : local.tint} size={13} />
      <span className="min-w-0 truncate">{name}</span>
      {unsynced && <CloudUpload size={12} className="shrink-0" aria-label="Waiting to sync" />}
    </span>
  );
}

// Memoized, and reads only the flags it shows: a streamed chunk must not re-render 500 rows.
// Compact: a title and state.
// Comfortable: the project mark and title, then branch or subfolder and model. Hovering swaps the
// slot for the row's actions in both, so nothing shifts.
export const ThreadRow = memo(function ThreadRow({
  session,
  selected,
  context,
  now,
  from,
  pull,
  agents,
  navActive,
  picked,
  bulk,
  dragging,
  pinKey,
  settleKey,
  onClearSelection,
  compact,
}: {
  session: SessionSummary;
  selected: boolean;
  /** The open thread is one this row started: the row is kept lit as its context, not opened. */
  context?: boolean;
  /** Being dragged to another place in its section: no hover card, and no pointer of its own. */
  dragging?: boolean;
  now: number;
  /**
   * No parent row above it (a search result, or a parent in another project the filter left out):
   * the thread it came from, which its third line names and opens.
   */
  from?: DelegateOrigin;
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
  /** The sidebar's compact density: one line, the rest on the hover card. */
  compact?: boolean;
}) {
  const host = useMachines((s) => s.machines.find((m) => m.id === session.host));
  const machine = host ? machineName(host) : undefined;

  const request = useApp((s) => s.threads[session.id]?.permissions[0]);
  const running = useApp((s) => isRunning(s.threads[session.id]));
  const startedAt = useApp((s) => s.threads[session.id]?.turn?.startedAt);
  // No turn runs, but the threads it started or messaged still work (state/waiting.ts).
  const wait = useWaiting(session.id);
  const wake = useApp((s) => s.snoozed[session.id]);
  const status = useApp((s) => statusOf(s, session.id));
  const worktree = useApp((s) => worktreeOf(s, session.id));
  const checkout = useCheckoutBranch(session, host?.state, !worktree);
  const gitBranch = worktree?.branch || checkout?.branch;
  const harnesses = useApp((s) => harnessesOn(s, session.host) ?? s.harnesses);
  const pinned = useApp((s) => s.pinned.includes(session.id));
  // A settle or pin made while its machine was away, waiting to be sent (state/machineOutbox.ts).
  const unsynced = useOutbox((o) => hasPendingChange(o.byHost, session.id));
  const finishedAt = useApp((s) => s.finished[session.id]);
  const failedAt = useApp((s) => s.failed[session.id]);
  const items = useApp((s) => s.threads[session.id]?.items);
  // Its own subagents count with the threads it started: one stack, whichever way they were spawned.
  const ownAgents = useApp((s) => s.agents[session.id] ?? NO_AGENTS);
  const stack = useMemo(() => {
    const own = liveSubagents(items, ownAgents, running, session.harness);
    return own.length ? [...own, ...(agents ?? [])] : agents;
  }, [items, ownAgents, running, session.harness, agents]);
  const delegates = useMemo(
    () => delegateCount(stack, (id) => harnesses?.harnesses.find((h) => h.id === id)?.name ?? id),
    [stack, harnesses],
  );
  // Done or Failed while you were elsewhere, until you open it.
  const fresh = !!finishedAt && !selected;
  const failed = !!failedAt && !selected;
  // What the row's one status chip says, or nothing when it shows its age instead
  // (state/sidebarRows.ts `rowStatus`).
  const chip = rowStatus({
    request,
    running: running || status === "running",
    stalled: status === "stalled",
    // A settled thread is done by the user's word: what it handed out may still run, but the row
    // does not say it waits (it would read as work the user has to come back to).
    waiting: status !== "done" && wait ? { label: waitingLabel(wait), title: waitingTitle(wait) } : undefined,
    failed,
    fresh,
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
  const model = catalogModelTitle(session.model, harness);
  const startRename = () => {
    setTitle(session.title ?? "");
    // After the menu closes and gives focus back.
    setTimeout(() => setRenaming(true), 0);
  };
  const commit = () => {
    setRenaming(false);
    if (title.trim() && title.trim() !== session.title) void renameThread(session.id, title, { user: true });
  };
  // Renaming edits the title in place, over the row's first line: the row keeps its height and
  // its other lines, so the list does not jump while you type.
  const renameBox = renaming && (
    <div className={cx("absolute inset-x-1 z-10", compact ? "top-1" : "top-[5px]")}>
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
        {...noTextHelpers}
        className="h-6 w-full rounded-md bg-surface-2 px-1.5 text-body shadow-1"
      />
    </div>
  );
  const done = status === "done";
  // A fork or a side chat says so with a small glyph, whether it is nested or a row of its own.
  const branch = session.parent ? branchKindOf(session.parent) : undefined;
  // The open thread is a delegated one with no row of its own: its parent stands in as the context.
  const lit = selected || !!context;
  // What wants you reads strong; what works, waits or has been read recedes until hovered.
  const wantsYou = !!request || fresh || failed || lit || picked;
  const markTint = cx(
    "transition-opacity duration-150 ease-snappy",
    wantsYou ? "opacity-100" : "opacity-60 group-hover:opacity-100",
  );
  // One weight for every title, whatever the thread is doing: the status word says that.
  const titleTint = compact
    ? cx("min-w-0 flex-1 truncate text-body", lit ? "text-fg" : "text-fg-muted")
    : "min-w-0 flex-1 truncate text-body font-medium text-fg";
  const fromText = from ? originText(from) : undefined;
  // Line 2's end: the harness mark with its model, never cut.
  const modelMark = (model || harnessName) && (
    <span className="ml-auto flex shrink-0 items-center gap-1" title={harnessName ?? model}>
      <HarnessIcon id={session.harness || "rustcode"} name={harness?.name} size={12} className="opacity-70" />
      <span className="normal-nums">{model || harnessName}</span>
    </span>
  );
  // Line 2's marks before the model: what it started and its pull request.
  const details = (
    <span className="ml-auto flex shrink-0 items-center gap-1.5">
      {delegates && (
        <Tooltip label={delegates.title}>
          <span
            role="img"
            aria-label={delegates.title}
            className="flex shrink-0 items-center gap-0.5 text-fg-muted tabular"
          >
            <Bot size={12} />
            {delegates.count}
          </span>
        </Tooltip>
      )}
      {pull && <PullMark pull={pull} />}
    </span>
  );
  // The end of the first line always has a time: a running turn's clock, else the hour a snoozed
  // one wakes, else its age, with a settled thread's check before it.
  const time = (
    <span className="flex min-w-0 items-center gap-1">
      {chip?.kind === "working" && <Spinner size={10} />}
      {chip?.kind === "working" && <span className="sr-only">Working</span>}
      {done && !chip && <Check size={12} className="shrink-0" />}
      <span className="min-w-0 truncate tabular">
        {chip?.kind === "working" && startedAt !== undefined ? (
          <Elapsed since={startedAt} />
        ) : wake ? (
          wakeLabel(wake, now)
        ) : (
          shortAge(session.updatedAt, now)
        )}
      </span>
    </span>
  );
  // The roomy row says what the thread is doing in words before its time.
  const state = (
    <span className="flex shrink-0 items-center gap-1.5">
      {chip && <RowStatusLabel status={chip} />}
      {time}
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
        <div
          data-sidebar-hover={!dragging && !picked ? "" : undefined}
          className={cx("group relative", dragging && "cursor-grabbing")}
        >
          {/* A moment's hover says what the row has no room for: its project, harness and model,
              what it is doing, its branch and folder, and the last thing said in it. */}
          <ThreadHoverCard
            session={session}
            project={project}
            machine={machine}
            harness={harnessName}
            model={model}
            branch={gitBranch}
            status={chip?.label ?? (done ? "Settled" : "Idle")}
            now={now}
            items={items}
            disabled={dragging || renaming}
          >
            <button
              type="button"
              data-row-id={session.id}
              tabIndex={navActive ? 0 : -1}
              onClick={() => selectThread(session.id)}
              onMouseDown={(e) => {
                if (e.button === MIDDLE_BUTTON) e.preventDefault();
              }}
              onAuxClick={(e) => {
                if (e.button !== MIDDLE_BUTTON) return;
                e.preventDefault();
                if (!done) settleThread(session.id);
              }}
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
              data-sidebar-current={lit && !dragging && !picked ? "" : undefined}
              className={cx(compact ? COMPACT_ROW : ROW_GRID, "relative w-full text-left")}
            >
              {(!lit || picked || dragging) && (
                <RowFill selected={!!dragging} picked={picked} lifted={dragging} />
              )}
              {compact ? (
                <>
                  <RowIcon mark={compactMark(chip)} project={project} className={markTint} />
                  <span className={ROW_LEFT}>
                    {pinned && <Pin size={12} className="shrink-0 rotate-45 text-fg-faint" />}
                    {branch && (
                      <span className="shrink-0 text-fg-faint">
                        {branch === "fork" ? <GitFork size={12} /> : <MessageSquareQuote size={12} />}
                      </span>
                    )}
                    <RowBadges threadId={session.id} />
                    {fromText && (
                      <Tooltip label={`Started ${fromText}`}>
                        <CornerUpLeft
                          size={12}
                          className="shrink-0 text-fg-faint"
                          aria-label={`Started ${fromText}`}
                        />
                      </Tooltip>
                    )}
                    <span className={cx(titleTint, renaming && "invisible")}>
                      {session.title || "New thread"}
                    </span>
                  </span>
                  <span
                    title={chip?.title}
                    className={cx(ROW_RIGHT, "text-sub text-fg-faint", ACTIONS_ROOM, SWAPPED_ON_HOVER)}
                  >
                    {delegates && (
                      <span className="flex shrink-0 items-center gap-0.5 tabular">
                        <Bot size={12} aria-hidden />
                        <span className="sr-only">{delegates.title}</span>
                        <span aria-hidden>{delegates.count}</span>
                      </span>
                    )}
                    <span className="flex min-w-0 items-center justify-end">{time}</span>
                  </span>
                </>
              ) : (
                <>
                  {/* Line 1: the title, then its state and time (hover actions take their place). */}
                  <span className={ROW_LINE}>
                    <span className={ROW_LEFT}>
                      <RowIcon mark={compactMark(chip)} project={project} className={markTint} />
                      {pinned && <Pin size={12} className="shrink-0 rotate-45 text-fg-faint" />}
                      {branch && (
                        <span
                          title={branch === "fork" ? "A fork" : "A side chat"}
                          className="shrink-0 text-fg-faint"
                        >
                          {branch === "fork" ? <GitFork size={12} /> : <MessageSquareQuote size={12} />}
                        </span>
                      )}
                      <RowBadges threadId={session.id} />
                      <span className={cx(titleTint, renaming && "invisible")}>
                        {session.title || "New thread"}
                      </span>
                    </span>
                    {/* What it is doing and since when; the hover actions take its place. */}
                    <span
                      title={chip?.title}
                      className={cx(ROW_RIGHT, "text-sub text-fg-muted", ACTIONS_ROOM, SWAPPED_ON_HOVER)}
                    >
                      <span className="flex min-w-0 items-center justify-end">{state}</span>
                    </span>
                  </span>
                  {/* Line 2: the computer it runs on, from the first frame (a branch read later would
                      swap it out), then its marks and its model against the edge. Its branch and
                      folder are on the hover card. */}
                  <span className={cx(ROW_LINE, ROW_INDENT, "gap-1.5 text-sub text-fg-muted")}>
                    <RowMachine hostId={session.host} unsynced={unsynced} />
                    {details}
                    {modelMark && <span className="flex shrink-0">{modelMark}</span>}
                  </span>
                </>
              )}
            </button>
          </ThreadHoverCard>
          {renameBox}
          <div
            data-row-actions
            // Hidden while the row is dragged: the actions sit beside the row, not on it.
            className={cx(
              "invisible absolute right-2 flex items-center opacity-0 transition-[opacity,visibility] duration-150 ease-snappy",
              compact ? "top-1" : "top-1.5",
              !dragging &&
                !renaming &&
                "group-hover:visible group-hover:opacity-100 group-has-[:focus-visible]:visible group-has-[:focus-visible]:opacity-100 group-has-[[data-state=open]]:visible group-has-[[data-state=open]]:opacity-100",
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
              <Tooltip label={`Actions · ${formatBinding("shift+f10")}`}>
                <DropdownMenu.Trigger asChild>
                  <button
                    type="button"
                    aria-label={`Actions for ${session.title || "thread"}`}
                    className="flex size-6 items-center justify-center rounded-sm text-fg-muted hover:bg-fill hover:text-fg"
                  >
                    <Ellipsis size={14} />
                  </button>
                </DropdownMenu.Trigger>
              </Tooltip>
              <MenuContent align="start" sideOffset={4} className="w-[280px]">
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
      <ContextMenuSurface className="w-[280px]">
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
export const PendingRow = memo(function PendingRow({
  pending,
  selected,
  navActive,
  compact,
}: {
  pending: PendingSetup;
  selected: boolean;
  navActive?: boolean;
  /** The sidebar's compact density: the first line alone. */
  compact?: boolean;
}) {
  const project = projectName(pending.worktree?.repo || pending.cwd);
  const failure = pending.failure;
  return (
    <div className="group relative" data-sidebar-hover>
      <Tooltip label={failure ? `${pending.title} · ${failure.text}` : pending.title}>
        <button
          type="button"
          data-row-id={pending.id}
          tabIndex={navActive ? 0 : -1}
          onClick={() => selectThread(pending.id)}
          aria-current={selected ? "page" : undefined}
          data-sidebar-current={selected ? "" : undefined}
          className={cx(compact ? COMPACT_ROW : ROW_GRID, "relative w-full text-left")}
        >
          {!selected && <RowFill selected={false} />}
          {/* The thread row's grid, so a thread being set up is the height and shape it will have. */}
          <span className={cx(ROW_LINE, compact && "col-span-full")}>
            <span className={ROW_LEFT}>
              <ProjectMark name={project} size={14} className="opacity-60 group-hover:opacity-100" />
              <span className="min-w-0 flex-1 truncate text-body font-medium text-fg">{pending.title}</span>
            </span>
            {/* Dismiss sits over the slot's end when the setup failed, so the words keep its room. */}
            <span className={cx(ROW_RIGHT, "text-sub text-fg-muted", failure && "pr-6")}>
              {failure ? (
                <>
                  <CircleAlert size={12} className="text-danger" />
                  <span className="font-medium text-danger">Failed</span>
                </>
              ) : (
                <>
                  <Spinner size={10} />
                  <Elapsed since={pending.startedAt} />
                </>
              )}
            </span>
          </span>
          {!compact && (
            <span className={cx(ROW_LINE, ROW_INDENT, "gap-1.5 text-sub text-fg-muted")}>
              {failure ? (
                <span className="min-w-0 truncate">{failure.text}</span>
              ) : (
                <RowMachine hostId={pending.host} />
              )}
            </span>
          )}
        </button>
      </Tooltip>
      <div className={cx("absolute right-2 flex items-center", compact ? "top-1" : "top-1.5")}>
        {failure && (
          <RowAction label="Dismiss" onClick={() => dismissPendingSetup(pending.id)}>
            <X size={12} />
          </RowAction>
        )}
      </div>
    </div>
  );
});
