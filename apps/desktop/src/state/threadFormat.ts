// What the transcript says about a thread: a finished turn's files and checks, times and ages,
// working and step labels, and diff lines. Pure helpers over `TranscriptItem`s, split out of
// state/thread.ts, which re-exports them.

import type { Json } from "../acp/jsonrpc";
import type { FileDiff } from "../acp/types";
import { unwrapShell } from "../lib/command";
import { splitImageMarkers } from "./compose";
import type { Link } from "./orchestration";
import type { TranscriptItem } from "./thread";
import { displayTool, orchestrationTool, sessionIdOf, usefulSummary } from "./toolNames";

export interface TurnCompletion {
  files: { path: string; added: number; removed: number }[];
  added: number;
  removed: number;
  checks: { command: string; outcome: "passed" | "failed" | "unknown" }[];
}

/** A run of tests, a linter, a type check or a build: the program, or a runner and its verb. */
const CHECK_PROGRAM = /^(pytest|jest|vitest|tsc|eslint|ruff|mypy|clippy|swift-test|playwright)$/;
const CHECK_RUNNER =
  /^(cargo|bun|bunx|npm|pnpm|yarn|npx|deno|go|swift|make|just|uv|python3?|mix|dotnet|gradle|\.\/gradlew|mvn|xcodebuild|biome)$/;
const CHECK_VERB = /^(test|tests|check|lint|clippy|typecheck|tsc|pytest|build|ci|fmt|vitest|jest)(:\S+)?$/;

/**
 * The checks a command line runs, each as typed: `bun test && bun run lint` is two.
 * A word in a path (`nl src/queue.test.ts`) or an option (`git diff --check`) is not one.
 */
export function checkCommands(command: string): string[] {
  return unwrapShell(command.trim())
    .split(/\s*(?:&&|\|\||;|\n)\s*/)
    .flatMap((part) => {
      const words = part
        .trim()
        .split(/\s+/)
        .filter((w) => !/^\w+=/.test(w));
      const program = words[0]?.split("/").at(-1) ?? "";
      if (CHECK_PROGRAM.test(program)) return [words.join(" ")];
      if (!CHECK_RUNNER.test(program)) return [];
      // The verb comes first, after `run`/`-m`, or after a few options (`cargo +nightly clippy`).
      const verbs = words
        .slice(1, 5)
        .filter((w) => !w.startsWith("-") && !w.startsWith("+") && w !== "run" && w !== "x");
      return verbs[0] && CHECK_VERB.test(verbs[0]) ? [words.join(" ")] : [];
    });
}

/** What the turn ending at `endIndex` changed and which checks it ran, from its tool calls only. */
export function turnCompletion(items: TranscriptItem[], endIndex: number): TurnCompletion | null {
  let start = endIndex - 1;
  while (start >= 0 && items[start]?.kind !== "user") start--;
  const files = new Map<string, { path: string; added: number; removed: number }>();
  const checks = new Map<string, { command: string; outcome: "passed" | "failed" | "unknown" }>();
  for (const item of items.slice(start + 1, endIndex)) {
    if (item.kind !== "tool") continue;
    if (item.status !== "failed") {
      for (const diff of item.diffs) {
        const counts = diffCounts([diff]);
        const f = files.get(diff.path) ?? { path: diff.path, added: 0, removed: 0 };
        f.added += counts.added;
        f.removed += counts.removed;
        files.set(diff.path, f);
      }
    }
    const input =
      item.input && typeof item.input === "object" && !Array.isArray(item.input) ? item.input : {};
    const command =
      typeof input.command === "string"
        ? input.command
        : typeof input.cmd === "string"
          ? input.cmd
          : undefined;
    const failed = item.status === "failed" || (item.exitCode !== undefined && item.exitCode !== 0);
    for (const key of command ? checkCommands(command) : []) {
      // A check run again counts as its last run.
      checks.delete(key);
      checks.set(key, {
        command: key,
        outcome: failed ? "failed" : item.status === "completed" ? "passed" : "unknown",
      });
    }
  }
  if (!files.size && !checks.size) return null;
  const list = [...files.values()];
  return {
    files: list,
    added: list.reduce((n, f) => n + f.added, 0),
    removed: list.reduce((n, f) => n + f.removed, 0),
    checks: [...checks.values()],
  };
}

// ---------------------------------------------------------------- derived values

export function formatDuration(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

/** How long a thread rests untouched before the sidebar settles it on its own. */
export const SETTLE_AFTER_MS = 3 * 86_400_000;

/**
 * Whether a thread last active at `at` has been idle long enough to settle. `afterMs` is the
 * user's choice (Settings › Appearance › Settle idle threads); 0 never settles one.
 */
export function idleSettled(at: number | undefined, now = Date.now(), afterMs = SETTLE_AFTER_MS): boolean {
  return afterMs > 0 && at !== undefined && now - at >= afterMs;
}

/** A running turn's clock: `0:42`, `12:03`, `1:02:03`. */
export function formatClock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

/** When a message was sent: the time today, then `yesterday at 12:33`, then the date with the time. */
export function messageTime(at: number | undefined, now = Date.now()): string {
  if (!at) return "";
  const date = new Date(at);
  const time = date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  const midnight = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((midnight(new Date(now)) - midnight(date)) / 86_400_000);
  if (days <= 0) return time;
  if (days === 1) return `yesterday at ${time}`;
  const sameYear = date.getFullYear() === new Date(now).getFullYear();
  const day = date.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    ...(sameYear ? {} : { year: "numeric" }),
  });
  return `${day}, ${time}`;
}

/**
 * The messages that open a new day in a thread: the first one sent on a calendar day after a
 * message from an earlier day, and the thread's first message when it was not sent today. Their
 * time stays in view under the bubble (`yesterday at 12:33`, `today at 09:10`), so a reader
 * scrolling a thread that ran over several days sees where each day starts without hovering.
 */
export function newDayMessages(items: readonly TranscriptItem[], now = Date.now()): Set<string> {
  const out = new Set<string>();
  const midnight = (at: number) => {
    const d = new Date(at);
    return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  };
  const today = midnight(now);
  let last: number | undefined;
  for (const item of items) {
    if (item.kind !== "user" || !item.at || item.wake || item.continued) continue;
    const day = midnight(item.at);
    if (last === undefined ? day !== today : day !== last) out.add(item.id);
    last = day;
  }
  return out;
}

/**
 * The last thing said in a thread, in one line: what a reader recognises it by in a
 * preview (ui/ThreadHoverCard.tsx). The newest message or answer, skipping the
 * steps between them and the pictures a message carries; null for a thread with
 * nothing said in it yet.
 */
export function lastWords(items: readonly TranscriptItem[] | undefined): string | undefined {
  if (!items) return undefined;
  for (let i = items.length - 1; i >= 0; i -= 1) {
    const item = items[i]!;
    const text =
      item.kind === "user" || item.kind === "assistant" || item.kind === "notice"
        ? splitImageMarkers(item.text).text
        : "";
    const line = text.replace(/\s+/g, " ").trim();
    if (line) return line.length > 220 ? `${line.slice(0, 219).trimEnd()}…` : line;
  }
  return undefined;
}

/** The sidebar's compact age: `now`, `5m`, `3h`, `2d`, then the date. */
export function shortAge(at: number | undefined, now = Date.now()): string {
  if (!at) return "";
  const minutes = Math.floor((now - at) / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d`;
  return new Date(at).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

export function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`;
  if (n >= 1000) return `${Math.round(n / 1000)}k`;
  return String(n);
}

export function relativeAge(at: number | undefined, now = Date.now()): string {
  if (!at) return "";
  const minutes = Math.floor((now - at) / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes} min. ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days} d ago`;
  return new Date(at).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

/** The `**headings**` of a thought, as the closed row's one-line gist. */
/**
 * The live line under a running turn: what the agent does right now. The
 * newest step of the turn decides: a running tool (as "Running <command>" for
 * a shell), a thought still streaming (its latest bold heading), a reply being written, else "Working".
 * `short` is for a row: the step as {@link agentStepLabel} says it.
 */
export function workingLabel(items: TranscriptItem[], short = false): string {
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i]!;
    if (item.kind === "user") break;
    if (item.kind === "tool") {
      // A subagent's own steps show on its row, not here.
      if (item.parent) continue;
      if (item.status !== "in_progress" && item.status !== "pending") return "Working";
      const agent = subagentOf(item);
      if (agent) {
        const working = items.filter(
          (i) => subagentOf(i) && i.kind === "tool" && (i.status === "in_progress" || i.status === "pending"),
        ).length;
        return working > 1 ? `${working} agents working` : `${agent.name} · ${agent.task}`;
      }
      // A row (a delegate's, under the thread that started it) has one short line: the step a
      // reader recognises, never the raw shell line with its `cd` and variables.
      return short ? agentStepLabel(item) : stepLabel(item);
    }
    if (item.kind === "thought") {
      if (item.done) return "Working";
      const heading = thoughtHeadings(item.text).at(-1);
      return heading ? `Thinking · ${heading}` : "Thinking";
    }
    if (item.kind === "assistant") return "Writing the reply";
  }
  return "Working";
}

/**
 * A tool call that starts a subagent (Claude Code's `Agent`/`Task`, RustCode's
 * `spawn_agent`): who it is and what it was asked.
 */
export function subagentOf(item: TranscriptItem): { name: string; task: string } | undefined {
  if (item.kind !== "tool") return undefined;
  const input = item.input && typeof item.input === "object" && !Array.isArray(item.input) ? item.input : {};
  if (item.title === "spawn_agent") {
    const type = typeof input.agent_type === "string" ? input.agent_type : "";
    const task = typeof input.task === "string" ? input.task.split("\n")[0]! : (item.summary ?? "");
    return { name: type && type !== "default" ? type : "Agent", task };
  }
  if (item.toolName !== "Agent" && item.toolName !== "Task") return undefined;
  const type = typeof input.subagent_type === "string" ? input.subagent_type : "";
  const description = typeof input.description === "string" ? input.description : "";
  return {
    name: type && type !== "general-purpose" ? type : "Agent",
    task: description || item.title,
  };
}

/**
 * The items without the steps subagents made, and those steps by the call that
 * started each subagent. A step whose parent is not in the list stays put.
 */
export function splitSubagentSteps(items: TranscriptItem[]): {
  items: TranscriptItem[];
  steps: ReadonlyMap<string, ToolItem[]>;
} {
  if (!items.some((i) => i.kind === "tool" && i.parent)) return { items, steps: NO_STEPS };
  const ids = new Set(items.flatMap((i) => (i.kind === "tool" ? [i.id] : [])));
  const steps = new Map<string, ToolItem[]>();
  const rest: TranscriptItem[] = [];
  for (const item of items) {
    if (item.kind === "tool" && item.parent && ids.has(item.parent)) {
      const list = steps.get(item.parent);
      if (list) list.push(item);
      else steps.set(item.parent, [item]);
    } else rest.push(item);
  }
  return { items: rest, steps };
}

export type ToolItem = Extract<TranscriptItem, { kind: "tool" }>;
const NO_STEPS: ReadonlyMap<string, ToolItem[]> = new Map();

/**
 * The tag blocks a harness writes at the head of a subagent's output: Claude
 * Code's list of the tools it used, and the reminders it wraps around a prompt.
 * They are for the model, not for the reader.
 */
const HARNESS_TAG_BLOCK = /^\s*<(tools-used-before-summary|system-reminder)>[\s\S]*?<\/\1>\s*/;

/**
 * An agent's answer without the harness tags it starts with, and without the
 * space they leave. Only leading blocks of the two known tags go: a reply that
 * talks about XML, or quotes one of these tags mid-sentence, is left intact.
 */
export function stripHarnessMarkup(text: string): string {
  let out = text;
  while (HARNESS_TAG_BLOCK.test(out)) out = out.replace(HARNESS_TAG_BLOCK, "");
  return out;
}

/** The names a harness gives an agent whose call names no type: they say nothing on a row. */
const GENERIC_AGENT_NAME = new Set(["agent", "task", "default", "general-purpose"]);

/**
 * A subagent's row title: its agent type (`Explore`), or the task's first line
 * when the type is only the generic `Agent`/`Task` every call gets.
 */
export function subagentTitle(name: string, task: string): string {
  const type = name.trim();
  if (type && !GENERIC_AGENT_NAME.has(type.toLowerCase())) return type;
  const line = task
    .split("\n")
    .map((l) => l.trim())
    .find((l) => l.length > 0);
  return line || type || "Agent";
}

/**
 * A leading part of a shell line that only sets up where or with what it runs: `cd x`, `export A=1`, `A=1`.
 * A value is a run of quoted strings and unquoted characters, and every character can start only one
 * of those, so a long or unterminated value cannot make the match backtrack exponentially.
 */
const SETUP_PART =
  /^(?:cd\s+(?:"[^"]*"|'[^']*'|[^\s;&|]+)|(?:export\s+)?[A-Za-z_][A-Za-z0-9_]*=(?:"[^"]*"|'[^']*'|[^\s;&|"'])*(?:\s+[A-Za-z_][A-Za-z0-9_]*=(?:"[^"]*"|'[^']*'|[^\s;&|"'])*)*)\s*(?:&&|;|\n)\s*/;
/** Environment assignments can contain spaces and secrets; remove the entire quoted value first. */
const ENV_ASSIGNMENT = /(^|\s)[A-Za-z_][A-Za-z0-9_]*=(?:"(?:\\.|[^"\\])*"|'[^']*'|[^\s;&|"'])*/g;

/** The word a reader knows a program by: `cargo`, not `/usr/local/bin/cargo`. */
const programName = (token: string) => token.split("/").filter(Boolean).at(-1) ?? "";
const SAFE_SUBCOMMANDS: Record<string, ReadonlySet<string>> = {
  bun: new Set(["test", "run", "build", "check", "install", "add", "remove", "update", "x", "pm"]),
  cargo: new Set(["test", "build", "check", "clippy", "fmt", "run", "clean", "metadata", "tree", "doc"]),
  git: new Set([
    "status",
    "diff",
    "log",
    "show",
    "branch",
    "switch",
    "checkout",
    "worktree",
    "commit",
    "fetch",
    "pull",
    "push",
    "add",
    "restore",
  ]),
  npm: new Set(["test", "run", "build", "install", "ci", "update", "outdated", "exec"]),
  pnpm: new Set(["test", "run", "build", "install", "add", "remove", "update", "exec"]),
  yarn: new Set(["test", "run", "build", "install", "add", "remove", "up", "exec"]),
};
const SAFE_OPTION = /^--?[A-Za-z][A-Za-z0-9-]*$/;

/**
 * A shell line cut to its program and first argument (`grep -rnE …`), without what only sets it
 * up: a leading `cd <path> &&`, `export A=1;`, or `A=1` before the program. Empty when nothing a
 * reader would recognise is left (a line that only assigns, a variable for a program), so the row
 * says "Running a command" instead of a raw line such as `F=/private/tmp/…`.
 */
export function shortCommand(command: string): string {
  // A title may hold the line as code: `` `bun test` ``.
  let line = unwrapShell(command.trim().replace(/^`+|`+$/g, "")).trim();
  for (;;) {
    const next = line.replace(SETUP_PART, "");
    if (next === line) break;
    line = next;
  }
  line = line.replace(ENV_ASSIGNMENT, "$1").replace(/\s+/g, " ").trim();
  const env = /^(?:[^\s;&|]+\/)?env(?:\s+|$)([\s\S]*)$/.exec(line);
  // `env -S` embeds a whole shell command (often with inline secrets) in one argument. A shell
  // parser would be needed to safely extract its program, so keep this activity generic.
  if (env && /(?:^|\s)(?:-S|--split-string)(?:=|\s|$)/.test(env[1]!)) return "";
  // The first line is what runs first; a heredoc or a script below it is not a label.
  line = line.split("\n")[0]!.trim();
  const tokens = line.split(/\s+/).filter(Boolean);
  const assigns = () => {
    while (tokens.length && /^[A-Za-z_][A-Za-z0-9_]*=/.test(tokens[0]!)) tokens.shift();
  };
  assigns();
  if (tokens[0] === "env") {
    tokens.shift();
    assigns();
  }
  const name = programName(tokens[0] ?? "");
  if (name === "export") return "";
  // A program the line computes (`$CMD`, `"$(…)"`), or punctuation, names nothing.
  if (!/^[A-Za-z0-9_.+-]+$/.test(name)) return "";
  const raw = tokens[1]?.replace(/[;&|]+$/, "");
  // Keep only ordinary flags and known tool verbs; arbitrary arguments can be paths, URLs or secrets.
  const first = raw && (SAFE_OPTION.test(raw) || SAFE_SUBCOMMANDS[name]?.has(raw)) ? raw : undefined;
  const shown = first && first.length <= 24 ? first : undefined;
  const head = shown ? `${name} ${shown}` : name;
  return tokens.length > (shown ? 2 : 1) ? `${head} …` : head;
}

/**
 * A subagent's current step, short enough for its row: `Reading Composer.tsx`,
 * `Searching for focusComposer`, `Running bun test`. The full label lives in
 * the steps fold, where there is room for it.
 */
export function agentStepLabel(item: ToolItem): string {
  const input = item.input && typeof item.input === "object" && !Array.isArray(item.input) ? item.input : {};
  const field = (value: Json | undefined) => (typeof value === "string" ? value.trim() : "");
  const kind = item.toolKind?.toLowerCase() ?? "";
  const title = item.title.trim();
  if (kind === "execute" || /^(bash|shell)\b/i.test(title)) {
    const line = field(input.command) || item.summary?.trim() || title;
    // A tool name alone (`Bash`, `shell`) is no command.
    const command = /^(?:bash|shell|terminal|exec(?:ute)?)$/i.test(line) ? "" : shortCommand(line);
    return command ? `Running ${command}` : "Running a command";
  }
  if (kind === "read" || /^read\b/i.test(title)) {
    const path = field(input.file_path) || field(input.path) || /^read\s+(.+)$/i.exec(title)?.[1] || "";
    const file = path.split(/[\\/]/).filter(Boolean).at(-1);
    return file ? `Reading ${file}` : "Reading a file";
  }
  if (kind === "search" || /^(grep|glob|search|find)\b/i.test(title)) {
    const query = field(input.pattern) || field(input.query) || usefulSummary(item.summary);
    return query ? `Searching for ${clipLabel(query, 40)}` : "Searching the files";
  }
  return clipLabel(stepLabel(item), STEP_LABEL_MAX);
}

/** How long a row's one-line step may be before it is cut. */
const STEP_LABEL_MAX = 60;

/** `text` on one line, at most `max` long, its cut marked. */
function clipLabel(text: string, max: number): string {
  const line = text.replace(/\s+/g, " ").trim();
  return line.length > max ? `${line.slice(0, max - 1).trimEnd()}…` : line;
}

/**
 * The thread each `start_session` call started, by call id. A call that has not
 * answered yet is matched by the prompt it asked for: the link exists before
 * the answer does, so its row knows the thread while the thread still starts.
 * One link is one row: a start row takes the link that started that thread (a
 * follow-up message to it is not one), and never a link another call already shows.
 */
export function startedLinks(items: readonly TranscriptItem[], links: readonly Link[]): Map<string, Link> {
  const out = new Map<string, Link>();
  const claimed = new Set<string>();
  const starts = items.filter(
    (item): item is ToolItem => item.kind === "tool" && orchestrationTool(item.title) === "start_session",
  );
  // The calls that answered first, so a prompt match cannot take a link whose call already names it.
  for (const item of starts) {
    const target = sessionIdOf(item.output);
    if (target === undefined) continue;
    const link =
      links.find((l) => l.target === target && l.started && !claimed.has(l.id)) ??
      links.find((l) => l.target === target && !claimed.has(l.id));
    if (!link) continue;
    out.set(item.id, link);
    claimed.add(link.id);
  }
  for (const item of starts) {
    if (out.has(item.id)) continue;
    const input =
      item.input && typeof item.input === "object" && !Array.isArray(item.input) ? item.input : {};
    const prompt = typeof input.prompt === "string" ? input.prompt : "";
    if (!prompt) continue;
    // The newest unclaimed one: a prompt repeated later takes the later link.
    const link = [...links].reverse().find((l) => l.started && l.message === prompt && !claimed.has(l.id));
    if (!link) continue;
    out.set(item.id, link);
    claimed.add(link.id);
  }
  return out;
}

/** A step's one-line label: `Running <command>` for a shell, else its summary or title. */
export function stepLabel(item: ToolItem): string {
  const label = displayTool(item.title, item.input, item.status);
  const summary = usefulSummary(item.summary);
  if (item.toolKind === "execute" || /^(bash|shell)/i.test(item.title)) {
    return `Running ${summary ?? label.summary ?? label.title}`;
  }
  return summary ?? label.summary ?? label.title;
}

export function thoughtHeadings(text: string): string[] {
  return [...text.matchAll(/\*\*([^*\n]+)\*\*/g)].map((m) => m[1]!.trim()).filter(Boolean);
}

export function diffCounts(diffs: FileDiff[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const diff of diffs) {
    for (const line of diffLines(diff)) {
      if (line.kind === "added") added++;
      else if (line.kind === "removed") removed++;
    }
  }
  return { added, removed };
}

export interface DiffLine {
  kind: "context" | "added" | "removed";
  text: string;
  oldNumber?: number;
  newNumber?: number;
}

/** A line diff of a hunk's old and new text (LCS; hunks are small, and big ones fall back to remove+add). */
export function diffLines(diff: FileDiff): DiffLine[] {
  const oldLines = diff.oldText === undefined || diff.oldText === "" ? [] : diff.oldText.split("\n");
  const newLines = diff.newText === "" ? [] : diff.newText.split("\n");
  const n = oldLines.length;
  const m = newLines.length;
  const out: DiffLine[] = [];
  if (n * m > 400_000) {
    oldLines.forEach((text, i) => {
      out.push({ kind: "removed", text, oldNumber: diff.oldStart + i });
    });
    newLines.forEach((text, i) => {
      out.push({ kind: "added", text, newNumber: diff.newStart + i });
    });
    return out;
  }
  const lcs: Uint32Array[] = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i]![j] =
        oldLines[i] === newLines[j] ? lcs[i + 1]![j + 1]! + 1 : Math.max(lcs[i + 1]![j]!, lcs[i]![j + 1]!);
    }
  }
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && oldLines[i] === newLines[j]) {
      out.push({
        kind: "context",
        text: oldLines[i]!,
        oldNumber: diff.oldStart + i,
        newNumber: diff.newStart + j,
      });
      i++;
      j++;
    } else if (i < n && (j >= m || lcs[i + 1]![j]! >= lcs[i]![j + 1]!)) {
      out.push({ kind: "removed", text: oldLines[i]!, oldNumber: diff.oldStart + i });
      i++;
    } else {
      out.push({ kind: "added", text: newLines[j]!, newNumber: diff.newStart + j });
      j++;
    }
  }
  return out;
}
