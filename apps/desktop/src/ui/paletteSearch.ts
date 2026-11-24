// What ⌘K finds besides threads and actions, once something is typed: every row of Settings
// (it opens on that row), the projects (a new thread in one), the pull requests the app has read
// (the thread on its branch, else GitHub) and the shortcuts that run a command. Each kind is a
// group of its own under its heading, after the actions. Plain data over the store, so the tests
// read the results without drawing the palette.

import { projectName } from "../acp/types";
import { APP_COMMANDS, type AppCommand } from "../lib/keys";
import { openExternal } from "../lib/native";
import { projectLabel, registeredProjects } from "../state/projectBook";
import { projectsOn } from "../state/projects";
import { cachedPulls, pullForBranch } from "../state/pulls";
import { runShortcut } from "../state/shortcuts";
import {
  openSettingAt,
  type SettingsSection,
  selectThread,
  shortcutHint,
  startNewThread,
  useApp,
} from "../state/store";
import { FolderOpen, Settings as Gear, GitMerge, GitPullRequest, Keyboard } from "./icons";
import { matches, type PaletteAction, ranked } from "./paletteActions";
import { SETTINGS_INDEX } from "./settingsIndex";

/** How many of each group a query shows, so one kind cannot push the others out of view. */
export const GROUP_LIMIT = { settings: 6, projects: 4, pulls: 6, shortcuts: 5 } as const;

/** The name each Settings page has in its nav (ui/Settings.tsx). */
const PAGE_NAMES: Record<SettingsSection, string> = {
  general: "General",
  chat: "Chat",
  permissions: "Permissions",
  appearance: "Appearance",
  keyboard: "Keyboard",
  agents: "Harnesses",
  orchestration: "Delegation",
  capabilities: "Capabilities",
  connections: "Connections",
  voice: "Voice & computer use",
  machines: "Machines",
  remote: "Remote access",
  outside: "Outside agents",
  usage: "Usage",
  statistics: "Statistics",
  diagnostics: "Diagnostics",
  about: "About",
};

/**
 * The shortcuts ⌘K can run: the window's commands. The composer's keys, the approval digits, the
 * thread list's letters and the find bar belong to what has focus, and ⌘K itself is open already.
 */
export function runnableCommands(): AppCommand[] {
  return APP_COMMANDS.filter(
    (c) =>
      !c.list &&
      c.group !== "Composer" &&
      !c.id.startsWith("composer-") &&
      !["answer-approval", "find-in-thread", "command-palette"].includes(c.id),
  );
}

function settingItems(query: string, runnable: Set<string>): PaletteAction[] {
  return SETTINGS_INDEX.filter(
    // A runnable shortcut is in the Shortcuts group already; its Keyboard row would repeat it.
    (e) => !(e.section === "keyboard" && runnable.has(e.title)),
  )
    .filter((e) => matches(`${e.title} ${e.keywords ?? ""} ${PAGE_NAMES[e.section]}`, query))
    .map(
      (e): PaletteAction => ({
        id: `setting-${e.section}-${e.title}`,
        label: e.title,
        detail: `Settings › ${PAGE_NAMES[e.section]}`,
        icon: Gear,
        words: e.keywords,
        group: "Settings",
        run: () => openSettingAt(e.section, e.title),
      }),
    );
}

/** This computer's projects: the folders its threads ran in and the ones added before any did. */
function projectItems(query: string): PaletteAction[] {
  const s = useApp.getState();
  const seen = new Set<string>();
  const paths: string[] = [];
  for (const p of [
    ...projectsOn(s.sessions).map((p) => p.path),
    ...registeredProjects().map((p) => p.path),
  ]) {
    if (seen.has(p)) continue;
    seen.add(p);
    paths.push(p);
  }
  return paths
    .map((path): PaletteAction => {
      const name = projectLabel(path) || projectName(path);
      return {
        id: `project-${path}`,
        label: `New thread in ${name}`,
        detail: path,
        icon: FolderOpen,
        words: `project folder ${projectName(path)}`,
        group: "Projects",
        run: () => startNewThread(path),
      };
    })
    .filter((a) => matches(`${a.label} ${a.words} ${a.detail}`, query));
}

/** The pull requests read for this computer's projects, by number, title and branch. */
function pullItems(query: string): PaletteAction[] {
  const s = useApp.getState();
  const items: PaletteAction[] = [];
  for (const project of projectsOn(s.sessions)) {
    for (const pull of cachedPulls(project.path) ?? []) {
      const thread = s.sessions.find(
        (t) => (t.worktree?.repo || t.cwd) === project.path && pullForBranch([pull], t.worktree?.branch),
      );
      items.push({
        id: `pull-${project.path}-${pull.number}`,
        label: `#${pull.number} ${pull.title}`,
        detail: [project.name, pull.state === "open" && pull.draft ? "draft" : pull.state, pull.headRef].join(
          " · ",
        ),
        icon: pull.state === "merged" ? GitMerge : GitPullRequest,
        words: `pr pull request ${pull.number} ${pull.headRef} ${pull.author}`,
        group: "Pull requests",
        disabled: thread || pull.url ? undefined : "No thread or link",
        run: () => {
          if (thread) selectThread(thread.id);
          else if (pull.url) openExternal(pull.url);
        },
      });
    }
  }
  return items.filter((a) => matches(`${a.label} ${a.words} ${a.detail}`, query));
}

function shortcutItems(query: string, commands: AppCommand[], taken: Set<string>): PaletteAction[] {
  return commands
    .filter((c) => !taken.has(c.label.toLowerCase()))
    .map(
      (c): PaletteAction => ({
        id: `shortcut-${c.id}`,
        label: c.label,
        icon: Keyboard,
        keys: shortcutHint(c.id) || undefined,
        words: `shortcut keyboard key ${c.group}`,
        group: "Shortcuts",
        run: () => {
          if (!runShortcut(c.id)) openSettingAt("keyboard", c.label);
        },
      }),
    )
    .filter((a) => matches(`${a.label} ${a.words}`, query));
}

/**
 * The search groups for a query, best first within each and capped per group. Nothing without a
 * query: the empty palette lists recent threads and actions. `taken` holds the action labels
 * already listed, so a shortcut that is an action too shows once.
 */
export function searchGroups(query: string, taken: Iterable<string> = []): PaletteAction[] {
  const q = query.trim();
  if (!q) return [];
  const commands = runnableCommands();
  const takenLabels = new Set([...taken].map((l) => l.toLowerCase()));
  return [
    ...ranked(settingItems(q, new Set(commands.map((c) => c.label))), q).slice(0, GROUP_LIMIT.settings),
    ...ranked(projectItems(q), q).slice(0, GROUP_LIMIT.projects),
    ...ranked(pullItems(q), q).slice(0, GROUP_LIMIT.pulls),
    ...ranked(shortcutItems(q, commands, takenLabels), q).slice(0, GROUP_LIMIT.shortcuts),
  ];
}
