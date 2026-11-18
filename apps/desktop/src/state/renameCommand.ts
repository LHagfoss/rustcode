// `/rename`: the agent names its own thread with RustCode's `update_session` tool, on every
// harness. A harness's own `/rename` (Claude Code's) only renamed its session inside the harness:
// the header showed the name for as long as the thread stayed open, and the list never did.

import type { SlashCommand } from "../acp/types";

/** The drawer's row: it replaces any `/rename` the harness advertises. */
export const RENAME_COMMAND: SlashCommand = {
  name: "/rename",
  description: "Let the agent name this thread from what it is about",
  takesArguments: false,
  report: false,
  action: "execute",
};

/** What the agent is asked when the user sends a bare `/rename`. */
export const RENAME_PROMPT =
  "/rename: pick a short title (6 words or fewer) for what this thread is about, and set it with " +
  "RustCode's `update_session` tool (`title`). Reply with only the new title.";

/**
 * A `/rename` the user typed: `{}` for a bare one (the agent names the thread), `{ title }` for one
 * with a name (the app renames it at once, no turn). Null for any other text.
 */
export function parseRename(text: string): { title?: string } | null {
  const match = /^\/rename(?:\s+([\s\S]*))?$/i.exec(text.trim());
  if (!match) return null;
  const title = match[1]?.trim();
  return title ? { title } : {};
}

const renamed = new WeakMap<SlashCommand[], SlashCommand[]>();

/**
 * The list with `/rename` as the app's own row, whatever the harness advertised under it. The same
 * list in gives the same list out, so a re-render keeps the reference.
 */
export function withRename(commands: SlashCommand[]): SlashCommand[] {
  if (commands.at(-1) === RENAME_COMMAND) return commands;
  let out = renamed.get(commands);
  if (!out) {
    out = [...commands.filter((c) => c.name.toLowerCase() !== RENAME_COMMAND.name), RENAME_COMMAND];
    renamed.set(commands, out);
  }
  return out;
}

/** How long a `/rename` waits for the agent's `update_session` before it lapses. */
const RENAME_WINDOW_MS = 30 * 60_000;
const asked = new Map<string, number>();

/** The user sent a bare `/rename` on `id`: the agent's next title there is theirs to set. */
export function allowAgentRename(id: string, now = Date.now()): void {
  asked.set(id, now);
}

/**
 * Whether the agent's title on `id` answers a `/rename`, used once. Such a title replaces even a
 * name the user chose, and is recorded as the user's choice: they asked for it.
 */
export function takeAgentRename(id: string, now = Date.now()): boolean {
  const at = asked.get(id);
  asked.delete(id);
  return at !== undefined && now - at < RENAME_WINDOW_MS;
}
