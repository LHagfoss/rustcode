// The sidebar: a toolbar clear of the traffic lights, a search capsule with
// a filter menu, and threads by where they are in their life (after T3
// Code's sidebar): Pinned, Needs You, Working, Threads (by project), and
// Settled (threads marked Done) folded away unless opened. Each section
// folds, and remembers it. Settings is in the top row, after New thread.

import {
  Check,
  ChevronRight,
  CircleCheck,
  CircleDashed,
  Ellipsis,
  Eye,
  FileDiff,
  Folder,
  FolderOpen,
  FolderX,
  GitBranch,
  KanbanSquare,
  ListFilter,
  MessageCircleQuestion,
  PanelLeft,
  Pencil,
  Pin,
  PinOff,
  Search,
  SquarePen,
  SquareTerminal,
  Trash2,
  Users,
  X,
} from "lucide-react";
import { ContextMenu, DropdownMenu } from "radix-ui";
import { memo, useEffect, useMemo, useRef, useState } from "react";
import { agentActive, type SubagentInfo } from "../acp/rustcode";
import { modelName, projectName, type SessionSummary } from "../acp/types";
import type { ThreadStatus } from "../acp/v2";
import { openTerminalAt, revealPath } from "../lib/native";
import { hasShell } from "../lib/shell";
import {
  askConfirm,
  featureMissing,
  openAgent,
  removeWorktree,
  renameThread,
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
import { relativeAge } from "../state/thread";
import { AgentMark } from "./AgentViews";
import { HarnessIcon } from "./HarnessIcon";
import { LinkBadge } from "./Links";
import { SettingsButton } from "./MainHeader";
import { Cluster, ClusterButton, cx, Spinner, Tooltip } from "./primitives";
import { StatusChip } from "./Status";
import { TitlebarRow } from "./Titlebar";

const MIN = 200;
const MAX = 460;

function useSidebarWidth() {
  const [width, setWidth] = useState(() => {
    try {
      return Number(localStorage.getItem("rustcode.sidebarWidth")) || 270;
    } catch {
      return 270;
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
  "flex w-full items-center gap-2 rounded-md px-2.5 py-[5px] text-body outline-none data-[highlighted]:bg-fill";
const menuSurface = "elevated z-50 min-w-[190px] rounded-lg p-1 rc-in";

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
        <Item className={menuItem} onSelect={() => void setThreadStatus(session.id, "done")}>
          <CircleCheck size={13} /> Mark Done
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
      <Separator className="mx-1.5 my-1 h-px bg-border" />
      <Item className={menuItem} onSelect={() => togglePin(session.id)}>
        {pinned ? <PinOff size={13} /> : <Pin size={13} />}
        {pinned ? "Unpin" : "Pin"}
      </Item>
      <Item className={menuItem} disabled={!canFiles} onSelect={onRename}>
        <Pencil size={13} /> Rename…
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
            onSelect={() => void openTerminalAt(worktree.path).catch((e) => toast(String(e), "warning"))}
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

// Memoized, and reads only the flags it shows: a streamed chunk must not re-render 500 rows.
const ThreadRow = memo(function ThreadRow({
  session,
  selected,
  scopeAll,
  now,
}: {
  session: SessionSummary;
  selected: boolean;
  scopeAll: boolean;
  now: number;
}) {
  const request = useApp((s) => s.threads[session.id]?.permissions[0]);
  const running = useApp((s) => s.threads[session.id]?.running ?? false);
  const status = useApp((s) => statusOf(s, session.id));
  const worktree = useApp((s) => worktreeOf(s, session.id));
  const harnesses = useApp((s) => s.harnesses);
  const pinned = useApp((s) => s.pinned.includes(session.id));
  const [renaming, setRenaming] = useState(false);
  const [title, setTitle] = useState("");
  const harness =
    session.harness && session.harness !== "rustcode"
      ? harnesses?.harnesses.find((h) => h.id === session.harness)
      : undefined;
  const detail = [
    worktree ? undefined : scopeAll && session.cwd ? projectName(session.cwd) : undefined,
    harness?.name ?? (session.harness && session.harness !== "rustcode" ? session.harness : undefined),
    modelName(session.model),
    relativeAge(session.updatedAt, now),
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
  return (
    <ContextMenu.Root>
      <ContextMenu.Trigger asChild>
        <div className="group relative">
          <button
            type="button"
            onClick={() => selectThread(session.id)}
            aria-current={selected ? "page" : undefined}
            className={cx(
              "flex w-full flex-col rounded-md px-2.5 py-1 text-left",
              selected ? "bg-fill" : "hover:bg-fill-subtle active:bg-fill",
            )}
          >
            <span className="flex w-full items-center gap-2">
              {pinned && <Pin size={10} className="shrink-0 rotate-45 text-fg-muted" />}
              <span
                className={cx(
                  "min-w-0 flex-1 truncate text-body",
                  selected ? "font-medium text-fg" : status === "done" ? "text-fg-muted" : "text-fg",
                )}
              >
                {session.title || "New thread"}
              </span>
              <LinkBadge threadId={session.id} />
              <span className="flex h-3.5 w-4 shrink-0 items-center justify-center group-hover:invisible">
                {request ? (
                  <MessageCircleQuestion size={12} className="text-warning" />
                ) : running ? (
                  <Spinner size={12} />
                ) : (
                  <StatusChip status={status} compact />
                )}
              </span>
            </span>
            {request ? (
              <span className="truncate font-medium text-warning text-sub">
                {request.question ? `Asks: ${request.question.text}` : `Approve: ${request.title}`}
              </span>
            ) : (
              <span className="flex items-center gap-1 truncate text-fg-faint text-sub">
                {worktree ? (
                  <span className="flex min-w-0 shrink items-center gap-0.5 rounded-sm bg-fill-subtle px-1 font-mono text-[10px]">
                    <GitBranch size={9} className="shrink-0" />
                    <span className="truncate">{worktree.branch}</span>
                  </span>
                ) : (
                  scopeAll && session.cwd && <Folder size={9} className="shrink-0" />
                )}
                {session.harness && session.harness !== "rustcode" && (
                  <HarnessIcon
                    id={session.harness}
                    name={harness?.name}
                    size={10}
                    className="text-fg-muted"
                  />
                )}
                <span className="truncate">{detail.join(" · ")}</span>
              </span>
            )}
          </button>
          <DropdownMenu.Root modal={false}>
            <DropdownMenu.Trigger asChild>
              <button
                type="button"
                aria-label={`Actions for ${session.title || "thread"}`}
                className="absolute top-[4px] right-1.5 flex size-[22px] items-center justify-center rounded-sm text-fg-muted opacity-0 hover:bg-fill focus-visible:opacity-100 group-hover:opacity-100 data-[state=open]:opacity-100"
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
          <AgentChildren threadId={session.id} />
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

const NO_AGENTS: SubagentInfo[] = [];

/** The thread's subagents, nested under its row: running ones first, then the latest finished. */
function AgentChildren({ threadId }: { threadId: string }) {
  const agents = useApp((s) => s.agents[threadId] ?? NO_AGENTS);
  const [open, setOpen] = useState<boolean | null>(null);
  const active = agents.filter(agentActive);
  if (!agents.length) return null;
  const expanded = open ?? active.length > 0;
  const shown = [...active, ...agents.filter((a) => !agentActive(a)).slice(-3)].slice(0, 6);
  return (
    <div className="ml-[18px] border-border border-l pl-1.5">
      <button
        type="button"
        onClick={() => setOpen(!expanded)}
        aria-expanded={expanded}
        className="flex h-[20px] w-full items-center gap-1 rounded-sm px-1.5 text-caption text-fg-muted hover:bg-fill-subtle"
      >
        <ChevronRight size={10} className={cx("transition-transform", expanded && "rotate-90")} />
        <Users size={10} />
        {active.length
          ? `${active.length} agent${active.length === 1 ? "" : "s"} working`
          : `${agents.length} agents`}
      </button>
      {expanded &&
        shown.map((a) => (
          <button
            key={a.id}
            type="button"
            onClick={() => {
              selectThread(threadId);
              openAgent(a.id);
            }}
            className="flex h-[22px] w-full items-center gap-1.5 rounded-sm px-1.5 text-left text-caption hover:bg-fill-subtle"
          >
            <span className="flex w-3 justify-center">
              <AgentMark agent={a} />
            </span>
            <span className="shrink-0 font-medium text-fg">{a.agentType || a.name}</span>
            <span className="min-w-0 truncate text-fg-muted">{a.task}</span>
          </button>
        ))}
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

type SectionId = "pinned" | "needs" | "working" | "threads" | "settled";

interface Section {
  id: SectionId;
  label: string;
  items: SessionSummary[];
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
        "group/h flex h-7 w-full items-center gap-1.5 rounded-md px-2.5 text-left font-semibold text-sub hover:bg-fill-subtle",
        section.id === "needs" ? "text-warning" : "text-fg-muted",
      )}
    >
      <span className="min-w-0 flex-1 truncate">{section.label}</span>
      <span className="font-normal text-fg-faint tabular">{section.items.length}</span>
      <ChevronRight
        size={11}
        strokeWidth={2.5}
        className={cx(
          "shrink-0 text-fg-faint transition-transform duration-200 ease-snappy",
          open && "rotate-90",
          // Open sections show the chevron on hover only, as macOS sidebars do.
          open && "opacity-0 group-hover/h:opacity-100 group-focus-visible/h:opacity-100",
        )}
      />
    </button>
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
  const projectCwd = useApp((s) => s.projectCwd);
  const hidden = useApp((s) => s.sidebarHidden);
  const pinned = useApp((s) => s.pinned);
  const [query, setQuery] = useState("");
  const [byProject, setByProject] = useStored("rustcode.sidebar.byProject", true);
  const [onlyProject, setOnlyProject] = useStored("rustcode.sidebar.onlyProject", false);
  const [folded, setFolded] = useStored<SectionId[]>("rustcode.sidebar.folded", FOLDED_BY_DEFAULT);
  const [width, setWidth] = useSidebarWidth();
  const dragging = useRef(false);
  const now = useNow();
  // design-lint: allow (mask alpha, not a colour)
  const fade = "linear-gradient(to bottom, #000 calc(100% - 28px), transparent)";

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return sessions.filter(
      (s) =>
        (!onlyProject || !projectCwd || s.cwd === projectCwd || s.worktree?.repo === projectCwd) &&
        (!q || `${s.title ?? ""} ${s.cwd}`.toLowerCase().includes(q)),
    );
  }, [sessions, query, onlyProject, projectCwd]);

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
    const needs: SessionSummary[] = [];
    const working: SessionSummary[] = [];
    const active: SessionSummary[] = [];
    const settled: SessionSummary[] = [];
    for (const s of filtered) {
      if (parentOf[s.id]) continue;
      const listed = s.status === "done" || s.status === "review" ? s.status : "idle";
      // Settled is exactly the threads marked Done (by the user, or the engine for a merged branch).
      const done = (marks[s.id] ?? listed) === "done";
      // A pinned thread always shows, whatever it is doing.
      if (pinned.includes(s.id)) pins.push(s);
      else if (flagged.has(`${s.id}:n`)) needs.push(s);
      else if (flagged.has(`${s.id}:w`)) working.push(s);
      else if (done) settled.push(s);
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
      { id: "needs", label: "Needs You", items: needs },
      { id: "working", label: "Working", items: working },
      { id: "threads", label: "Threads", items: active, projects },
      { id: "settled", label: "Settled", items: settled },
    ];
    return all.filter((g) => g.items.length);
  }, [filtered, flags, marks, pinned, parentOf, byProject]);

  // A search shows every match, folded sections too.
  const isOpen = (id: SectionId) => !!query.trim() || !folded.includes(id);
  const toggle = (id: SectionId) =>
    setFolded((f) => (f.includes(id) ? f.filter((x) => x !== id) : [...f, id]));

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

  const rows = (items: SessionSummary[], scopeAll: boolean) => (
    <div className="flex flex-col gap-px">
      {items.map((s) => (
        <div key={s.id} className="flex flex-col gap-px">
          <ThreadRow session={s} selected={s.id === selectedId} scopeAll={scopeAll} now={now} />
          {childrenOf[s.id]?.map((child) => (
            <div key={child.id} className="ml-[14px] border-border border-l pl-1">
              <ThreadRow session={child} selected={child.id === selectedId} scopeAll={false} now={now} />
            </div>
          ))}
        </div>
      ))}
    </div>
  );

  return (
    <aside
      aria-label="Threads"
      className="relative flex shrink-0 flex-col overflow-hidden bg-sidebar transition-[width] duration-200 ease-snappy"
      style={{ width: hidden ? 0 : width }}
      aria-hidden={hidden}
    >
      <div className="flex h-full flex-col" style={{ width, minWidth: width }}>
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
          {!sessionsLoaded && (
            <div className="flex items-center gap-2 px-2.5 py-3 text-callout text-fg-muted">
              <Spinner /> Loading threads…
            </div>
          )}
          {sessionsLoaded && !filtered.length && (
            <div className="px-2.5 py-3 text-callout text-fg-muted">
              {query
                ? "No matching threads."
                : onlyProject
                  ? "No threads in this project yet."
                  : "No threads yet."}
            </div>
          )}
          {sections.map((section) => {
            const open = isOpen(section.id);
            return (
              <section key={section.id} className="mt-1.5 first:mt-0" aria-label={section.label}>
                <SectionHeader section={section} open={open} onToggle={() => toggle(section.id)} />
                <Fold open={open}>
                  {section.projects
                    ? section.projects.map((p) => (
                        <div key={p.key} className="mb-1">
                          <div className="flex h-6 items-center gap-1.5 px-2.5 text-fg-faint text-sub">
                            <Folder size={11} className="shrink-0" />
                            <span className="truncate">{p.label}</span>
                          </div>
                          {rows(p.items, false)}
                        </div>
                      ))
                    : rows(section.items, !byProject || section.id !== "threads")}
                  {section.id === "settled" && (
                    <p className="px-2.5 pt-1 pb-1.5 text-fg-faint text-sub">
                      Threads marked Done. A new message brings one back.
                    </p>
                  )}
                </Fold>
              </section>
            );
          })}
        </nav>
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
