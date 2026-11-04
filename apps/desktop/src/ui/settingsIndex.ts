// What Settings can be searched by: one line per row of every page, with the
// words people look for but that the row does not say. Pure (no React, no
// store at runtime), so the nav's search is testable on its own.
//
// A few pages have no rows of their own — Agents lists cards, Connections and
// Advanced render the engine's answers — so their groups are listed here; a
// group title is a landing target too (settingsKit.tsx).

import type { SettingsSection } from "../state/store";

export interface SettingEntry {
  section: SettingsSection;
  title: string;
  /** Extra words it is found by. */
  keywords?: string;
}

export const SETTINGS_INDEX: SettingEntry[] = [
  // ------------------------------------------------------------ General
  {
    section: "general",
    title: "Default project folder",
    keywords: "directory cwd project path choose",
  },
  {
    section: "general",
    title: "Harness, model and effort",
    keywords: "agent default harness which runs new thread",
  },
  {
    section: "general",
    title: "Default access",
    keywords: "permission mode approval full access auto-edit read-only shield",
  },
  {
    section: "general",
    title: "Continue threads after a restart",
    keywords: "resume quit update engine cut short",
  },
  {
    section: "general",
    title: "Discord Rich Presence",
    keywords: "rich presence status shared project",
  },
  { section: "general", title: "Trackpad haptics", keywords: "haptic tap feedback force touch" },

  // ------------------------------------------------------------ Chat
  {
    section: "chat",
    title: "Smart UI",
    keywords: "interactive view chart table sandbox html agent answer",
  },
  { section: "chat", title: "Link previews", keywords: "links chips title icon page" },

  // ------------------------------------------------------------ Agents
  {
    section: "agents",
    title: "New threads start on",
    keywords: "default harness model effort agent",
  },
  {
    section: "agents",
    title: "Coding agents",
    keywords: "agent harness install sign in check enable disable local remove",
  },
  {
    section: "agents",
    title: "Threads can start and message other threads",
    keywords: "orchestration subagents spawn compare message agents",
  },

  // ------------------------------------------------------------ Connections
  {
    section: "connections",
    title: "GitHub",
    keywords: "command line pull requests account host login token",
  },
  {
    section: "connections",
    title: "Model accounts",
    keywords: "provider accounts sign in auth switch account token models",
  },
  {
    section: "connections",
    title: "GitHub CLI not installed",
    keywords: "install sign in command line auth",
  },
  { section: "connections", title: "Not signed in", keywords: "auth login account token" },
  { section: "connections", title: "No accounts", keywords: "provider sign in auth login" },

  // ------------------------------------------------------------ Capabilities
  {
    section: "capabilities",
    title: "MCP servers",
    keywords: "mcp server tools config add remove connect scope headers",
  },
  {
    section: "capabilities",
    title: "Skills",
    keywords: "skill agents shared project user edit folder create",
  },

  // ------------------------------------------------------------ Appearance
  { section: "appearance", title: "Light or dark", keywords: "mode theme system" },
  {
    section: "appearance",
    title: "Rounded font",
    keywords: "font rounded typography text size",
  },
  {
    section: "appearance",
    title: "Animate streaming text",
    keywords: "animation reveal fade motion streaming words",
  },
  {
    section: "appearance",
    title: "Theme",
    keywords: "colors colours palette theme files editor tokens create",
  },
  {
    section: "appearance",
    title: "New thread banner",
    keywords: "banner picture image header new thread",
  },

  // ------------------------------------------------------------ Keyboard
  { section: "keyboard", title: "New thread", keywords: "shortcut keyboard keys ⌘N" },
  {
    section: "keyboard",
    title: "Folder of this thread (a new one, or move this one)",
    keywords: "shortcut keyboard keys ⌘O",
  },
  {
    section: "keyboard",
    title: "Default folder for new threads",
    keywords: "shortcut keyboard keys ⌘⇧O",
  },
  {
    section: "keyboard",
    title: "Command palette: threads, actions",
    keywords: "shortcut keyboard keys ⌘K switcher",
  },
  { section: "keyboard", title: "Settings", keywords: "shortcut keyboard keys ⌘, preferences" },
  { section: "keyboard", title: "Toggle sidebar", keywords: "shortcut keyboard keys ⌘B" },
  { section: "keyboard", title: "Find in thread", keywords: "shortcut keyboard keys ⌘F search" },
  { section: "keyboard", title: "Terminal", keywords: "shortcut keyboard keys ⌘J shell" },
  {
    section: "keyboard",
    title: "Text larger, smaller, actual size",
    keywords: "shortcut keyboard keys ⌘+ ⌘- ⌘0 zoom",
  },
  { section: "keyboard", title: "Stop the turn", keywords: "shortcut keyboard keys esc ⌘." },
  {
    section: "keyboard",
    title: "Send · queue while running",
    keywords: "shortcut keyboard keys ⏎ enter",
  },
  { section: "keyboard", title: "Steer the running turn", keywords: "shortcut keyboard keys ⌘⏎" },
  { section: "keyboard", title: "New line", keywords: "shortcut keyboard keys ⇧⏎ shift" },
  { section: "keyboard", title: "Answer an approval", keywords: "shortcut keyboard keys 1–9" },

  // ------------------------------------------------------------ Usage
  {
    section: "usage",
    title: "Show Claude usage in the composer",
    keywords: "plan keychain limits windows read composer usage",
  },

  // ------------------------------------------------------------ Advanced
  {
    section: "advanced",
    title: "Engine",
    keywords: "status restart running stopped starting acp transport",
  },
  { section: "advanced", title: "Engine log", keywords: "log stderr debug errors" },
  {
    section: "advanced",
    title: "RustCode",
    keywords: "version update check upgrade download what's new about",
  },
  {
    section: "advanced",
    title: "Set up your agents again",
    keywords: "setup onboarding install sign in harness start over",
  },
];

/** How many results the nav shows at once. */
export const SETTINGS_RESULTS = 12;

/**
 * Every word of the query appears in the row's title or its keywords, in any
 * order and case. No query, nothing: the nav shows the pages then.
 */
export function searchSettings(query: string): SettingEntry[] {
  const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  return SETTINGS_INDEX.filter((entry) => {
    const hay = `${entry.title} ${entry.keywords ?? ""}`.toLowerCase();
    return words.every((word) => hay.includes(word));
  }).slice(0, SETTINGS_RESULTS);
}
