// What Settings can be searched by: one line per row of every page, with the
// words people look for but that the row does not say. Pure (no React, no
// store at runtime), so the nav's search is testable on its own.
//
// A few pages have no rows of their own — Agents lists cards, Connections and
// Advanced render the engine's answers — so their groups are listed here; a
// group title is a landing target too (settingsKit.tsx).

import { APP_COMMANDS, commandHint } from "../lib/keys";
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
    title: "Continue threads when a usage limit resets",
    keywords: "resume 5-hour weekly rate limit plan automatically auto start",
  },
  {
    section: "general",
    title: "Auto-compact",
    keywords: "compact compaction context tokens threshold cache prompt cache idle claude codex",
  },
  {
    section: "general",
    title: "Auto-compact threshold",
    keywords: "compact compaction context tokens size limit threshold",
  },
  {
    section: "general",
    title: "System notifications",
    keywords: "notify notification alert badge dock attention approval question finished system",
  },
  {
    section: "general",
    title: "Only when RustCode is in the background",
    keywords: "notify notification background window focused in front quiet",
  },
  {
    section: "general",
    title: "While-you-were-away recap",
    keywords: "away recap summary digest idle returned after lunch dismissed",
  },
  {
    section: "general",
    title: "Away for",
    keywords: "away recap hours idle threshold",
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
    title: "Chat width",
    keywords: "lane transcript composer measure column wide full comfortable narrow",
  },
  {
    section: "chat",
    title: "Smart UI",
    keywords: "interactive view chart table sandbox html agent answer",
  },
  { section: "chat", title: "Link previews", keywords: "links chips title icon page" },
  {
    section: "chat",
    title: "Thread header buttons",
    keywords:
      "header buttons show hide reorder drag arrange customize toolbar status plan pull requests terminal details changes",
  },

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
    title: "Claude accounts",
    keywords:
      "claude login account accounts separate second config dir claude_config_dir sign in plan limits",
  },
  {
    section: "agents",
    title: "An account's sign-in command",
    keywords: "claude auth login copy command terminal mkdir config dir claude_config_dir",
  },
  {
    section: "agents",
    title: "Threads can start and message other threads",
    keywords: "orchestration subagents spawn compare message agents",
  },
  {
    section: "agents",
    title: "Threads one thread can run at once",
    keywords: "orchestration delegation parallel slots limit concurrent children tree",
  },
  {
    section: "agents",
    title: "Let agents read threads in other projects",
    keywords: "orchestration read search across all projects privacy agents",
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
  // One line per command in the table (lib/keys.ts), so a renamed shortcut or a
  // new one is searchable without editing this list.
  ...APP_COMMANDS.map((command) => ({
    section: "keyboard" as const,
    title: command.label,
    keywords: `shortcut keyboard keys ${commandHint(command.binding, command)}`.toLowerCase(),
  })),

  // ------------------------------------------------------------ Usage
  {
    section: "usage",
    title: "Show Claude usage in the composer",
    keywords: "plan keychain limits windows read composer usage",
  },
  {
    section: "usage",
    title: "Each account's plan",
    keywords: "claude account login plan usage windows separate limits",
  },

  // ------------------------------------------------------------ Statistics
  {
    section: "statistics",
    title: "Totals",
    keywords:
      "tokens cost turns active days period today 7 days 30 days all time change delta sparkline recorded usage cached cache write reported usd spend",
  },
  {
    section: "statistics",
    title: "Daily activity",
    keywords: "per day chart daily turns tokens cost peak graph bar",
  },
  {
    section: "statistics",
    title: "Cost by harness and model",
    keywords: "tokens per harness model cost reported breakdown share stacked legend",
  },
  {
    section: "statistics",
    title: "Usage limits",
    keywords: "plan limits windows reset left used percentage subscription rate limit",
  },
  {
    section: "statistics",
    title: "Threads",
    keywords: "per thread totals delegates subagents children cost tokens",
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
    title: "Record request bodies",
    keywords: "capture payload telemetry privacy redact clip query_data store request body response",
  },
  {
    section: "advanced",
    title: "Provider requests (built-in harness)",
    keywords: "capture payload provider request body response redact privacy built-in",
  },
  {
    section: "advanced",
    title: "Harness messages",
    keywords: "capture payload harness acp jsonrpc protocol message redact privacy",
  },
  {
    section: "advanced",
    title: "Set up your agents again",
    keywords: "setup onboarding install sign in harness start over",
  },

  // ------------------------------------------------------------ About
  {
    section: "about",
    title: "RustCode",
    keywords: "version update check upgrade download what's new about licences licenses open source",
  },
  {
    section: "about",
    title: "Update channel",
    keywords: "beta stable prerelease channel release updates early",
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
