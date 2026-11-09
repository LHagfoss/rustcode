// What ⌘K can do that the sidebar no longer has a button for, and how it finds a thread: the
// project folder picker, the pull requests, usage and statistics pages, the sidebar's view
// options, pinning and snoozing the open thread, and the Settled shelf. Plain data over the
// store, so the tests can read the actions without drawing the palette.

import {
  Archive,
  BarChart3,
  Clock,
  Filter,
  FolderPlus,
  Gauge,
  GitPullRequest,
  Layers,
  type LucideIcon,
  Pin,
  PinOff,
} from "lucide-react";
import type { PullRequest } from "../acp/rustcode";
import { projectName, type SessionSummary } from "../acp/types";
import { pickFolder } from "../lib/native";
import { cachedPulls, pullForBranch } from "../state/pulls";
import { PREF, readPref, writePref } from "../state/sidebarPrefs";
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
  return s.sessions.some((x) => {
    const status = statusOf(s, x.id);
    return status === "done" || (status !== "review" && idleSettled(x.updatedAt, now));
  });
}

/** The actions for the places and options the sidebar's buttons and filter menu used to hold. */
export function sidebarActions(now = Date.now()): PaletteAction[] {
  const s = useApp.getState();
  const grouped = readPref(PREF.groupByProject, false);
  const working = readPref(PREF.workingSection, true);
  const only = readPref(PREF.onlyProject, false);
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
      run: () => void pickFolder(s.projectCwd || undefined).then((p) => p && startNewThread(p)),
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
            globalThis.document
              ?.querySelector('[data-shelf="settled"]')
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
