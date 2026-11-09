// ⌘K: one field for everything. Threads by title or project (as the
// SwiftUI app's switcher), plus the app's actions: new threads on any
// harness, panels, views, settings pages and the engine. ">" shows actions
// only; ⇥ switches threads between all projects and this one.

import {
  CircleCheck,
  CircleDashed,
  Columns3,
  CornerDownLeft,
  Eye,
  FileDiff,
  Settings as Gear,
  GitBranch,
  Hand,
  type LucideIcon,
  MessageSquare,
  Moon,
  PanelLeft,
  PanelRight,
  RefreshCw,
  Search,
  SquarePen,
  SquareTerminal,
  Sun,
  SunMoon,
  Zap,
} from "lucide-react";
import { Dialog } from "radix-ui";
import { useMemo, useState } from "react";
import { modelName, projectName } from "../acp/types";
import {
  featureMissing,
  openSettings,
  restartEngine,
  type SettingsSection,
  selectThread,
  setNewThreadChoice,
  setNewWorktree,
  setSwitcher,
  setTerminalOpen,
  setTheme,
  setThreadStatus,
  settleThread,
  setView,
  shortcutHint,
  showPanel,
  startNewThread,
  statusOf,
  toggleSidebar,
  useApp,
} from "../state/store";
import { relativeAge } from "../state/thread";
import { cx, Kbd } from "./primitives";

export interface PaletteAction {
  id: string;
  label: string;
  detail?: string;
  icon: LucideIcon;
  keys?: string;
  /** Extra words it is found by. */
  words?: string;
  disabled?: string;
  run: () => void;
}

const SETTINGS: [SettingsSection, string][] = [
  ["general", "General"],
  ["chat", "Chat"],
  ["agents", "Agents: harnesses, models and orchestration"],
  ["connections", "Connections"],
  ["capabilities", "Capabilities: skills and MCP servers"],
  ["remote", "Remote access: phones and paired devices"],
  ["machines", "Machines: other computers to run threads on"],
  ["appearance", "Appearance"],
  ["keyboard", "Keyboard"],
  ["usage", "Usage"],
  ["statistics", "Statistics: recorded tokens and reported cost"],
  ["advanced", "Advanced: request bodies and setup"],
  ["diagnostics", "Diagnostics: server status, logs and debug report"],
];

/** Actions other views add (board, review, …), so the palette lists them without knowing them. */
const extra: (() => PaletteAction[])[] = [];

export function registerPaletteActions(provider: () => PaletteAction[]) {
  extra.push(provider);
}

function baseActions(): PaletteAction[] {
  const s = useApp.getState();
  const harnesses = (s.harnesses?.harnesses ?? []).filter((h) => h.enabled);
  return [
    {
      id: "new",
      label: "New thread",
      icon: SquarePen,
      keys: shortcutHint("new-thread"),
      run: () => startNewThread(),
    },
    ...harnesses
      .filter((h) => h.id !== (s.harnesses?.default.harness ?? "rustcode"))
      .map(
        (h): PaletteAction => ({
          id: `new-${h.id}`,
          label: `New thread on ${h.name}`,
          detail: "Harness",
          icon: Zap,
          words: "harness switch",
          disabled: h.installed ? undefined : `${h.name} is not installed`,
          run: () => {
            startNewThread();
            setNewThreadChoice({ harness: h.id, model: null, effort: null });
          },
        }),
      ),
    {
      id: "inbox",
      label: "Needs you",
      detail: "Approvals, questions, failed turns and delegates",
      icon: Hand,
      keys: shortcutHint("inbox"),
      words: "attention waiting inbox approve decline failed delegate",
      run: () => setView({ kind: "inbox" }),
    },
    {
      id: "board",
      label: s.view.kind === "board" ? "Show the chat" : "Show the board",
      detail: "Running, Needs input, Review, Settled",
      icon: Columns3,
      words: "kanban status view list",
      run: () => setView(s.view.kind === "board" ? { kind: "chat" } : { kind: "board" }),
    },
    ...(s.selectedId
      ? [
          {
            id: "review",
            label: "Review changes of this thread",
            icon: FileDiff,
            words: "diff comments pull request",
            run: () => setView({ kind: "review", threadId: s.selectedId! }),
          },
          {
            id: "mark-review",
            label: "Mark this thread for review",
            icon: Eye,
            words: "status",
            run: () => void setThreadStatus(s.selectedId!, "review"),
          },
          ...(statusOf(s, s.selectedId!) === "done"
            ? [
                {
                  id: "unsettle",
                  label: "Unsettle this thread",
                  icon: CircleDashed,
                  words: "status done clear idle reopen",
                  run: () => void setThreadStatus(s.selectedId!, "idle"),
                },
              ]
            : [
                {
                  id: "mark-done",
                  label: "Settle this thread",
                  icon: CircleCheck,
                  words: "status done settle",
                  run: () => settleThread(s.selectedId!),
                },
              ]),
        ]
      : []),
    {
      id: "new-worktree",
      label: "New thread in a worktree",
      icon: GitBranch,
      words: "branch",
      disabled: featureMissing("worktrees") ?? undefined,
      run: () => {
        startNewThread();
        setNewWorktree({ on: true, branch: "", edited: false });
      },
    },
    ...s.links.fanOuts.slice(-3).map(
      (f): PaletteAction => ({
        id: `compare-${f.id}`,
        label: `Compare: ${f.title}`,
        icon: Columns3,
        words: "fan out harnesses",
        run: () => setView({ kind: "compare", fanOut: f.id }),
      }),
    ),
    {
      id: "details",
      label: "Thread details",
      icon: PanelRight,
      words: "model worktree subagents usage",
      run: () => showPanel("details"),
    },
    {
      id: "sidebar",
      label: "Toggle sidebar",
      icon: PanelLeft,
      keys: shortcutHint("toggle-sidebar"),
      run: toggleSidebar,
    },
    {
      id: "terminal",
      label: "Toggle terminal",
      icon: SquareTerminal,
      keys: shortcutHint("toggle-terminal"),
      run: () => setTerminalOpen(!useApp.getState().terminalOpen),
    },
    ...SETTINGS.map(
      ([id, label]): PaletteAction => ({
        id: `settings-${id}`,
        label: `Settings: ${label}`,
        icon: Gear,
        keys: id === "general" ? shortcutHint("settings") : undefined,
        words: "preferences",
        run: () => openSettings(id),
      }),
    ),
    {
      id: "theme-system",
      label: "Appearance: System",
      icon: SunMoon,
      words: "theme",
      run: () => setTheme("system"),
    },
    {
      id: "theme-light",
      label: "Appearance: Light",
      icon: Sun,
      words: "theme",
      run: () => setTheme("light"),
    },
    { id: "theme-dark", label: "Appearance: Dark", icon: Moon, words: "theme", run: () => setTheme("dark") },
    {
      id: "restart",
      label: "Restart server",
      icon: RefreshCw,
      words: "engine",
      disabled: Object.values(s.threads).some((t) => t.running) ? "A turn is running" : undefined,
      run: () => void restartEngine(),
    },
  ];
}

function highlight(text: string, query: string) {
  const q = query.trim().toLowerCase();
  const at = q ? text.toLowerCase().indexOf(q) : -1;
  if (at < 0) return text;
  return (
    <>
      {text.slice(0, at)}
      <b className="font-semibold">{text.slice(at, at + q.length)}</b>
      {text.slice(at + q.length)}
    </>
  );
}

/** Every word of the query appears in the text. */
function matches(text: string, query: string): boolean {
  const hay = text.toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((w) => hay.includes(w));
}

type Result =
  | { kind: "thread"; id: string; title: string; detail: string; open: boolean }
  | { kind: "action"; action: PaletteAction };

export function CommandPalette() {
  const open = useApp((s) => s.switcherOpen);
  const sessions = useApp((s) => s.sessions);
  const selectedId = useApp((s) => s.selectedId);
  const projectCwd = useApp((s) => s.projectCwd);
  const [query, setQuery] = useState("");
  const [scope, setScope] = useState<"all" | "project">("all");
  const [index, setIndex] = useState(0);

  const actionsOnly = query.startsWith(">");
  const q = (actionsOnly ? query.slice(1) : query).trim();

  // biome-ignore lint/correctness/useExhaustiveDependencies: actions read live state when the palette opens
  const results = useMemo<Result[]>(() => {
    const actions = [...baseActions(), ...extra.flatMap((p) => p())]
      .filter((a) => !q || matches(`${a.label} ${a.words ?? ""} ${a.detail ?? ""}`, q))
      .map((action): Result => ({ kind: "action", action }));
    if (actionsOnly) return actions;
    const threads = sessions
      .filter((s) => scope === "all" || s.cwd === projectCwd)
      .filter((s) => !q || matches(`${s.title ?? ""} ${projectName(s.cwd)}`, q))
      .slice(0, q ? 40 : 8)
      .map(
        (s): Result => ({
          kind: "thread",
          id: s.id,
          title: s.title || "New thread",
          detail: [projectName(s.cwd), modelName(s.model), relativeAge(s.updatedAt)]
            .filter(Boolean)
            .join(" · "),
          open: s.id === selectedId,
        }),
      );
    // With a query, threads lead; without one, the recent threads and then every action.
    return q ? [...threads, ...actions.slice(0, 12)] : [...threads, ...actions];
  }, [sessions, q, scope, projectCwd, selectedId, actionsOnly, open]);

  const choose = (i: number) => {
    const r = results[i];
    if (!r) return;
    if (r.kind === "thread") selectThread(r.id);
    else if (!r.action.disabled) {
      setSwitcher(false);
      r.action.run();
    }
  };

  const firstAction = results.findIndex((r) => r.kind === "action");

  return (
    <Dialog.Root
      open={open}
      onOpenChange={(o) => {
        setSwitcher(o);
        if (!o) {
          setQuery("");
          setIndex(0);
        }
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-40 bg-bg/60" />
        <Dialog.Content
          aria-describedby={undefined}
          className="elevated fixed top-[max(72px,14vh)] left-1/2 z-50 flex w-[min(620px,calc(100vw-56px))] -translate-x-1/2 flex-col overflow-hidden rc-in"
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              setIndex((i) =>
                Math.max(0, Math.min(results.length - 1, i + (e.key === "ArrowDown" ? 1 : -1))),
              );
            } else if (e.key === "Enter") {
              e.preventDefault();
              choose(index);
            } else if (e.key === "Tab") {
              e.preventDefault();
              setScope(scope === "all" ? "project" : "all");
            }
          }}
        >
          <Dialog.Title className="sr-only">Command palette</Dialog.Title>
          <div className="flex h-[52px] items-center gap-3 px-4">
            <Search size={17} className="text-fg-muted" />
            <input
              autoFocus
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setIndex(0);
              }}
              placeholder="Search threads, or > for actions…"
              aria-label="Search threads and actions"
              className="flex-1 bg-transparent text-title3 outline-none placeholder:text-fg-faint"
            />
          </div>
          <div className="max-h-[440px] overflow-y-auto p-2" role="listbox" aria-label="Results">
            {!results.length && (
              <div className="px-3 py-4 text-center text-callout text-fg-muted">Nothing matches “{q}”.</div>
            )}
            {results.map((r, i) => (
              <div key={r.kind === "thread" ? r.id : r.action.id}>
                {i === firstAction && (
                  <div className="px-3 pt-2 pb-1 font-semibold text-caption text-fg-muted">Actions</div>
                )}
                {i === 0 && r.kind === "thread" && (
                  <div className="px-3 pt-1 pb-1 font-semibold text-caption text-fg-muted">
                    {q ? "Threads" : "Recent threads"}
                  </div>
                )}
                <button
                  type="button"
                  role="option"
                  aria-selected={i === index}
                  aria-disabled={r.kind === "action" && !!r.action.disabled}
                  onMouseMove={() => setIndex(i)}
                  onClick={() => choose(i)}
                  className={cx(
                    "flex w-full items-center gap-3 rounded-lg px-3 text-left",
                    r.kind === "thread" ? "min-h-[48px] py-2" : "min-h-[36px] py-1.5",
                    i === index && "bg-accent/16",
                    r.kind === "action" && r.action.disabled && "opacity-45",
                  )}
                >
                  {r.kind === "thread" ? (
                    <>
                      <MessageSquare size={13} className={i === index ? "text-accent" : "text-fg-muted"} />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-body">{highlight(r.title, q)}</span>
                        <span className="block truncate text-caption text-fg-muted">{r.detail}</span>
                      </span>
                      {r.open && (
                        <span className="rounded-full bg-fg/7 px-[7px] py-0.5 font-medium text-caption text-fg-muted">
                          Open
                        </span>
                      )}
                    </>
                  ) : (
                    <>
                      <r.action.icon size={13} className={i === index ? "text-accent" : "text-fg-muted"} />
                      <span className="min-w-0 flex-1 truncate text-body">
                        {highlight(r.action.label, q)}
                      </span>
                      {r.action.disabled ? (
                        <span className="truncate text-caption text-fg-faint">{r.action.disabled}</span>
                      ) : (
                        r.action.keys && <Kbd>{r.action.keys}</Kbd>
                      )}
                    </>
                  )}
                  {i === index && <CornerDownLeft size={12} className="shrink-0 text-fg-faint" />}
                </button>
              </div>
            ))}
          </div>
          <div className="flex h-[34px] items-center gap-3 bg-fill-subtle px-[18px] text-caption text-fg-muted">
            <span className="flex items-center gap-1">
              <Kbd>↑↓</Kbd> move
            </span>
            <span className="flex items-center gap-1">
              <Kbd>⏎</Kbd> open
            </span>
            <span className="flex items-center gap-1">
              <Kbd>&gt;</Kbd> actions
            </span>
            <span className="flex items-center gap-1">
              <Kbd>⇥</Kbd> all / project
            </span>
            <span className="flex-1" />
            <span>{scope === "all" ? "All projects" : projectName(projectCwd)}</span>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
