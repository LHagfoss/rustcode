// What an approval would actually do: how risky it is, the key-value facts a
// person needs to decide (the command, the paths, the folder, the edit's size),
// which answers are a grant and for how long, and how an answered card reads as
// one line. Pure, so the cards and the inbox answer the same way and the rules
// are tested without a window.

import type { PermissionOption, PermissionRequest } from "../acp/types";
import { diffCounts } from "../state/thread";
import { displayCommand } from "./command";

/** What a call can do, worst first. Unknown calls are `risky`, never `safe`. */
export type ApprovalRisk = "safe" | "risky" | "destructive";

export interface RiskAssessment {
  level: ApprovalRisk;
  /** One or two words for the card's badge. */
  label: string;
  /** Why, when the input says it: "writes outside the project", "deletes a tree". */
  why?: string;
}

/** Tools that only look: they never make a request risky on their own. */
const READ_ONLY_TOOLS = new Set([
  "read_file",
  "view_file",
  "list_dir",
  "list_directory",
  "search",
  "grep",
  "glob",
  "find_files",
  "read_notebook",
  "image_search",
  "web_search",
  "think",
  "todo",
  "plan",
]);

/** Tool names that change something, so a read-only kind cannot make them safe. */
const WRITING_TOOLS = new Set([
  "write_to_file",
  "write_file_chunk",
  "replace_file_content",
  "multi_replace_file_content",
  "edit_file",
  "apply_patch",
  "copy_file",
  "move_file",
  "delete_file",
  "run_command",
  "shell",
  "bash",
]);

const READ_ONLY_KINDS = new Set(["read", "search", "think"]);

/** A command whose every segment is one of these does nothing but look. */
const READ_ONLY_COMMANDS = new Set([
  "ls",
  "cat",
  "head",
  "tail",
  "less",
  "more",
  "wc",
  "pwd",
  "echo",
  "which",
  "whoami",
  "date",
  "rg",
  "grep",
  "find",
  "fd",
  "file",
  "stat",
  "du",
  "df",
  "tree",
  "jq",
  "sort",
  "uniq",
  "cut",
  "diff",
  "true",
  "printenv",
  "basename",
  "dirname",
]);

/** `git` subcommands that only read. Anything else (`push`, `reset`, `clean`) is risky. */
const READ_ONLY_GIT = new Set([
  "status",
  "diff",
  "log",
  "show",
  "blame",
  "describe",
  "shortlog",
  "ls-files",
  "rev-parse",
  "grep",
]);

/** Flags that turn an otherwise read-only git subcommand into a change. */
const WRITING_AFTER: Record<string, string[]> = {
  branch: ["-d", "-D", "-m", "-M", "-f", "--delete", "--move", "--force", "--set-upstream-to"],
  remote: ["add", "remove", "rm", "set-url", "rename"],
  tag: ["-d", "-a", "-f", "--delete", "--annotate"],
  stash: ["push", "pop", "drop", "clear", "apply"],
};

const DESTRUCTIVE_COMMANDS: { why: string; test: (words: string[]) => boolean }[] = [
  {
    why: "deletes a tree",
    test: (words) => {
      if (words[0]?.replace(/^.*\//, "") !== "rm") return false;
      const flags = words.slice(1).filter((word) => word.startsWith("-"));
      return flags.some((flag) => /[rR]/.test(flag) || flag === "--recursive");
    },
  },
  { why: "forces a push", test: (words) => isGit(words, "push") && words.some(isForceFlag) },
  { why: "discards commits and changes", test: (words) => isGit(words, "reset") && words.includes("--hard") },
  {
    why: "deletes untracked files",
    test: (words) => isGit(words, "clean") && words.some((word) => /^-[a-z]*[fdx]/.test(word)),
  },
  {
    why: "discards changes",
    test: (words) => isGit(words, "checkout") && words.includes("--") && words.includes("."),
  },
  {
    why: "writes a disk",
    test: (words) =>
      ["mkfs", "dd"].some((name) => words[0]?.replace(/^.*\//, "") === name) ||
      (words[0] === "diskutil" && words[1] === "erase"),
  },
  {
    why: "stops the machine",
    test: (words) => ["shutdown", "reboot", "halt"].includes(words[0]?.replace(/^.*\//, "") ?? ""),
  },
  {
    why: "drops data",
    test: (words) => words.includes("drop") && words.some((w) => /^(table|database|schema)$/i.test(w)),
  },
  { why: "empties a table", test: (words) => words[0]?.toLowerCase() === "truncate" },
  {
    why: "opens every permission",
    test: (words) => words[0] === "chmod" && words.includes("-R") && words.includes("777"),
  },
  { why: "runs as root", test: (words) => words[0]?.replace(/^.*\//, "") === "sudo" },
  {
    why: "writes a raw disk",
    test: (words) => words.some((word) => word.startsWith("/dev/sd") || word.startsWith("/dev/disk")),
  },
];

/** Installing or reaching the network changes the machine or the world: never `safe`. */
const RISKY_COMMAND_PATTERNS: RegExp[] = [
  /\b(?:npm|pnpm|yarn|bun)\s+(?:add|install|i|remove|uninstall|publish|deploy)\b/,
  /\bpip3?\s+install\b/,
  /\bbrew\s+(?:install|uninstall|upgrade)\b/,
  /\bcargo\s+(?:add|install|publish)\b/,
  /\b(?:curl|wget)\b/,
  /\bgit\s+(?:push|fetch|pull|clone|submodule|worktree)\b/,
  /\bdocker\b/,
  /\b(?:ssh|scp|rsync)\b/,
];

/** Runners whose plain subcommands execute code the request did not show. */
const RUNNERS = new Set([
  "cargo",
  "bun",
  "npm",
  "pnpm",
  "yarn",
  "node",
  "deno",
  "python",
  "python3",
  "pip",
  "pip3",
  "go",
  "swift",
  "make",
  "ruby",
  "perl",
]);

/** Runner subcommands that only print facts about the project. */
const READ_ONLY_RUNNER_SUBCOMMANDS = new Set(["metadata", "help", "--help", "-h", "--version", "-V"]);

function isGit(words: string[], sub: string): boolean {
  return words[0]?.replace(/^.*\//, "") === "git" && words[1] === sub;
}

function isForceFlag(word: string): boolean {
  return word === "--force" || word === "--force-with-lease" || /^-[a-zA-Z]*f[a-zA-Z]*$/.test(word);
}

/** The command segments of one command line: pipelines and `&&`/`;` run separately. */
function segmentsOf(command: string): string[][] {
  return command
    .split(/\s*(?:&&|\|\||;|\||\n)\s*/)
    .map((segment) =>
      segment
        .trim()
        .split(/\s+/)
        .map((word) => word.replace(/^['"]+|['"]+$/g, ""))
        .filter(Boolean),
    )
    .filter((words) => words.length > 0);
}

function commandOf(request: PermissionRequest): string | undefined {
  const input = request.input;
  if (!input || typeof input !== "object" || Array.isArray(input)) return undefined;
  const record = input as Record<string, unknown>;
  const value = record.command ?? record.cmd;
  return typeof value === "string" ? displayCommand(value) : undefined;
}

/** The paths an input names: one path key, a list of them, or a diff's files. */
export function approvalPaths(request: PermissionRequest): string[] {
  const paths = new Set<string>();
  const input = request.input;
  if (input && typeof input === "object" && !Array.isArray(input)) {
    const record = input as Record<string, unknown>;
    for (const key of ["path", "file_path", "filePath", "target", "destination", "source"]) {
      const value = record[key];
      if (typeof value === "string" && value.trim()) paths.add(value.trim());
    }
    for (const key of ["paths", "files"]) {
      const list = record[key];
      if (Array.isArray(list)) {
        for (const item of list) if (typeof item === "string" && item.trim()) paths.add(item.trim());
      }
    }
  }
  for (const diff of request.diffs) paths.add(diff.path);
  return [...paths];
}

/** The working folder an input names, when the request carries one. */
export function approvalCwd(request: PermissionRequest): string | undefined {
  const input = request.input;
  if (!input || typeof input !== "object" || Array.isArray(input)) return undefined;
  const record = input as Record<string, unknown>;
  for (const key of ["cwd", "working_directory", "workdir", "directory"]) {
    const value = record[key];
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return undefined;
}

/** Whether a path leaves the thread's folder (absolute outside it, or climbing out). */
export function leavesFolder(path: string, cwd: string | undefined, project: string): boolean {
  if (path.startsWith("~")) return true;
  if (!path.startsWith("/")) return /(?:^|\/)\.\.(?:\/|$)/.test(path);
  const root = cwd ?? project;
  if (!root) return true;
  return !(path === root || path.startsWith(`${root.replace(/\/$/, "")}/`));
}

/** Paths a command writes: its redirects and `--output` targets. */
function commandWritePaths(command: string): string[] {
  const paths: string[] = [];
  for (const match of command.matchAll(/(?:>>?\s*|(?:^|\s)(?:-o|--output(?:=|\s+)))([~/\w][^\s;|&]*)/g)) {
    if (match[1] && !match[1].startsWith("&")) paths.push(match[1]);
  }
  return paths;
}

function classifyCommand(command: string, cwd: string | undefined, project: string): RiskAssessment {
  const segments = segmentsOf(command);
  if (/\b(?:curl|wget)\b[^|;&]*\|\s*(?:sudo\s+)?(?:sh|bash|zsh|python3?)\b/.test(command)) {
    return { level: "destructive", label: "Destructive", why: "runs a downloaded script" };
  }
  for (const words of segments) {
    const destructive = DESTRUCTIVE_COMMANDS.find((rule) => rule.test(words));
    if (destructive) return { level: "destructive", label: "Destructive", why: destructive.why };
  }
  const outside = commandWritePaths(command).filter((path) => leavesFolder(path, cwd, project));
  if (outside.length) {
    return {
      level: "destructive",
      label: "Destructive",
      why: `writes outside the project: ${outside[0]}`,
    };
  }
  if (commandWritePaths(command).length > 0) {
    return { level: "risky", label: "Writes a file", why: commandWritePaths(command)[0] };
  }
  if (RISKY_COMMAND_PATTERNS.some((pattern) => pattern.test(command.toLowerCase()))) {
    return { level: "risky", label: "Risky", why: "changes the machine or reaches the network" };
  }
  for (const words of segments) {
    if (!isPlainRead(words)) return { level: "risky", label: "Risky", why: "runs a command" };
  }
  return { level: "safe", label: "Read-only" };
}

/** Whether one command segment only reads. Anything this cannot name is not read-only. */
function isPlainRead(words: string[]): boolean {
  const name = words[0]!.replace(/^.*\//, "");
  if (RUNNERS.has(name)) {
    const sub = words[1] ?? "";
    return READ_ONLY_RUNNER_SUBCOMMANDS.has(sub);
  }
  if (name === "git") {
    const sub = words[1] ?? "";
    if (!READ_ONLY_GIT.has(sub)) return false;
    const writing = WRITING_AFTER[sub] ?? [];
    return !words
      .slice(2)
      .some((word) => writing.some((flag) => word === flag || word.startsWith(`${flag}=`)));
  }
  return READ_ONLY_COMMANDS.has(name);
}

/**
 * How risky a request is, from its tool, its kind and its input. Conservative:
 * only calls that are plainly read-only are `safe`, and anything unknown is
 * `risky`, so a new tool is never quieter than an old one.
 */
export function assessApproval(request: PermissionRequest, project = ""): RiskAssessment {
  if (request.question) return { level: "safe", label: "Question" };
  const tool = request.title.trim().split(/\s/)[0] ?? "";
  const command = commandOf(request);
  const cwd = approvalCwd(request);
  if (command) return classifyCommand(command, cwd, project);

  const kind = request.toolKind;
  const knownRead = READ_ONLY_TOOLS.has(tool) || (!!kind && READ_ONLY_KINDS.has(kind));
  const knownWrite = WRITING_TOOLS.has(tool) || kind === "edit" || kind === "delete" || kind === "move";
  const paths = approvalPaths(request);
  const outside = paths.find((path) => leavesFolder(path, cwd, project));
  if (outside && !knownRead) {
    return {
      level: "destructive",
      label: "Destructive",
      why: `writes outside the project: ${outside}`,
    };
  }
  if (knownWrite || request.diffs.length > 0) {
    return { level: "risky", label: "Changes files", why: paths[0] };
  }
  if (knownRead) return { level: "safe", label: "Read-only" };
  return { level: "risky", label: "Risky" };
}

/** One fact of the preview: its name at the left, its value in monospace or not. */
export interface PreviewRow {
  label: string;
  value: string;
  mono?: boolean;
  wrap?: boolean;
}

/** An edit's size, in the diff summary the transcript already draws: `+12 −3`, or `new file`. */
export function diffSummary(diffs: PermissionRequest["diffs"]): string {
  if (diffs.length === 1 && diffs[0]!.oldText === undefined) return "new file";
  const { added, removed } = diffCounts(diffs);
  if (added === 0 && removed === 0) return "no line change";
  return [added > 0 ? `+${added}` : "", removed > 0 ? `−${removed}` : ""].filter(Boolean).join(" ");
}

/** The key–value facts of a request: exactly what would run, on what, and where. */
export function approvalPreview(request: PermissionRequest, project = ""): PreviewRow[] {
  const rows: PreviewRow[] = [];
  const command = commandOf(request);
  if (command) rows.push({ label: "Command", value: command, mono: true, wrap: true });
  const cwd = approvalCwd(request);
  if (cwd) rows.push({ label: "Folder", value: cwd, mono: true, wrap: true });
  const paths = approvalPaths(request);
  if (paths.length) {
    rows.push({
      label: paths.length === 1 ? "Path" : "Paths",
      value: paths.join("\n"),
      mono: true,
      wrap: true,
    });
  }
  for (const diff of request.diffs) {
    rows.push({ label: "Edit", value: `${diff.path} · ${diffSummary([diff])}`, mono: true, wrap: true });
  }
  const outside = paths.find((path) => leavesFolder(path, cwd, project));
  if (project && outside) rows.push({ label: "Note", value: "Outside the thread's folder." });
  return rows;
}

/** Which answer a grant offers: the engine's `allow_hour` sits between once and always. */
export type GrantScope = "once" | "hour" | "always" | "decline" | "other";

export function optionScope(option: PermissionOption): GrantScope {
  const id = option.id.toLowerCase();
  const name = option.name.toLowerCase();
  if (option.kind === "reject_once" || option.kind === "reject_always") return "decline";
  if (id === "allow_hour" || id.includes("hour") || name.includes("1 hour")) return "hour";
  if (option.kind === "allow_once") return "once";
  if (option.kind === "allow_always") return "always";
  return "other";
}

/** What a scope says in a line: "once", "1 hour", "always". */
export function scopeLabel(scope: GrantScope): string {
  if (scope === "hour") return "1 hour";
  if (scope === "always") return "always";
  if (scope === "once") return "once";
  return "";
}

/** What a call is, in a few words: the command's first line, or the tool and its path. */
export function approvalSubject(request: PermissionRequest): string {
  const command = commandOf(request);
  if (command) return command.split("\n")[0]!.trim();
  const paths = approvalPaths(request);
  if (paths.length === 1) return `${request.title.split(/\s/)[0]} ${paths[0]}`;
  return displayCommand(request.title);
}

/**
 * How an answered approval reads as one settled line: `Allowed edit
 * src/app.ts · once`, `Declined git push --force`. A rejection names no scope.
 */
export function resolvedLine(request: PermissionRequest, option: PermissionOption): string {
  const subject = approvalSubject(request);
  const scope = optionScope(option);
  if (scope === "decline") return `Declined ${subject}`;
  const verb = scope === "hour" || scope === "always" ? "Allowed" : "Approved";
  const label = scopeLabel(scope);
  return label ? `${verb} ${subject} · ${label}` : `${verb} ${subject}`;
}
