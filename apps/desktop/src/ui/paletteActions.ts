import { applySidebarSort } from "../state/threadActions";
// What ⌘K can do that the sidebar no longer has a button for, and how it finds a thread: the
// project folder picker, the pull requests, usage and statistics pages, the sidebar's view
// options, pinning and snoozing the open thread, and the Settled shelf. Plain data over the
// store, so the tests can read the actions without drawing the palette.

import type { PullRequest } from "../acp/rustcode";
import { projectName, type SessionSummary } from "../acp/types";
import { pickFolder } from "../lib/native";
import { projectActions, projectKey } from "../state/projectActions";
import { registerProject } from "../state/projectBook";
import { cachedPulls, pullForBranch } from "../state/pulls";
import {
  DEFAULT_SIDEBAR_SORT,
  PREF,
  readPref,
  SETTLE_AFTER_DEFAULT_DAYS,
  SIDEBAR_SORTS,
  settleAfterDays,
  settleAfterMs,
  sidebarDensity,
  sidebarSort,
  writePref,
} from "../state/sidebarPrefs";
import { shelfKey } from "../state/sidebarRows";
import { snoozePresets } from "../state/snooze";
import {
  askCustomSnooze,
  openSettings,
  setPanel,
  setPinned,
  snoozeThread,
  startNewThread,
  statusOf,
  toggleSidebar,
  useApp,
} from "../state/store";
import { idleSettled } from "../state/thread";
import { LOCAL_HOST, splitThreadKey } from "../state/threadKeys";
import {
  Archive,
  ArrowDownUp,
  BarChart3,
  Clock,
  Filter,
  FolderPlus,
  Gauge,
  GitPullRequest,
  Layers,
  type LucideIcon,
  Pencil,
  Pin,
  PinOff,
  Play,
  Rows3,
  Search,
} from "./icons";
import { openProjectActionsEditor, runProjectAction } from "./ProjectActions";

export interface PaletteAction {
  id: string;
  label: string;
  detail?: string;
  icon: LucideIcon;
  keys?: string;
  /** Extra words it is found by. */
  words?: string;
  disabled?: string;
  /** The heading it is listed under (ui/paletteSearch.ts); none is Actions. */
  group?: string;
  run: () => void;
}

/** Every word of the query appears in the text. */
export function matches(text: string, query: string): boolean {
  const hay = text.toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((w) => hay.includes(w));
}

/** Every word of the query starts a word of the title: what the action is called, not a hidden word. */
export function titleMatches(title: string, query: string): boolean {
  const words = title
    .toLowerCase()
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean);
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((q) => words.some((w) => w.startsWith(q)));
}

/**
 * How well an action answers the query: one named by it first, then one whose title only holds the
 * query inside it, then one found by its extra words alone.
 */
export function rankOf(a: PaletteAction, query: string): number {
  if (!query.trim()) return 0;
  if (titleMatches(a.label, query)) return 0;
  return matches(a.label, query) ? 1 : 2;
}

/**
 * The actions in the order a query deserves: the one it names first (and the shortest of them: Open
 * Usage before Settings: Usage), then the ones whose title holds it, then the ones only its extra
 * words find. Ties keep the order they were listed in.
 */
export function ranked(actions: PaletteAction[], query: string): PaletteAction[] {
  if (!query.trim()) return actions;
  const rank = (a: PaletteAction) => rankOf(a, query);
  return [...actions].sort((a, b) => {
    const r = rank(a) - rank(b);
    return r || (rank(a) === 0 ? a.label.length - b.label.length : 0);
  });
}

/** A thread's pull request, from the projects' saved reads (state/pulls.ts), if it has one. */
export function pullOfThread(s: SessionSummary): PullRequest | undefined {
  return pullForBranch(cachedPulls(s.worktree?.repo || s.cwd), s.worktree?.branch);
}

/** What a thread is found by: its title, project, branch and pull request (number and title). */
export function threadHaystack(s: SessionSummary, pull = pullOfThread(s)): string {
  return [
    s.title ?? "",
    projectName(s.worktree?.repo || s.cwd),
    projectName(s.cwd),
    s.worktree?.branch ?? "",
    pull ? `#${pull.number} PR ${pull.title}` : "",
  ].join(" ");
}

/** Whether any thread is settled: marked so, or untouched long enough to count as settled. */
function hasSettled(s: ReturnType<typeof useApp.getState>, now: number): boolean {
  const settleMs = settleAfterMs(settleAfterDays(readPref(PREF.settleAfterDays, SETTLE_AFTER_DEFAULT_DAYS)));
  return s.sessions.some((x) => {
    const status = statusOf(s, x.id);
    return status === "done" || (status !== "review" && idleSettled(x.updatedAt, now, settleMs));
  });
}

/** The actions for the places and options the sidebar's buttons and filter menu used to hold. */
export function sidebarActions(now = Date.now()): PaletteAction[] {
  const s = useApp.getState();
  const grouped = readPref(PREF.groupByProject, false);
  const working = readPref(PREF.workingSection, true);
  const only = readPref(PREF.onlyProject, false);
  const compact = sidebarDensity(readPref(PREF.density, "detailed")) === "compact";
  const toolbar = readPref(PREF.toolbar, true);
  const sort = sidebarSort(readPref(PREF.sort, DEFAULT_SIDEBAR_SORT));
  const open = s.selectedId ? s.sessions.find((x) => x.id === s.selectedId) : undefined;
  const pinned = open ? s.pinned.includes(open.id) : false;
  const asleep = open ? s.snoozed[open.id] : undefined;
  const state = (on: boolean) => (on ? "On" : "Off");
  return [
    {
      id: "add-project",
      label: "Add project folder",
      detail: "Open a folder and start a thread in it",
      icon: FolderPlus,
      words: "open add folder directory workspace project",
      run: () =>
        void pickFolder(s.projectCwd || undefined).then((p) => {
          if (!p) return;
          // The same store the project tools write: a folder picked here is a
          // project before its first thread exists.
          registerProject(p);
          startNewThread(p);
        }),
    },
    {
      id: "pulls",
      label: "Open Git",
      detail: "Pull requests of this project",
      icon: GitPullRequest,
      words: "open git pull requests pr github review merge changes",
      disabled: s.agent?.pullRequests ? undefined : "Not available with this harness",
      run: () => setPanel("pulls"),
    },
    {
      id: "usage",
      label: "Open Usage",
      detail: "Plan limits and what each harness reported",
      icon: Gauge,
      words: "open settings limits quota plan tokens",
      run: () => openSettings("usage"),
    },
    {
      id: "statistics",
      label: "Statistics",
      detail: "Recorded tokens and reported cost",
      icon: BarChart3,
      words: "stats cost tokens charts",
      run: () => openSettings("statistics"),
    },
    {
      id: "sidebar-group",
      label: "Sidebar: Group by project",
      detail: state(grouped),
      icon: Layers,
      words: "view folders projects list",
      run: () => writePref(PREF.groupByProject, !grouped),
    },
    {
      id: "sidebar-working",
      label: "Sidebar: Working at the top",
      detail: state(working),
      icon: Layers,
      words: "view shelf running section",
      run: () => writePref(PREF.workingSection, !working),
    },
    {
      id: "sidebar-only",
      label: s.projectCwd ? `Sidebar: Only ${projectName(s.projectCwd)}` : "Sidebar: Only this project",
      detail: state(only),
      icon: Filter,
      words: "view filter one project",
      disabled: s.projectCwd ? undefined : "No project open",
      run: () => writePref(PREF.onlyProject, !only),
    },
    {
      id: "sidebar-toolbar",
      label: "Sidebar: Toolbar",
      detail: state(toolbar),
      icon: Search,
      words: "view search filter sort new thread buttons header",
      run: () => writePref(PREF.toolbar, !toolbar),
    },
    ...SIDEBAR_SORTS.map(
      (option): PaletteAction => ({
        id: `sidebar-sort-${option.id}`,
        label: `Sidebar: Sort by ${option.label.toLowerCase()}`,
        detail: sort === option.id ? "On" : undefined,
        icon: ArrowDownUp,
        words: "view order sort list threads",
        run: () => applySidebarSort(option.id),
      }),
    ),
    {
      id: "sidebar-density",
      label: "Sidebar: Compact rows",
      detail: state(compact),
      icon: Rows3,
      words: "view density dense detailed one line thread rows list",
      run: () => writePref(PREF.density, compact ? "detailed" : "compact"),
    },
    {
      id: "settled-open",
      label: "Open settled threads",
      detail: "Jump to the Settled shelf",
      icon: Archive,
      words: "done archive shelf",
      disabled: hasSettled(s, now) ? undefined : "No settled threads",
      run: () => {
        if (useApp.getState().sidebarHidden) toggleSidebar();
        writePref(shelfKey("settled"), true);
        // The shelf unfolds with the list's motion: scroll to it once it is drawn.
        setTimeout(
          () =>
            // `?.()` too: a test may have swapped in a document without one by the time it fires.
            globalThis.document
              ?.querySelector?.('[data-shelf="settled"]')
              ?.scrollIntoView({ block: "start", behavior: "smooth" }),
          220,
        );
      },
    },
    ...(open
      ? [
          {
            id: "pin",
            label: pinned ? "Unpin this thread" : "Pin this thread",
            icon: pinned ? PinOff : Pin,
            words: "pinned top favourite",
            run: () => setPinned([open.id], !pinned),
          } satisfies PaletteAction,
          ...snoozeActions(open.id, asleep, now),
        ]
      : []),
  ];
}

/**
 * The open thread's project actions (ui/ProjectActions.tsx): `Run: Dev server` for each, and the
 * editor. A thread on another computer has none here: its terminal is there.
 */
export function projectActionItems(): PaletteAction[] {
  const s = useApp.getState();
  const id = s.selectedId;
  const cwd = id ? s.threads[id]?.cwd : undefined;
  if (!id || !cwd || splitThreadKey(id).host !== LOCAL_HOST) return [];
  const project = projectKey(cwd, s.sessions.find((x) => x.id === id)?.repo);
  const actions = projectActions(project);
  return [
    ...actions.map(
      (action): PaletteAction => ({
        id: `project-action-${action.id}`,
        label: `Run: ${action.name}`,
        detail: action.command,
        icon: Play,
        words: `action script command terminal ${action.command}`,
        run: () => runProjectAction(cwd, action),
      }),
    ),
    {
      id: "project-actions-edit",
      label: actions.length ? "Edit project actions…" : "Add a project action…",
      detail: `Commands for ${projectName(project)}, run in the terminal`,
      icon: Pencil,
      words: "action script command terminal run dev build test",
      run: () => openProjectActionsEditor(project),
    },
  ];
}

function snoozeActions(id: string, asleep: number | undefined, now: number): PaletteAction[] {
  return [
    ...snoozePresets(now).map(
      (p): PaletteAction => ({
        id: `snooze-${p.label}`,
        label: `Snooze this thread: ${p.label}`,
        icon: Clock,
        words: "later remind hide wake",
        run: () => snoozeThread(id, p.at),
      }),
    ),
    {
      id: "snooze-custom",
      label: "Snooze this thread: Custom…",
      icon: Clock,
      words: "later remind hide wake date time",
      run: () => askCustomSnooze(id),
    },
    ...(asleep
      ? [
          {
            id: "unsnooze",
            label: "Wake this thread",
            icon: Clock,
            words: "snoozed unsnooze back",
            run: () => snoozeThread(id, null),
          } satisfies PaletteAction,
        ]
      : []),
  ];
}
