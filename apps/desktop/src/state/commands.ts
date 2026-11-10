// The slash-command drawer of a thread that has no session yet. Commands only
// arrive as `available_commands_update`, on `session/new` and `session/load`,
// so a new thread would otherwise offer none. The app asks the engine for its
// catalog when it connects (`_rustcode/commands`) and remembers each harness's
// last advertised list in app storage.

import { arr, bool, type Json, obj, str } from "../acp/jsonrpc";
import type { SlashCommand } from "../acp/types";

/** How many harnesses' lists are kept in app storage. */
export const HARNESS_COMMANDS_MAX = 12;
/** How many commands are kept per harness. */
export const COMMANDS_PER_HARNESS_MAX = 200;

/** Each harness's last advertised commands, by harness id. */
export type HarnessCommands = Record<string, SlashCommand[]>;

/**
 * The list a menu shows: `seen` over `base`, merged by name and in base order,
 * with a command the harness re-describes replacing the catalog's.
 *
 * `base` is the engine's catalog, which only answers for the built-in harness:
 * a prompt on another harness goes to that harness, so only the commands the
 * app answers itself carry over when a harness has not advertised yet.
 */
export function menuCommands({
  base,
  seen,
  builtin,
}: {
  base: SlashCommand[];
  seen?: SlashCommand[];
  builtin: boolean;
}): SlashCommand[] {
  const merged = new Map<string, SlashCommand>();
  for (const command of base) if (builtin || command.action !== "execute") merged.set(command.name, command);
  for (const command of seen ?? []) merged.set(command.name, command);
  return [...merged.values()];
}

/**
 * `commands`, then the skills the engine lists (`action: "skill"`) that the
 * harness's own list does not already have. Skills are RustCode's catalog, not
 * a harness's: Claude Code only advertises the ones under `.claude/skills`, so
 * a skill in `~/.agents/skills` would otherwise never reach the drawer.
 */
export function withSkills(commands: SlashCommand[], base: SlashCommand[]): SlashCommand[] {
  const present = new Set(commands.map((command) => command.name.toLowerCase()));
  const skills = base.filter(
    (command) => command.action === "skill" && !present.has(command.name.toLowerCase()),
  );
  return skills.length ? [...commands, ...skills] : commands;
}

/**
 * `commands` as the harness's latest list: it moves to the most recent end of
 * the map, and both the map and its list are trimmed to their bounds.
 */
export function rememberCommands(
  all: HarnessCommands,
  harness: string,
  commands: SlashCommand[],
): HarnessCommands {
  const next: HarnessCommands = {};
  for (const [id, list] of Object.entries(all)) if (id !== harness) next[id] = list;
  next[harness] = commands.slice(0, COMMANDS_PER_HARNESS_MAX);
  const ids = Object.keys(next);
  for (const id of ids.slice(0, Math.max(0, ids.length - HARNESS_COMMANDS_MAX))) delete next[id];
  return next;
}

/** The stored map, with anything a newer build or an edit left malformed dropped. */
export function parseHarnessCommands(raw: string | null): HarnessCommands {
  if (!raw) return {};
  let parsed: Json | undefined;
  try {
    parsed = JSON.parse(raw) as Json;
  } catch {
    return {};
  }
  const out: HarnessCommands = {};
  for (const [harness, value] of Object.entries(obj(parsed) ?? {})) {
    const commands = arr(value).flatMap((entry) => {
      const command = parseCommand(entry);
      return command ? [command] : [];
    });
    if (commands.length) out[harness] = commands.slice(0, COMMANDS_PER_HARNESS_MAX);
  }
  return out;
}

function parseCommand(value: Json | undefined): SlashCommand | null {
  const command = obj(value);
  const name = str(command?.name);
  if (!name) return null;
  return {
    name: name.startsWith("/") ? name : `/${name}`,
    description: str(command?.description) ?? "",
    takesArguments: bool(command?.takesArguments) ?? false,
    report: bool(command?.report) ?? false,
    action: str(command?.action) ?? "execute",
  };
}

/** The stored map as text, for app storage. */
export function serializeHarnessCommands(all: HarnessCommands): string {
  return JSON.stringify(all);
}
