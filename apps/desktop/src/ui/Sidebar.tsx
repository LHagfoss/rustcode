// The sidebar, after T3 Code's: a toolbar clear of the traffic lights, a
// search field with a filter menu, then one list: pinned threads first, then
// every active thread (by project when asked), each saying in its right slot
// what it does (Working, Approval, Input, Failed, Done until you open it), and
// Settled (threads marked Done) as a shelf at the foot. ⌘1–⌘9 open the
// first nine rows; holding ⌘ shows which is which.

import {
  Check,
  ChevronDown,
  ChevronRight,
  CircleAlert,
  CircleCheck,
  CircleDashed,
  Copy,
  Ellipsis,
  Eye,
  FileDiff,
  Folder,
  FolderOpen,
  FolderX,
  GitBranch,
  Hand,
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
import { ContextMenu, DropdownMenu } from "radix-ui";
import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { modelName, projectName, type SessionSummary } from "../acp/types";
import type { ThreadStatus } from "../acp/v2";
import { openTerminalAt, revealPath } from "../lib/native";
import { hasShell } from "../lib/shell";
import {
  type ChildInfo,
  type ChildState,
  childrenListed,
  childState,
  childSummary,
  childVisible,
  orderChildren,
  unseenChild,
} from "../state/children";
import { isActive } from "../state/orchestration";
import {
  askConfirm,
  featureMissing,
  markUnread,
  removeWorktree,
  renameThread,
  repairThread,
  searchThreads,
  selectThread,
  setThreadStatus,
  setView,
  startNewThread,
  statusOf,
  toast,
  togglePin,
  toggleSidebar,
  trashThread,
  useApp,
  worktreeOf,
} from "../state/store";
import { formatClock, idleSettled, shortAge } from "../state/thread";
import { HarnessIcon } from "./HarnessIcon";
import { LinkBadge } from "./Links";
import { SettingsButton } from "./MainHeader";
import { Cluster, ClusterButton, cx, Skeleton, Spinner, Tooltip } from "./primitives";
import { StatusChip } from "./Status";
import { TitlebarRow } from "./Titlebar";
import { UpdateRow } from "./Updates";

const MIN = 208;
const MAX = 460;

function useSidebarWidth() {
  const [width, setWidth] = useState(() => {
    try {
      return Number(localStorage.getItem("rustcode.sidebarWidth")) || 256;
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

const menuItem =
  "flex w-full items-center gap-2 rounded-lg px-3 py-[7px] text-body outline-none data-[highlighted]:bg-fill";
const menuSurface = "elevated z-50 min-w-[200px] rounded-xl p-1.5 rc-in";

function ThreadMenuItems({
  session,
  Item,
  Separator,
  onRename,
}: {
  session: SessionSummary;
  Item: typeof DropdownMenu.Item | typeof ContextMenu.Item;
  Separator: typeof DropdownMenu.Separator | typeof ContextMenu.Separator;
  onRename: () => void;
}) {
  const pinned = useApp((s) => s.pinned.includes(session.id));
  const status = useApp((s) => statusOf(s, session.id));
  const worktree = useApp((s) => worktreeOf(s, session.id));
  const statusMissing = useApp((s) => (s.features.status ? null : featureMissing("status")));
  const canFiles = hasShell();
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
      <Item className={menuItem} onSelect={() => setView({ kind: "review", threadId: session.id })}>
        <FileDiff size={13} /> Review Changes
      </Item>
      <Item className={menuItem} onSelect={() => markUnread(session.id)}>
        <CircleCheck size={13} /> Mark Unread
      </Item>
      <Separator className="mx-1.5 my-1 h-px bg-border" />
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
      {session.cwd && (
        <Item className={menuItem} onSelect={() => copyText(worktree?.path ?? session.cwd!, "Path copied")}>
          <Copy size={13} /> Copy Path
        </Item>
      )}
      <Item className={menuItem} onSelect={() => copyText(session.id, "Thread ID copied")}>
        <Copy size={13} /> Copy Thread ID
      </Item>
      {worktree && (
        <>
          <Separator className="mx-1.5 my-1 h-px bg-border" />
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
          <Item
            className={cx(menuItem, "text-danger data-[highlighted]:bg-danger/12")}
            onSelect={() => void removeWorktree(session.id)}
          >
            <FolderX size={13} /> Remove Worktree…
          </Item>
        </>
      )}
      <Separator className="mx-1.5 my-1 h-px bg-border" />
      <Item
        className={cx(menuItem, "text-danger data-[highlighted]:bg-danger/12")}
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
// After T3 Code's rows: the title and one meta line, and a single slot on the right that shows the
// age, a live clock while the turn runs, or what the thread waits for; hovering swaps it for the
// row's actions, so nothing shifts.
const ThreadRow = memo(function ThreadRow({
  session,
  selected,
  scopeAll,
  now,
  snippet,
  jump,
  compact,
  hasChildren,
}: {
  session: SessionSummary;
  selected: boolean;
  scopeAll: boolean;
  now: number;
  /** While searching: the words around the match in what was said. */
  snippet?: string;
  /** While ⌘ is held: the digit that opens this row. */
  jump?: number;
  /** A child of the thread above it: one line, its harness and what it is doing, no meta line. */
  compact?: boolean;
  /** The sidebar lists this thread's children under it, which already say what the links are. */
  hasChildren?: boolean;
}) {
  const request = useApp((s) => s.threads[session.id]?.permissions[0]);
  const running = useApp((s) => s.threads[session.id]?.running ?? false);
  const startedAt = useApp((s) => s.threads[session.id]?.turnStartedAt);
  const waiting = useApp((s) => s.threads[session.id]?.waiting ?? 0);
  const status = useApp((s) => statusOf(s, session.id));
  const worktree = useApp((s) => worktreeOf(s, session.id));
  const harnesses = useApp((s) => s.harnesses);
  const pinned = useApp((s) => s.pinned.includes(session.id));
  const finishedAt = useApp((s) => s.finished[session.id]);
  const failedAt = useApp((s) => s.failed[session.id]);
  // Done or Failed while you were elsewhere, until you open it (T3 Code's unseen completion).
  const fresh = !!finishedAt && !selected;
  const failed = !!failedAt && !selected;
  const [renaming, setRenaming] = useState(false);
  const [title, setTitle] = useState("");
  const external = !!session.harness && session.harness !== "rustcode";
  const harness = external ? harnesses?.harnesses.find((h) => h.id === session.harness) : undefined;
  const meta = [
    worktree ? undefined : scopeAll && session.cwd ? projectName(session.cwd) : undefined,
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
  if (compact) {
    return (
      <ContextMenu.Root>
        <ContextMenu.Trigger asChild>
          <div className="group relative">
            <button
              type="button"
              onClick={() => selectThread(session.id)}
              onDoubleClick={startRename}
              onKeyDown={(e) => {
                if (e.key === "F2") {
                  e.preventDefault();
                  startRename();
                }
              }}
              aria-current={selected ? "page" : undefined}
              className={cx(
                "flex h-7 w-full items-center gap-1.5 rounded-md px-2.5 text-left",
                selected ? "bg-fill" : "hover:bg-fill-subtle active:bg-fill",
              )}
            >
              <Tooltip label={meta.length ? meta.join(" · ") : (session.title ?? "Thread")}>
                <span className="flex shrink-0 items-center">
                  <HarnessIcon
                    id={session.harness ?? "rustcode"}
                    name={harness?.name}
                    size={12}
                    className={external ? "text-fg-muted" : "text-fg-faint"}
                  />
                </span>
              </Tooltip>
              <span
                className={cx(
                  "min-w-0 flex-1 truncate text-callout",
                  selected || request || failed
                    ? "font-medium text-fg"
                    : running || status === "done"
                      ? "text-fg/75"
                      : "text-fg-muted",
                )}
              >
                {session.title || "New thread"}
              </span>
              {running ? (
                <span className="flex shrink-0 items-center gap-1 text-accent text-sub tabular">
                  <Spinner size={10} />
                  {startedAt ? <Elapsed since={startedAt} /> : null}
                </span>
              ) : request || status === "needs_input" ? (
                <Hand size={12} className="shrink-0 text-warning" aria-label="Needs you" />
              ) : failed ? (
                <X size={12} className="shrink-0 text-danger" aria-label="Failed" />
              ) : fresh || status === "done" ? (
                <CircleCheck size={12} className="shrink-0 text-success" aria-label="Done" />
              ) : null}
            </button>
          </div>
        </ContextMenu.Trigger>
        <ContextMenu.Portal>
          <ContextMenu.Content className={menuSurface}>
            <ThreadMenuItems
              session={session}
              Item={ContextMenu.Item}
              Separator={ContextMenu.Separator}
              onRename={startRename}
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
          <button
            type="button"
            onClick={() => selectThread(session.id)}
            onDoubleClick={startRename}
            onKeyDown={(e) => {
              if (e.key === "F2") {
                e.preventDefault();
                startRename();
              }
            }}
            aria-current={selected ? "page" : undefined}
            className={cx(
              "flex w-full flex-col gap-px rounded-md px-2.5 py-[5px] text-left",
              selected ? "bg-fill" : "hover:bg-fill-subtle active:bg-fill",
            )}
          >
            <span className="flex w-full items-center gap-1.5 group-hover:pr-11 group-has-[:focus-visible]:pr-11 group-has-[[data-state=open]]:pr-11">
              {pinned && <Pin size={10} className="shrink-0 rotate-45 text-fg-muted" />}
              <span
                className={cx(
                  "min-w-0 flex-1 truncate text-body",
                  // What wants you reads strong; what works, waits or is seen recedes.
                  selected || request || fresh || failed
                    ? "font-medium text-fg"
                    : done || running || waiting
                      ? "text-fg/75"
                      : "text-fg",
                )}
              >
                {session.title || "New thread"}
              </span>
              <LinkBadge threadId={session.id} hidden={hasChildren} />
              <span className="flex shrink-0 items-center gap-1 text-sub tabular group-hover:invisible group-has-[:focus-visible]:invisible group-has-[[data-state=open]]:invisible">
                {jump !== undefined ? (
                  <kbd className="rounded-sm bg-fill px-1 font-sans text-fg-muted">⌘{jump}</kbd>
                ) : request ? (
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
                ) : waiting ? (
                  <span
                    className="flex items-center gap-1 text-fg-muted"
                    title={`Waiting on ${waiting} background command${waiting === 1 ? "" : "s"}`}
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
                    <span className="text-fg-faint">{shortAge(session.updatedAt, now)}</span>
                  </>
                )}
              </span>
            </span>
            {snippet ? (
              <span className="truncate text-fg-muted text-sub">“{snippet}”</span>
            ) : request ? (
              <span className="truncate text-fg-muted text-sub">
                {request.question ? request.question.text : request.title}
              </span>
            ) : (
              (meta.length > 0 || worktree || external) && (
                <span className="flex items-center gap-1 truncate text-fg-faint text-sub">
                  {worktree && (
                    <span className="flex min-w-0 shrink items-center gap-0.5 rounded-sm bg-fill-subtle px-1 font-mono text-[10px]">
                      <GitBranch size={9} className="shrink-0" />
                      <span className="truncate">{worktree.branch}</span>
                    </span>
                  )}
                  {external && (
                    <HarnessIcon
                      id={session.harness!}
                      name={harness?.name}
                      size={10}
                      className="text-fg-muted"
                    />
                  )}
                  <span className="truncate">{meta.join(" · ")}</span>
                </span>
              )
            )}
          </button>
          <div className="absolute top-[3px] right-1.5 hidden items-center group-hover:flex group-has-[:focus-visible]:flex group-has-[[data-state=open]]:flex">
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
                    Item={DropdownMenu.Item}
                    Separator={DropdownMenu.Separator}
                    onRename={startRename}
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
            Item={ContextMenu.Item}
            Separator={ContextMenu.Separator}
            onRename={startRename}
          />
        </ContextMenu.Content>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  );
});

/**
 * Threads this one started (start_session, fan_out), under it. A parent you are not reading
 * shows one line however many children it has; opening the group (or the parent) lists what
 * works, what wants you and what you opened, and the rest stay behind "N finished" as soon
 * as they finish, so delegated work leaves the list the moment it is done.
 */
function ChildThreads({
  items,
  parentId,
  selectedId,
  now,
}: {
  items: SessionSummary[];
  parentId: string;
  selectedId: string | null;
  now: number;
}) {
  const [openGroups, setOpenGroups] = useStored<Record<string, boolean>>("rustcode.sidebar.childrenOpen", {});
  const [openFinished, setOpenFinished] = useState(false);
  // One string, so the rows re-render only when a child's state changes. A child whose link is
  // still open is live even before its turn reports so (it was just started or messaged).
  const states = useApp((s) => {
    const linked = new Map(
      s.links.links.filter((l) => l.orchestrator === parentId).map((l) => [l.target, l.state] as const),
    );
    return items
      .map((c) => {
        const status = statusOf(s, c.id);
        const link = linked.get(c.id);
        const live = link && isActive(link) ? (link === "needs_user" ? "needs_input" : "running") : status;
        const state = childState(live, !!s.failed[c.id]);
        return `${c.id}:${state}:${s.finished[c.id] ? 1 : 0}`;
      })
      .join(" ");
  });
  const children = useMemo(() => {
    const by = new Map(
      states
        .split(" ")
        .filter(Boolean)
        .map((part) => {
          const [id, state, unseen] = part.split(":");
          return [id!, { id: id!, state: state as ChildState, unseen: unseen === "1" }] as const;
        }),
    );
    return items.flatMap((c) => {
      const info = by.get(c.id);
      return info ? [info] : [];
    });
  }, [items, states]);
  const expanded = !!openGroups[parentId];
  const listed = childrenListed(children, { expanded, parentSelected: parentId === selectedId, selectedId });
  const ordered = orderChildren(children, selectedId);
  const shown = ordered.filter((c) => childVisible(c, selectedId));
  const folded = ordered.filter((c) => !childVisible(c, selectedId));
  const dot = unseenChild(children);
  const toggle = () => setOpenGroups({ ...openGroups, [parentId]: !expanded });
  const row = (child: ChildInfo) => {
    const session = items.find((c) => c.id === child.id);
    return session ? (
      <div key={child.id} className="ml-[14px] border-border border-l pl-1 rc-in">
        <ThreadRow session={session} selected={child.id === selectedId} scopeAll={false} now={now} compact />
      </div>
    ) : null;
  };
  return (
    <>
      <div className="ml-[14px] border-border border-l pl-1">
        <button
          type="button"
          onClick={toggle}
          aria-expanded={expanded}
          title={childSummary(children)}
          className="flex h-7 w-full items-center gap-1 rounded-md py-0 pr-1.5 pl-2.5 text-left text-fg-muted hover:bg-fill-subtle hover:text-fg"
        >
          <ChevronRight size={9} className={cx("shrink-0 transition-transform", expanded && "rotate-90")} />
          <span className="min-w-0 flex-1 truncate text-caption">{childSummary(children)}</span>
          {dot && (
            <span
              role="img"
              aria-label={dot === "failed" ? "A child thread failed" : "A child thread finished"}
              className={cx("size-1 shrink-0 rounded-full", dot === "failed" ? "bg-danger" : "bg-success")}
            />
          )}
        </button>
      </div>
      {listed && shown.map(row)}
      {listed && folded.length > 0 && (
        <button
          type="button"
          onClick={() => setOpenFinished(!openFinished)}
          aria-expanded={openFinished}
          className="ml-[14px] flex h-[20px] items-center gap-1 rounded-sm px-2 text-caption text-fg-faint hover:bg-fill-subtle hover:text-fg-muted"
        >
          <ChevronRight size={10} className={cx("transition-transform", openFinished && "rotate-90")} />
          {openFinished ? "Hide finished" : `${folded.length} finished`}
        </button>
      )}
      {listed && openFinished && folded.map(row)}
    </>
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

type SectionId = "pinned" | "threads" | "settled";

interface Section {
  id: SectionId;
  label: string;
  items: SessionSummary[];
  /** What a folded section hides: a thread that needs you, else one that works, else one just finished. */
  hidden?: HiddenKind;
  /** Threads, split by project when there are several. */
  projects?: { key: string; label: string; items: SessionSummary[] }[];
}

/** Sections that start folded: Settled, so done threads stay out of the way. */
const FOLDED_BY_DEFAULT: SectionId[] = ["settled"];

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
        "group/h flex h-7 w-full items-center gap-1.5 rounded-md px-2.5 text-left font-semibold text-callout hover:bg-fill-subtle",
        "text-fg-muted",
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
            <ChevronRight size={11} strokeWidth={2.5} className="shrink-0 text-fg-faint" />
          </span>
        )}
        {open && (
          <ChevronRight
            size={11}
            strokeWidth={2.5}
            className="shrink-0 rotate-90 text-fg-faint opacity-0 [grid-area:1/1] group-hover/h:opacity-100 group-focus-visible/h:opacity-100"
          />
        )}
      </span>
    </button>
  );
}

/**
 * A project's folder, as T3 Code draws it: the folder and its name, a dot for
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
  onlyProject,
  setOnlyProject,
}: {
  byProject: boolean;
  setByProject: (v: boolean) => void;
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
      <Tooltip label="Filter and view">
        <DropdownMenu.Trigger asChild>
          <button
            type="button"
            aria-label="Filter and view"
            className={cx(
              "flex size-[30px] shrink-0 items-center justify-center rounded-full hover:bg-fill data-[state=open]:bg-fill",
              on ? "text-accent" : "text-fg-muted",
            )}
          >
            <ListFilter size={14} />
          </button>
        </DropdownMenu.Trigger>
      </Tooltip>
      <DropdownMenu.Portal>
        <DropdownMenu.Content align="end" sideOffset={6} className={menuSurface}>
          <DropdownMenu.Item className={menuItem} onSelect={() => setByProject(!byProject)}>
            {check(byProject)} Group by project
          </DropdownMenu.Item>
          <DropdownMenu.Item
            className={menuItem}
            disabled={!projectCwd}
            onSelect={() => setOnlyProject(!onlyProject)}
          >
            {check(onlyProject)}
            <span className="truncate">Only {projectCwd ? projectName(projectCwd) : "this project"}</span>
          </DropdownMenu.Item>
          <DropdownMenu.Separator className="mx-1.5 my-1 h-px bg-border" />
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
  const pinned = useApp((s) => s.pinned);
  const [query, setQuery] = useState("");
  // By state, as T3 Code's sidebar, with the project on each row; grouping by project is an option.
  const [byProject, setByProject] = useStored("rustcode.sidebar.groupByProject", false);
  const [onlyProject, setOnlyProject] = useStored("rustcode.sidebar.onlyProject", false);
  const [folded, setFolded] = useStored<string[]>("rustcode.sidebar.folded", FOLDED_BY_DEFAULT);
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
  // Settled lists 10 threads, then 25 more at a time, as T3 Code does.
  const [settledShown, setSettledShown] = useState(10);
  // design-lint: allow (mask alpha, not a colour)
  const fade = "linear-gradient(to bottom, #000 calc(100% - 28px), transparent)";

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

  const sections = useMemo(() => {
    const flagged = new Set(flags.split(" "));
    const pins: SessionSummary[] = [];
    const isFresh = (id: string) => !!finished[id];
    const active: SessionSummary[] = [];
    const settled: SessionSummary[] = [];
    for (const s of filtered) {
      if (parentOf[s.id]) continue;
      const listed = s.status === "done" || s.status === "review" ? s.status : "idle";
      // Settled: threads marked Done (by the user, or the engine for a merged branch), and, as in
      // T3 Code, threads untouched for three days unless they wait for review.
      const status = marks[s.id] ?? listed;
      const done = status === "done" || (status !== "review" && idleSettled(s.updatedAt, now));
      // A pinned thread always shows, whatever it is doing.
      // As T3 Code's: one list, each row saying what it does; only settled threads leave it.
      if (pinned.includes(s.id)) pins.push(s);
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
    const all: Section[] = [
      { id: "pinned", label: "Pinned", items: pins },
      { id: "threads", label: "Threads", items: active, projects },
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
  }, [filtered, flags, marks, finished, pinned, parentOf, byProject, now]);

  // A search shows every match, folded sections too.
  const isOpen = (id: string) => !!query.trim() || !folded.includes(id);
  const toggle = (id: string) => setFolded((f) => (f.includes(id) ? f.filter((x) => x !== id) : [...f, id]));

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

  // ⌘1–⌘9 open the first nine rows as they show; holding ⌘ labels them (T3 Code).
  const searching = !!query.trim();
  const jumpOrder = useMemo(() => {
    const out: string[] = [];
    for (const section of sections) {
      if (section.id === "settled") continue;
      if (section.projects) {
        for (const p of section.projects)
          if (searching || !folded.includes(`project:${p.key}`)) out.push(...p.items.map((x) => x.id));
      } else out.push(...section.items.map((x) => x.id));
    }
    return out.slice(0, 9);
  }, [sections, folded, searching]);
  const [metaHeld, setMetaHeld] = useState(false);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const down = (e: KeyboardEvent) => {
      if (e.key === "Meta") {
        timer = setTimeout(() => setMetaHeld(true), 200);
        return;
      }
      if (e.metaKey && !e.shiftKey && !e.altKey && !e.ctrlKey && /^[1-9]$/.test(e.key)) {
        const id = jumpOrder[Number(e.key) - 1];
        if (!id) return;
        e.preventDefault();
        selectThread(id);
      }
    };
    const up = (e: KeyboardEvent) => {
      if (e.key !== "Meta") return;
      clearTimeout(timer);
      setMetaHeld(false);
    };
    const blur = () => {
      clearTimeout(timer);
      setMetaHeld(false);
    };
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    window.addEventListener("blur", blur);
    return () => {
      clearTimeout(timer);
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      window.removeEventListener("blur", blur);
    };
  }, [jumpOrder]);
  const jumpOf = (id: string) => {
    if (!metaHeld) return undefined;
    const i = jumpOrder.indexOf(id);
    return i < 0 ? undefined : i + 1;
  };

  const rows = (items: SessionSummary[], scopeAll: boolean) => (
    <div className="flex flex-col gap-px">
      {items.map((s) => (
        <div key={s.id} className="flex flex-col gap-px">
          <ThreadRow
            session={s}
            selected={s.id === selectedId}
            scopeAll={scopeAll}
            now={now}
            snippet={query.trim() ? contentHits[s.id] : undefined}
            jump={jumpOf(s.id)}
            hasChildren={!!childrenOf[s.id]?.length}
          />
          {childrenOf[s.id] && (
            <ChildThreads items={childrenOf[s.id]!} parentId={s.id} selectedId={selectedId} now={now} />
          )}
        </div>
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
          "flex h-full flex-col bg-sidebar transition-[translate] duration-200 ease-snappy",
          hidden ? "-translate-x-full" : "translate-x-0",
        )}
        style={{ width, minWidth: width }}
      >
        <TitlebarRow className="justify-end pr-2.5">
          <Cluster>
            <ClusterButton label="Hide sidebar" tooltip="Hide sidebar (⌘B)" onClick={toggleSidebar}>
              <PanelLeft size={14} />
            </ClusterButton>
            <ClusterButton label="New thread" tooltip="New thread (⌘N)" onClick={() => startNewThread()}>
              <SquarePen size={14} />
            </ClusterButton>
            <SettingsButton />
          </Cluster>
        </TitlebarRow>
        <div className="mx-2.5 mt-2.5 mb-1.5 flex shrink-0 items-center gap-1">
          <label className="flex h-[30px] min-w-0 flex-1 items-center gap-1.5 rounded-full bg-fill-subtle px-3 focus-within:bg-fill hover:bg-fill">
            <Search size={13} className="shrink-0 text-fg-muted" />
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => e.key === "Escape" && setQuery("")}
              placeholder="Search threads"
              aria-label="Search threads"
              className="min-w-0 flex-1 bg-transparent text-callout outline-none placeholder:text-fg-faint"
            />
            {query && (
              <button
                type="button"
                aria-label="Clear search"
                onClick={() => setQuery("")}
                className="text-fg-faint hover:text-fg-muted"
              >
                <X size={12} />
              </button>
            )}
          </label>
          <FilterMenu
            byProject={byProject}
            setByProject={setByProject}
            onlyProject={onlyProject}
            setOnlyProject={setOnlyProject}
          />
        </div>
        <nav
          className="min-h-0 flex-1 overflow-y-auto px-2 pb-6"
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
                  // Settled is a shelf at the foot of the list, as in T3 Code.
                  className={cx("mt-2 first:mt-0", section.id === "settled" && "mt-auto pt-3")}
                  aria-label={section.label}
                >
                  {/* One list, as T3 Code's: only the Settled shelf has a header. */}
                  {section.id === "settled" && (
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
                                <div className="pl-3">{rows(p.items, false)}</div>
                              </Fold>
                            </div>
                          );
                        })
                      : rows(
                          section.id === "settled" && !query.trim()
                            ? section.items.slice(0, settledShown)
                            : section.items,
                          !byProject || section.id !== "threads",
                        )}
                    {section.id === "settled" && !query.trim() && section.items.length > settledShown && (
                      <button
                        type="button"
                        onClick={() => setSettledShown((n) => n + 25)}
                        className="flex h-7 w-full items-center gap-1 rounded-md px-2.5 text-left text-fg-muted text-sub hover:bg-fill-subtle hover:text-fg"
                      >
                        <ChevronDown size={11} />
                        Show {Math.min(25, section.items.length - settledShown)} more
                      </button>
                    )}
                    {section.id === "settled" && (
                      <p className="px-2.5 pt-1 pb-1.5 text-fg-faint text-sub">
                        Done, or untouched for 3 days. A new message brings one back.
                      </p>
                    )}
                  </Fold>
                </section>
              );
            })}
          </div>
        </nav>
        <UpdateRow />
      </div>
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
