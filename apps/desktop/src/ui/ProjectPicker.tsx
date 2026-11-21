// The project picker: one panel, opened from the folder button under the composer and from the
// project in the thread header (issue 3598). With no thread open it picks where the next one
// starts; with one open it moves that thread to the project picked, and still offers what to do
// with the thread's folder (Finder, Terminal, copy the path, another thread there). Both places
// read the thread's own folder from the store, so a move from either shows in both. On a paired
// machine "Browse Folders…" walks that machine's folders and makes a new project there
// (ui/MachineFolderBrowser.tsx), since its disk has no file dialog here.

import { type ReactElement, type ReactNode, useMemo, useState } from "react";
import { projectName } from "../acp/types";
import { folderRows, matchSpans, middleEllipsis } from "../lib/folderList";
import { canOpenTerminal, openFolderInEditor, openPath, openTerminalAt, pickFolder } from "../lib/native";
import { fileManagerName } from "../lib/platform";
import { hasShell } from "../lib/shell";
import { pickProject } from "../state/folders";
import { registeredProjects } from "../state/projectBook";
import { isCurrentProject, moveBlockedReason } from "../state/projectPick";
import { startNewThread, toast, useApp, useMachineOfThread } from "../state/store";
import type { ThreadState } from "../state/thread";
import { isRunning } from "../state/thread";
import { CardMenuItem, CardPopover, type CardTriggerProps, menuKeys } from "./cardPopover";
import {
  Check,
  Code,
  Copy,
  Folder,
  FolderOpen,
  FolderPlus,
  GitBranch,
  SquarePen,
  SquareTerminal,
} from "./icons";
import { MachineFolderBrowser } from "./MachineFolderBrowser";
import { cx, type MenuQuickAction, MenuQuickActions, MenuSearchField, menuSeparator } from "./primitives";

/** How much of a path a row shows before it is cut in the middle. */
const PATH_CHARS = 30;

/** Why the open thread cannot move now, as the picker and its triggers say it. */
export function useMoveBlockedReason(thread?: ThreadState): string | undefined {
  const canMove = useApp((s) => !!s.agent?.sessionMove);
  return moveBlockedReason(
    thread ? { id: thread.id, cwd: thread.cwd, running: isRunning(thread) } : undefined,
    canMove,
  );
}

/**
 * The picker. `trigger` is the control that opens it (the composer's folder pill, the header's
 * project); `side` and `align` pin the panel where that control wants it.
 *
 * The list is projects, not every folder a thread ever ran in: the root, a home folder and a
 * folder named like a container (`~/code`, `~/dev`) that holds another project are not offered, and
 * a worktree nests under the repo it was cut from (lib/folderList.ts). Projects are ranked by how
 * much they are used — how recently, and how many threads — so the ones worked in sit at the top.
 * One line per row — the name, then the end of its path, faded — so a deep checkout never turns the
 * list into a wall of wrapping text. "Choose Folder…" ("Browse Folders…" on a paired machine) is
 * the last project row, and the last thing the arrow keys reach before the folder's own actions.
 */
export function ProjectPicker({
  thread,
  cwd,
  host,
  trigger,
  tooltip,
  side = "top",
  align,
}: {
  thread?: ThreadState;
  cwd: string;
  host: string;
  trigger: ReactElement<CardTriggerProps & Record<string, unknown>>;
  tooltip?: ReactNode;
  side?: "top" | "bottom";
  align?: "start" | "center" | "end";
}) {
  const sessions = useApp((s) => s.sessions);
  const machineName = useApp((s) => s.machineNames[host]);
  const threadMachine = useMachineOfThread(thread?.id);
  const blocked = useMoveBlockedReason(thread);
  const [filter, setFilter] = useState("");
  const [browsing, setBrowsing] = useState(false);
  // A thread on another computer moves among that computer's folders: its threads' projects,
  // never this computer's paths, which are meaningless there.
  const sources = useMemo(
    () => [
      ...sessions
        // A thread with no folder of its own lists a stand-in (its home), not a project (issue 3425).
        .filter((s) => (s.host ?? "") === host && !s.workspaceUnknown)
        .map((s) => ({ cwd: s.cwd, worktree: s.worktree, updatedAt: s.updatedAt })),
      ...(!host ? registeredProjects().map(({ path }) => ({ cwd: path, registered: true })) : []),
    ],
    [sessions, host],
  );
  const all = useMemo(() => folderRows(sources), [sources]);
  const rows = useMemo(() => folderRows(sources, { filter }), [sources, filter]);
  // The search appears once there is a list worth searching, and stays while it holds something.
  const searchable = all.length > 4 || !!filter.trim();
  const picked = thread
    ? { id: thread.id, cwd: thread.cwd, running: isRunning(thread), repo: thread.worktree?.repo }
    : undefined;
  const choose = (path: string) => void pickProject(thread?.id, path);
  const label = thread
    ? `Project: ${projectName(cwd)}`
    : cwd
      ? `Project folder: ${projectName(cwd)}`
      : "Choose a folder";
  return (
    <>
      <CardPopover
        label={label}
        onKeyDown={menuKeys}
        tooltip={tooltip ?? (cwd || "Choose a folder")}
        side={side}
        align={align}
        className="flex w-[300px] flex-col gap-1"
        onOpenChange={(open) => {
          // A search is about the panel it was typed in, not a filter the next one inherits.
          if (!open) setFilter("");
        }}
        trigger={trigger}
      >
        {thread && <FolderActions cwd={cwd} host={host} remote={!!threadMachine} />}
        {thread && (
          // The path is the trigger's and Copy Path's already: one line says what a row does, or why
          // it cannot now.
          <div data-menu-static className="px-3 pt-1 pb-0.5">
            {blocked ? (
              <p role="note" className="text-caption text-fg-muted">
                {blocked}
              </p>
            ) : (
              <div className="text-caption text-fg-faint">Move to</div>
            )}
          </div>
        )}
        {/* The folders scroll under a search that filters them; "Choose Folder…" stays pinned under
          them, last in the list and last to the arrow keys. */}
        {searchable && (
          <div data-menu-static className="px-0.5 pb-0.5">
            <MenuSearchField
              value={filter}
              onChange={setFilter}
              placeholder={thread ? "Search projects" : "Search folders"}
              label={thread ? "Search projects" : "Search folders"}
            />
          </div>
        )}
        <div className="scroll-stable flex min-h-0 flex-1 flex-col gap-1">
          {rows.map((row) => {
            const current = isCurrentProject(picked, cwd, row.path);
            // The thread's own project is no move; while a move is blocked no row is one either.
            const disabled = !!thread && (!!blocked || current);
            return (
              <CardMenuItem
                key={row.path}
                onSelect={() => choose(row.path)}
                disabled={disabled}
                aria-current={current || undefined}
                className={cx(
                  "gap-2 disabled:cursor-default disabled:opacity-50 disabled:hover:bg-transparent",
                  current && "bg-fill",
                  row.repo && "pl-6",
                )}
              >
                {/* A worktree is the repo's, not a project of its own: indented under it, marked by
                  the branch it is. */}
                {row.repo ? (
                  <GitBranch size={12} className="shrink-0 text-fg-faint" />
                ) : (
                  <Folder size={13} className="shrink-0 text-fg-muted" />
                )}
                <span className="min-w-[40%] flex-1 truncate text-body text-fg">
                  {matchSpans(row.name, filter).map((span, i) =>
                    span.hit ? (
                      <mark key={i} className="rounded-xs bg-fill-strong text-fg">
                        {span.text}
                      </mark>
                    ) : (
                      span.text
                    ),
                  )}
                </span>
                <span className="min-w-0 shrink truncate text-caption text-fg-faint">
                  {middleEllipsis(row.path, PATH_CHARS)}
                </span>
                {current && <Check size={13} className="shrink-0 text-fg-muted" />}
              </CardMenuItem>
            );
          })}
          {!rows.length && all.length > 0 && (
            <p className="px-3 py-2 text-callout text-fg-muted">No folder matches</p>
          )}
          {!all.length && host && (
            <p className="px-3 py-2 text-callout text-fg-muted">
              No projects on {machineName ?? "that computer"} yet. Browse its folders, or make a new project
              there.
            </p>
          )}
        </div>
        {all.length > 0 && <div className="mx-1.5 my-1 h-px shrink-0 bg-border" />}
        {/* Another computer's disk has no file dialog here: its engine lists the folders instead. */}
        <CardMenuItem
          disabled={!!thread && !!blocked}
          className="disabled:cursor-default disabled:opacity-50 disabled:hover:bg-transparent"
          onSelect={() => (host ? setBrowsing(true) : void pickFolder(cwd).then((p) => p && choose(p)))}
        >
          <FolderPlus size={13} className="text-fg-muted" /> {host ? "Browse Folders…" : "Choose Folder…"}
        </CardMenuItem>
      </CardPopover>
      {host && (
        <MachineFolderBrowser
          open={browsing}
          onOpenChange={setBrowsing}
          host={host}
          machine={machineName ?? "that computer"}
          start={cwd || undefined}
          action={thread ? "Move Here" : "Open"}
          onPick={choose}
        />
      )}
    </>
  );
}

/**
 * What the folder of an existing thread offers. A thread on another computer has its folder there:
 * this one can copy the path, but neither Finder nor a terminal on this Mac can open it.
 */
function FolderActions({ cwd, host, remote }: { cwd: string; host: string; remote?: boolean }) {
  const actions: MenuQuickAction[] = [
    ...(!remote
      ? [
          {
            label: fileManagerName(),
            description: `Open in ${fileManagerName()}`,
            icon: <FolderOpen />,
            onSelect: () => openPath(cwd),
          },
        ]
      : []),
    ...(!remote && hasShell()
      ? [
          {
            label: "Editor",
            description: "Open in Editor",
            icon: <Code />,
            onSelect: () => void openFolderInEditor(cwd).catch((e) => toast(String(e), "error")),
          },
        ]
      : []),
    ...(!remote && canOpenTerminal()
      ? [
          {
            label: "Terminal",
            description: "Open in Terminal",
            icon: <SquareTerminal />,
            onSelect: () => void openTerminalAt(cwd).catch((e) => toast(String(e), "error")),
          },
        ]
      : []),
    {
      label: "Copy Path",
      icon: <Copy />,
      onSelect: () => void navigator.clipboard.writeText(cwd).then(() => toast("Path copied", "info")),
    },
  ];
  return (
    <>
      {actions.length >= 3 ? (
        <MenuQuickActions Item={CardMenuItem} actions={actions} />
      ) : (
        actions.map((action) => (
          <CardMenuItem key={action.label} onSelect={action.onSelect}>
            <span aria-hidden className="flex shrink-0 text-fg-muted [&_svg]:size-[13px]">
              {action.icon}
            </span>
            {action.description ?? action.label}
          </CardMenuItem>
        ))
      )}
      <CardMenuItem onSelect={() => startNewThread(cwd, host || undefined)}>
        <SquarePen size={13} className="text-fg-muted" /> New Thread Here
      </CardMenuItem>
      <div className={menuSeparator} />
    </>
  );
}
