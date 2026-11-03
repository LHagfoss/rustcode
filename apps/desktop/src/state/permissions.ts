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

/** Tool kinds "Auto-edit" lets through: reading and editing files, not running commands. */
const EDIT_KINDS = new Set(["read", "edit", "move", "search", "think", "fetch"]);

/**
 * The option to answer `request` with under `mode`, or null to ask the user.
 * Questions always reach the user, and so does a plan's "switch mode" request:
 * that one is the user's review of the plan, not a safety prompt.
 */
export function autoAnswer(request: PermissionRequest, mode: PermissionMode): PermissionOption | null {
  if (mode === "ask" || request.question || request.toolKind === "switch_mode") return null;
  if (mode === "edits" && !EDIT_KINDS.has(request.toolKind ?? "")) return null;
  return allowOption(request);
}

export function allowOption(request: PermissionRequest): PermissionOption | null {
  return (
    request.options.find((o) => o.kind === "allow_once") ??
    request.options.find((o) => o.kind === "allow_always") ??
    null
  );
}
