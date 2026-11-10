import type { PermissionOption, PermissionRequest } from "../acp/types";

/**
 * How much a thread may do without asking, chosen under the composer.
 * The app answers the harness's approval requests itself, so one setting
 * works the same on every harness.
 */
export type PermissionMode = "ask" | "edits" | "full";

export const PERMISSION_MODES: { id: PermissionMode; label: string; detail: string }[] = [
  { id: "ask", label: "Ask", detail: "Ask before commands and edits" },
  { id: "edits", label: "Auto-edit", detail: "Edit files without asking; ask before commands" },
  { id: "full", label: "Full access", detail: "Run everything without asking" },
];

export function parsePermissionMode(value: unknown): PermissionMode {
  return value === "edits" || value === "full" ? value : "ask";
}

/** How much each mode allows, narrowest first. */
const RANK: Record<PermissionMode, number> = { ask: 0, edits: 1, full: 2 };

/**
 * What a delegated thread may do without asking: `requested`, never broader than
 * the thread that started it. Starting a thread hands work somewhere the agent
 * cannot see, so it may not hand out more than its own limit (`docs/orchestrator.md`).
 *
 * Only the app makes this call. A person raising a child in that child's own
 * composer (`setPermissionMode`) is the user's own choice and is not capped.
 */
export function effectiveMode(parent: PermissionMode, requested: PermissionMode): PermissionMode {
  return RANK[requested] <= RANK[parent] ? requested : parent;
}

/** Tool kinds "Auto-edit" lets through: reading and editing files, not running commands. */
const EDIT_KINDS = new Set(["read", "edit", "move", "search", "think", "fetch"]);

/**
 * Whether a thread at `mode` may delegate without a prompt. Starting or messaging
 * another thread runs work somewhere else, so it needs Full access; reading one
 * (or waiting for an answer) is part of normal reading, so Auto-edit has it too.
 */
export function delegationAllowed(
  mode: PermissionMode,
  kind:
    | "read"
    | "start"
    | "message"
    | "fan_out"
    | "organize"
    | "interrupt"
    | "transfer"
    | "configure"
    | "merge"
    | "cleanup"
    | "project"
    | "schedule",
): boolean {
  if (mode === "full") return true;
  return mode === "edits" && (kind === "read" || kind === "organize");
}

/** How a mode reads in a sentence: "allowed by Full access". */
export function modeLabel(mode: PermissionMode): string {
  return PERMISSION_MODES.find((m) => m.id === mode)?.label ?? "Ask";
}

/**
 * The option to answer `request` with under `mode`, or null to ask the user.
 * Questions always reach the user, and so does a plan's "switch mode" request:
 * that one is the user's review of the plan, not a safety prompt.
 */
export function autoAnswer(request: PermissionRequest, mode: PermissionMode): PermissionOption | null {
  if (mode === "ask" || request.question || request.toolKind === "switch_mode") return null;
  if (mode === "edits" && !EDIT_KINDS.has(kindOf(request))) return null;
  return allowOption(request);
}

/** RustCode's built-in tools ask without an ACP kind; their names say it. */
const BUILTIN_KINDS: Record<string, string> = {
  replace_file_content: "edit",
  multi_replace_file_content: "edit",
  write_to_file: "edit",
  write_file_chunk: "edit",
  copy_file: "edit",
  move_file: "move",
  view_file: "read",
  delete_file: "delete",
  run_command: "execute",
};

function kindOf(request: PermissionRequest): string {
  if (request.toolKind && request.toolKind !== "other") return request.toolKind;
  return BUILTIN_KINDS[request.title.split(/\s/)[0] ?? ""] ?? "other";
}

export function allowOption(request: PermissionRequest): PermissionOption | null {
  return (
    request.options.find((o) => o.kind === "allow_once") ??
    request.options.find((o) => o.kind === "allow_always") ??
    null
  );
}
