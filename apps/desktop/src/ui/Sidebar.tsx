// The sidebar, after ContentView.swift: a toolbar clear of the traffic
// lights, a search capsule, threads grouped by attention (Needs You,
// Working, Threads), and the project card with Settings at the foot.

import {
  Archive,
  ArchiveRestore,
  Ellipsis,
  Folder,
  MessageCircleQuestion,
  PanelLeft,
  Pencil,
  Pin,
  PinOff,
  Search,
  Settings,
  SquarePen,
  Trash2,
  X,
} from "lucide-react";
import { ContextMenu, DropdownMenu } from "radix-ui";
import { useEffect, useMemo, useRef, useState } from "react";
import { modelName, projectName, type SessionSummary } from "../acp/types";
import { pickFolder } from "../lib/native";
import { hasShell } from "../lib/shell";
import {
  askConfirm,
  openSettings,
  renameThread,
  selectThread,
  setArchived,
  setProjectCwd,
  startNewThread,
  togglePin,
  toggleSidebar,
  trashThread,
  useApp,
} from "../state/store";
import { relativeAge } from "../state/thread";
import { ChromeButton, Cluster, ClusterButton, cx, Segments, Spinner, Tooltip } from "./primitives";

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
  const canFiles = hasShell();
  return (
    <>
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
    scopeAll && session.cwd ? projectName(session.cwd) : undefined,
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
              "flex w-full flex-col gap-0.5 rounded-[9px] px-2.5 py-[7px] text-left transition-colors duration-100",
              selected ? "bg-fg/10" : "hover:bg-fg/[0.035]",
            )}
          >
            <span className="flex w-full items-center gap-2">
              {pinned && <Pin size={10} className="shrink-0 rotate-45 text-fg-2" />}
              <span className="min-w-0 flex-1 truncate text-body">{session.title || "New thread"}</span>
              <span className="flex h-3.5 w-4 shrink-0 items-center justify-center group-hover:invisible">
                {request ? (
                  <MessageCircleQuestion size={12} className="text-attention" />
                ) : running ? (
                  <Spinner size={12} />
                ) : null}
              </span>
            </span>
            {request ? (
              <span className="truncate font-medium text-attention text-caption">
                {request.question ? `Asks: ${request.question.text}` : `Approve: ${request.title}`}
              </span>
            ) : (
              <span className="flex items-center gap-1 truncate text-caption text-fg-2">
                {scopeAll && session.cwd && <Folder size={9} className="shrink-0" />}
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
  const [showArchived, setShowArchived] = useState(false);
  const [query, setQuery] = useState("");
  const [scope, setScope] = useState<"all" | "project">("all");
  const [width, setWidth] = useSidebarWidth();
  const dragging = useRef(false);
  const now = useNow();

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

  const groups = useMemo(() => {
    const needs: SessionSummary[] = [];
    const working: SessionSummary[] = [];
    const pins: SessionSummary[] = [];
    const rest: SessionSummary[] = [];
    for (const s of filtered) {
      const t = threads[s.id];
      if (t?.permissions.length) needs.push(s);
      else if (t?.running) working.push(s);
      else if (pinned.includes(s.id)) pins.push(s);
      else rest.push(s);
    }
    pins.sort((a, b) => pinned.indexOf(a.id) - pinned.indexOf(b.id));
    return [
      { id: "needs", label: "Needs You", items: needs },
      { id: "working", label: "Working", items: working },
      { id: "pinned", label: "Pinned", items: pins },
      { id: "threads", label: showArchived ? "Archived" : undefined, items: rest },
    ].filter((g) => g.items.length);
  }, [filtered, threads, pinned, showArchived]);

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
        <label className="surface mx-3 mt-2 flex h-[30px] shrink-0 items-center gap-1.5 rounded-full px-3">
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
          <span className="font-semibold text-caption text-fg-2">Threads</span>
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
        <nav className="min-h-0 flex-1 overflow-y-auto px-2.5 pb-[84px]">
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
                  <ThreadRow
                    key={s.id}
                    session={s}
                    selected={s.id === selectedId}
                    scopeAll={scope === "all"}
                    now={now}
                  />
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
        <div className="surface absolute right-2.5 bottom-2.5 left-2.5 flex items-center gap-1 rounded-2xl border-[0.5px] border-hairline p-1">
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
