// The sidebar, after ContentView.swift: a toolbar clear of the traffic
// lights, a search capsule, threads grouped by attention (Needs You,
// Working, Threads), and the project card with Settings at the foot.

import {
  Archive,
  ArchiveRestore,
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
  MessageCircleQuestion,
  PanelLeft,
  Pencil,
  Pin,
  PinOff,
  Search,
  Settings,
  SquarePen,
  SquareTerminal,
  Trash2,
  Users,
  X,
} from "lucide-react";
import { ContextMenu, DropdownMenu } from "radix-ui";
import { useEffect, useMemo, useRef, useState } from "react";
import { agentActive, type SubagentInfo } from "../acp/rustcode";
import { modelName, projectName, type SessionSummary } from "../acp/types";
import { openTerminalAt, pickFolder, revealPath } from "../lib/native";
import { hasShell } from "../lib/shell";
import {
  askConfirm,
  featureMissing,
  openAgent,
  openSettings,
  removeWorktree,
  renameThread,
  selectThread,
  setArchived,
  setProjectCwd,
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
import { LinkBadge } from "./Links";
import { ChromeButton, Cluster, ClusterButton, cx, Segments, Spinner, Tooltip } from "./primitives";
import { StatusChip } from "./Status";

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
  "flex w-full items-center gap-2 rounded-[8px] px-2.5 py-[5px] text-body outline-none data-[highlighted]:bg-brand-fill data-[highlighted]:text-white";
const menuSurface = "surface z-50 min-w-[190px] rounded-[12px] border border-hairline p-1 rc-in";

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
  const archived = useApp((s) => session.id in s.archived);
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
        <div className="px-2.5 pb-1 text-caption text-fg-3">Kept by the app: {statusMissing}</div>
      )}
      <Item className={menuItem} onSelect={() => setView({ kind: "review", threadId: session.id })}>
        <FileDiff size={13} /> Review Changes
      </Item>
      <Separator className="mx-1.5 my-1 h-px bg-hairline" />
      <Item className={menuItem} onSelect={() => togglePin(session.id)}>
        {pinned ? <PinOff size={13} /> : <Pin size={13} />}
        {pinned ? "Unpin" : "Pin"}
      </Item>
      <Item className={menuItem} disabled={!canFiles} onSelect={onRename}>
        <Pencil size={13} /> Rename…
      </Item>
      <Item className={menuItem} onSelect={() => setArchived(session.id, !archived)}>
        {archived ? <ArchiveRestore size={13} /> : <Archive size={13} />}
        {archived ? "Unarchive" : "Archive"}
      </Item>
      {worktree && (
        <>
          <Separator className="mx-1.5 my-1 h-px bg-hairline" />
          <div className="flex items-center gap-1.5 px-2.5 pt-0.5 pb-1 text-caption text-fg-2">
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
            className={cx(menuItem, "text-danger data-[highlighted]:bg-danger")}
            onSelect={() => void removeWorktree(session.id)}
          >
            <FolderX size={13} /> Remove Worktree…
          </Item>
        </>
      )}
      <Separator className="mx-1.5 my-1 h-px bg-hairline" />
      <Item
        className={cx(menuItem, "text-danger data-[highlighted]:bg-danger")}
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

function ThreadRow({
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
  const thread = useApp((s) => s.threads[session.id]);
  const status = useApp((s) => statusOf(s, session.id));
  const worktree = useApp((s) => worktreeOf(s, session.id));
  const harnesses = useApp((s) => s.harnesses);
  const pinned = useApp((s) => s.pinned.includes(session.id));
  const [renaming, setRenaming] = useState(false);
  const [title, setTitle] = useState("");
  const request = thread?.permissions[0];
  const running = thread?.running ?? false;
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
      <div className="rounded-[9px] bg-fg/10 px-1.5 py-[5px]">
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
          className="w-full rounded-[6px] bg-field px-1.5 py-0.5 text-body outline-none ring-2 ring-info/50"
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
              "flex w-full flex-col gap-0.5 rounded-[9px] px-2.5 py-[7px] text-left transition-colors duration-150 ease-out",
              selected ? "bg-fg/10" : "hover:bg-fg/[0.05] active:bg-fg/[0.08]",
              status === "done" && !selected && "opacity-60",
            )}
          >
            <span className="flex w-full items-center gap-2">
              {pinned && <Pin size={10} className="shrink-0 rotate-45 text-fg-2" />}
              <span className="min-w-0 flex-1 truncate text-body">{session.title || "New thread"}</span>
              <LinkBadge threadId={session.id} />
              <span className="flex h-3.5 w-4 shrink-0 items-center justify-center group-hover:invisible">
                {request ? (
                  <MessageCircleQuestion size={12} className="text-attention" />
                ) : running ? (
                  <Spinner size={12} />
                ) : (
                  <StatusChip status={status} compact />
                )}
              </span>
            </span>
            {request ? (
              <span className="truncate font-medium text-attention text-caption">
                {request.question ? `Asks: ${request.question.text}` : `Approve: ${request.title}`}
              </span>
            ) : (
              <span className="flex items-center gap-1 truncate text-caption text-fg-2">
                {worktree ? (
                  <span className="flex min-w-0 shrink items-center gap-0.5 rounded-[5px] bg-fill-3 px-1 font-mono text-[10px]">
                    <GitBranch size={9} className="shrink-0" />
                    <span className="truncate">{worktree.branch}</span>
                  </span>
                ) : (
                  scopeAll && session.cwd && <Folder size={9} className="shrink-0" />
                )}
                <span className="truncate">{detail.join(" · ")}</span>
              </span>
            )}
          </button>
          <DropdownMenu.Root>
            <DropdownMenu.Trigger asChild>
              <button
                type="button"
                aria-label={`Actions for ${session.title || "thread"}`}
                className="absolute top-[6px] right-1.5 flex size-[20px] items-center justify-center rounded-[6px] text-fg-2 opacity-0 hover:bg-fg/10 focus-visible:opacity-100 group-hover:opacity-100 data-[state=open]:opacity-100"
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
}

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
    <div className="ml-[18px] border-fg/[0.08] border-l pl-1.5">
      <button
        type="button"
        onClick={() => setOpen(!expanded)}
        aria-expanded={expanded}
        className="flex h-[20px] w-full items-center gap-1 rounded-[6px] px-1.5 text-caption text-fg-2 hover:bg-fg/[0.035]"
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
            className="flex h-[22px] w-full items-center gap-1.5 rounded-[6px] px-1.5 text-left text-caption hover:bg-fg/[0.035]"
          >
            <span className="flex w-3 justify-center">
              <AgentMark agent={a} />
            </span>
            <span className="shrink-0 font-medium text-fg">{a.agentType || a.name}</span>
            <span className="min-w-0 truncate text-fg-2">{a.task}</span>
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

export function Sidebar() {
  const sessions = useApp((s) => s.sessions);
  const sessionsLoaded = useApp((s) => s.sessionsLoaded);
  const selectedId = useApp((s) => s.selectedId);
  const threads = useApp((s) => s.threads);
  const projectCwd = useApp((s) => s.projectCwd);
  const hidden = useApp((s) => s.sidebarHidden);
  const pinned = useApp((s) => s.pinned);
  const archived = useApp((s) => s.archived);
  const view = useApp((s) => s.view.kind);
  const [showArchived, setShowArchived] = useState(false);
  const [query, setQuery] = useState("");
  const [scope, setScope] = useState<"all" | "project">("all");
  const [width, setWidth] = useSidebarWidth();
  const dragging = useRef(false);
  const now = useNow();
  // The list scrolls under the floating project card: it ends above it with a fade, and its last row can scroll clear.
  const card = useRef<HTMLDivElement>(null);
  const [cardHeight, setCardHeight] = useState(60);
  useEffect(() => {
    const el = card.current;
    if (!el) return;
    const observer = new ResizeObserver(() => setCardHeight(el.offsetHeight));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  const inset = cardHeight + 10;
  const fade = `linear-gradient(to bottom, #000 calc(100% - ${inset + 28}px), transparent calc(100% - ${inset}px))`;

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return sessions.filter(
      (s) =>
        (scope === "all" || s.cwd === projectCwd) &&
        s.id in archived === showArchived &&
        (!q || `${s.title ?? ""} ${s.cwd}`.toLowerCase().includes(q)),
    );
  }, [sessions, query, scope, projectCwd, archived, showArchived]);
  const archivedCount = useMemo(() => sessions.filter((s) => s.id in archived).length, [sessions, archived]);

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

  const groups = useMemo(() => {
    const needs: SessionSummary[] = [];
    const working: SessionSummary[] = [];
    const pins: SessionSummary[] = [];
    const rest: SessionSummary[] = [];
    for (const s of filtered) {
      if (parentOf[s.id]) continue;
      const t = threads[s.id];
      if (t?.permissions.length) needs.push(s);
      else if (t?.running) working.push(s);
      else if (pinned.includes(s.id)) pins.push(s);
      else rest.push(s);
    }
    pins.sort((a, b) => pinned.indexOf(a.id) - pinned.indexOf(b.id));
    // The rest by project: a worktree thread belongs to its repository.
    const byProject = new Map<string, SessionSummary[]>();
    for (const s of rest) {
      const key = s.worktree?.repo || s.cwd || "";
      byProject.set(key, [...(byProject.get(key) ?? []), s]);
    }
    const projects =
      showArchived || byProject.size <= 1
        ? [{ id: "threads", label: showArchived ? "Archived" : undefined, items: rest }]
        : [...byProject.entries()].map(([key, items]) => ({
            id: `project:${key}`,
            label: key ? projectName(key) : "No folder",
            items,
          }));
    return [
      { id: "needs", label: "Needs You", items: needs },
      { id: "working", label: "Working", items: working },
      { id: "pinned", label: "Pinned", items: pins },
      ...projects,
    ].filter((g) => g.items.length);
  }, [filtered, threads, pinned, showArchived, parentOf]);

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

  return (
    <aside
      aria-label="Threads"
      className={cx(
        "relative flex shrink-0 flex-col overflow-hidden bg-sidebar transition-[width] duration-200 ease-snappy",
      )}
      style={{ width: hidden ? 0 : width }}
      aria-hidden={hidden}
    >
      <div className="flex h-full flex-col" style={{ width, minWidth: width }}>
        <div
          data-tauri-drag-region
          className="flex h-[34px] shrink-0 items-end justify-end gap-1 pr-2.5 pl-20"
        >
          <Cluster>
            <ClusterButton label="Hide sidebar" tooltip="Hide sidebar (⌘B)" onClick={toggleSidebar}>
              <PanelLeft size={14} />
            </ClusterButton>
            <ClusterButton label="New thread" tooltip="New thread (⌘N)" onClick={() => startNewThread()}>
              <SquarePen size={14} />
            </ClusterButton>
          </Cluster>
        </div>
        <label className="mx-3 mt-2 flex h-[30px] shrink-0 items-center gap-1.5 rounded-full bg-fill-3 px-3 transition-colors duration-150 focus-within:bg-fill-2 hover:bg-fill-2">
          <Search size={13} className="shrink-0 text-fg-2" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && setQuery("")}
            placeholder="Search threads"
            aria-label="Search threads"
            className="min-w-0 flex-1 bg-transparent text-callout outline-none placeholder:text-fg-3"
          />
          {query && (
            <button
              type="button"
              aria-label="Clear search"
              onClick={() => setQuery("")}
              className="text-fg-3 hover:text-fg-2"
            >
              <X size={12} />
            </button>
          )}
        </label>
        <div className="flex shrink-0 items-center justify-between pt-3 pr-3 pb-1 pl-5">
          <Segments
            label="View"
            value={view === "board" ? "board" : "list"}
            onChange={(v) => setView(v === "board" ? { kind: "board" } : { kind: "chat" })}
            options={[
              { value: "list", label: "List" },
              { value: "board", label: "Board" },
            ]}
          />
          <Segments
            label="Scope"
            value={scope}
            onChange={setScope}
            options={[
              { value: "all", label: "All" },
              { value: "project", label: "Project" },
            ]}
          />
        </div>
        <nav
          className="min-h-0 flex-1 overflow-y-auto px-2.5"
          style={{
            paddingBottom: inset + 24,
            maskImage: fade,
            WebkitMaskImage: fade,
            scrollPaddingBottom: inset + 24,
          }}
        >
          {!sessionsLoaded && (
            <div className="flex items-center gap-2 px-2.5 py-3 text-callout text-fg-2">
              <Spinner /> Loading threads…
            </div>
          )}
          {sessionsLoaded && !filtered.length && (
            <div className="px-2.5 py-3 text-callout text-fg-2">
              {showArchived
                ? "No archived threads."
                : query
                  ? "No matching threads."
                  : scope === "project"
                    ? "No threads in this project yet."
                    : "No threads yet."}
            </div>
          )}
          {groups.map((group, gi) => (
            <div key={group.id}>
              {gi > 0 && <div className="mx-2.5 my-1.5 h-px bg-fg/[0.06]" />}
              {group.label && (
                <div
                  className={cx(
                    "px-2.5 pt-0.5 pb-1 font-semibold text-caption",
                    group.id === "needs" ? "text-warning" : "text-fg-2",
                  )}
                >
                  {group.label}
                </div>
              )}
              <div className="flex flex-col gap-0.5">
                {group.items.map((s) => (
                  <div key={s.id} className="flex flex-col gap-0.5">
                    <ThreadRow
                      session={s}
                      selected={s.id === selectedId}
                      scopeAll={scope === "all" && !group.id.startsWith("project:")}
                      now={now}
                    />
                    {childrenOf[s.id]?.map((child) => (
                      <div key={child.id} className="ml-[14px] border-fg/[0.08] border-l pl-1">
                        <ThreadRow
                          session={child}
                          selected={child.id === selectedId}
                          scopeAll={false}
                          now={now}
                        />
                      </div>
                    ))}
                  </div>
                ))}
              </div>
            </div>
          ))}
          {(archivedCount > 0 || showArchived) && (
            <button
              type="button"
              onClick={() => setShowArchived(!showArchived)}
              className="mt-2 flex w-full items-center gap-1.5 rounded-[9px] px-2.5 py-1.5 text-caption text-fg-2 hover:bg-fg/[0.035]"
            >
              <Archive size={11} />
              {showArchived ? "Back to threads" : `Archived (${archivedCount})`}
            </button>
          )}
        </nav>
        <div
          ref={card}
          className="surface absolute right-2.5 bottom-2.5 left-2.5 flex items-center gap-1 rounded-2xl border-[0.5px] border-hairline p-1"
        >
          <Tooltip label="Default folder for new threads">
            <button
              type="button"
              onClick={() =>
                void pickFolder(projectCwd).then((p) => {
                  if (!p) return;
                  setProjectCwd(p);
                  startNewThread(p);
                })
              }
              className="flex min-w-0 flex-1 items-center gap-2 rounded-[10px] px-2.5 py-[7px] text-left hover:bg-fg/8 active:bg-fg/12"
            >
              <Folder size={14} className="shrink-0 text-brand" fill="currentColor" fillOpacity={0.25} />
              <span className="min-w-0">
                <span className="block truncate font-medium text-callout">
                  {projectCwd ? projectName(projectCwd) : "Choose a project"}
                </span>
                {projectCwd && (
                  <span className="block truncate text-caption text-fg-2">
                    {projectCwd.replace(/\/[^/]+$/, "").replace(/^\/Users\/[^/]+/, "~")}
                  </span>
                )}
              </span>
            </button>
          </Tooltip>
          <ChromeButton label="Settings" tooltip="Settings (⌘,)" onClick={() => openSettings("general")}>
            <Settings size={15} />
          </ChromeButton>
        </div>
      </div>
      {!hidden && (
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize sidebar"
          onPointerDown={onResize}
          className="group absolute top-0 right-[-3px] bottom-0 z-10 w-[7px] cursor-col-resize"
        >
          <div className="mx-auto h-full w-px bg-transparent group-hover:bg-fg/10" />
        </div>
      )}
    </aside>
  );
}
