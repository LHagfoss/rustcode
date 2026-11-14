// The thread header's Actions menu: the project's quick actions (state/projectActions.ts), each a
// shell command that runs in the workspace terminal in the thread's folder, and the editor that
// names them. ⌘K lists the same actions (ui/paletteActions.ts).

import { Pencil, Play, Plus, Trash2, Zap } from "lucide-react";
import { Dialog, DropdownMenu } from "radix-ui";
import { useEffect, useState, useSyncExternalStore } from "react";
import { projectName } from "../acp/types";
import { canOpenPty } from "../lib/pty";
import {
  MAX_ACTIONS,
  type ProjectAction,
  projectActions,
  projectKey,
  queueTerminalCommand,
  saveProjectActions,
  useProjectActions,
} from "../state/projectActions";
import { setTerminalOpen, toast, useApp } from "../state/store";
import type { ThreadState } from "../state/thread";
import {
  ClusterButton,
  cx,
  inputClass,
  MenuContent,
  menuItem,
  menuLabel,
  menuSeparator,
  Pill,
} from "./primitives";

/** Runs a command in the workspace terminal of `cwd`, opening the panel. */
export function runProjectAction(cwd: string, action: ProjectAction): void {
  if (!canOpenPty()) {
    toast("The terminal runs in the desktop app", "error");
    return;
  }
  queueTerminalCommand(cwd, action.command);
  setTerminalOpen(true);
}

/** The project key of a thread: its repository when the engine named one, else its folder. */
export function useThreadProject(thread?: ThreadState): string | undefined {
  const repo = useApp((s) => (thread ? s.sessions.find((x) => x.id === thread.id)?.repo : undefined));
  return thread?.cwd ? projectKey(thread.cwd, repo) : undefined;
}

// ---------------------------------------------------------------- which project the editor edits

let editing: string | null = null;
const listeners = new Set<() => void>();

/** Opens the editor for a project's actions (null closes it). */
export function openProjectActionsEditor(project: string | null): void {
  editing = project;
  for (const listener of listeners) listener();
}

function useEditing(): string | null {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => editing,
    () => null,
  );
}

/** The header's button: a menu of the project's actions, or the way to add the first one. */
export function ProjectActionsButton({ thread }: { thread?: ThreadState }) {
  const project = useThreadProject(thread);
  const actions = useProjectActions(project);
  if (!thread?.cwd || !project) return null;
  const cwd = thread.cwd;
  return (
    <DropdownMenu.Root modal={false}>
      <DropdownMenu.Trigger asChild>
        <ClusterButton label="Actions" tooltip={`Actions of ${projectName(project)}`}>
          <Zap size={14} />
        </ClusterButton>
      </DropdownMenu.Trigger>
      <MenuContent side="bottom" align="end" className="max-w-[360px] min-w-[240px]">
        {actions.length > 0 ? (
          <>
            <DropdownMenu.Label className={menuLabel}>Run in the terminal</DropdownMenu.Label>
            {actions.map((action) => (
              <DropdownMenu.Item
                key={action.id}
                className={menuItem}
                onSelect={() => runProjectAction(cwd, action)}
              >
                <Play size={13} className="shrink-0 text-fg-muted" />
                <span className="min-w-0 flex-1 truncate">{action.name}</span>
                <span className="min-w-0 max-w-[45%] truncate font-mono text-caption text-fg-faint">
                  {action.command}
                </span>
              </DropdownMenu.Item>
            ))}
            <DropdownMenu.Separator className={menuSeparator} />
            <DropdownMenu.Item className={menuItem} onSelect={() => openProjectActionsEditor(project)}>
              <Pencil size={13} className="text-fg-muted" /> Edit Actions…
            </DropdownMenu.Item>
          </>
        ) : (
          <DropdownMenu.Item className={menuItem} onSelect={() => openProjectActionsEditor(project)}>
            <Plus size={13} className="text-fg-muted" /> Add Action…
          </DropdownMenu.Item>
        )}
      </MenuContent>
    </DropdownMenu.Root>
  );
}

interface Row {
  key: number;
  id?: string;
  name: string;
  command: string;
}

let nextKey = 0;
const blankRow = (): Row => ({ key: nextKey++, name: "", command: "" });

/** The editor: one row per action (name, command), in the order the menu lists them. */
export function ProjectActionsEditor() {
  const project = useEditing();
  const [rows, setRows] = useState<Row[]>([]);

  useEffect(() => {
    if (!project) return;
    const saved = projectActions(project).map((a) => ({ key: nextKey++, ...a }));
    setRows(saved.length ? saved : [blankRow()]);
  }, [project]);

  const update = (key: number, patch: Partial<Row>) =>
    setRows((list) => list.map((r) => (r.key === key ? { ...r, ...patch } : r)));
  const save = () => {
    if (!project) return;
    saveProjectActions(project, rows);
    openProjectActionsEditor(null);
  };
  const half = rows.some((r) => !r.name.trim() !== !r.command.trim());

  return (
    <Dialog.Root open={!!project} onOpenChange={(open) => !open && openProjectActionsEditor(null)}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center">
          <Dialog.Content
            aria-describedby="project-actions-hint"
            className="elevated pointer-events-auto flex max-h-[80%] w-[min(560px,calc(100vw-48px))] flex-col gap-3 p-5"
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                save();
              }
            }}
          >
            <Dialog.Title className="font-semibold text-body">
              Actions of {project ? projectName(project) : ""}
            </Dialog.Title>
            <p id="project-actions-hint" className="text-callout text-fg-muted">
              Commands you run often in this project. Each runs in the thread's terminal (⌘J), in its folder;
              worktrees of the project share the list. Kept on this computer.
            </p>
            <div className="flex min-h-0 flex-col gap-2 overflow-y-auto">
              {rows.map((row, i) => (
                <div key={row.key} className="flex items-center gap-2">
                  <input
                    aria-label={`Action ${i + 1} name`}
                    className={cx(inputClass, "w-[140px] shrink-0")}
                    placeholder="Dev server"
                    value={row.name}
                    onChange={(e) => update(row.key, { name: e.target.value })}
                  />
                  <input
                    aria-label={`Action ${i + 1} command`}
                    className={cx(inputClass, "min-w-0 flex-1 font-mono")}
                    placeholder="bun run dev"
                    spellCheck={false}
                    value={row.command}
                    onChange={(e) => update(row.key, { command: e.target.value })}
                  />
                  <ClusterButton
                    label={`Remove action ${i + 1}`}
                    tooltip="Remove"
                    onClick={() => setRows((list) => list.filter((r) => r.key !== row.key))}
                  >
                    <Trash2 size={13} />
                  </ClusterButton>
                </div>
              ))}
            </div>
            <div className="flex items-center gap-2">
              <Pill
                size="sm"
                quiet
                disabled={rows.length >= MAX_ACTIONS}
                onClick={() => setRows((list) => [...list, blankRow()])}
              >
                <Plus size={12} /> Add Action
              </Pill>
              <span className="flex-1" />
              {half && <span className="text-caption text-fg-muted">A row needs a name and a command.</span>}
              <Pill onClick={() => openProjectActionsEditor(null)}>Cancel</Pill>
              <Pill prominent disabled={half} onClick={save}>
                Save
              </Pill>
            </div>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
