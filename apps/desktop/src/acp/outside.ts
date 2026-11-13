// Settings › Outside agents (docs/mcp-outside-agents.md): other apps (Claude
// Desktop, Cursor, Codex) that use RustCode's MCP tools with a token of their
// own. The engine owns the grants, the scope checks and the audit; these are
// the requests the page makes and the shapes it reads.

import { arr, bool, type Json, type JsonObject, num, obj, str } from "./jsonrpc";

export const OutsideMethod = {
  list: "_rustcode/outside/list",
  create: "_rustcode/outside/create",
  update: "_rustcode/outside/update",
  revoke: "_rustcode/outside/revoke",
  answer: "_rustcode/outside/answer",
} as const;

export type OutsideScope = "read_only" | "delegate" | "manage";
export type OutsidePlatform = "claude-desktop" | "cursor" | "codex" | "other";

export const OUTSIDE_SCOPES: readonly OutsideScope[] = ["read_only", "delegate", "manage"];
export const OUTSIDE_PLATFORMS: readonly OutsidePlatform[] = ["claude-desktop", "cursor", "codex", "other"];

export const SCOPE_LABEL: Record<OutsideScope, string> = {
  read_only: "Read-only",
  delegate: "Delegate",
  manage: "Manage",
};

export const SCOPE_HELP: Record<OutsideScope, string> = {
  read_only: "Read threads, usage, harnesses and machines. Starts and changes nothing.",
  delegate:
    "Read, and start or message threads. Threads it starts run at Auto-edit: files can change, a command that needs approval is declined.",
  manage:
    "Delegate, and the management tools (merge, clean up, settings) as RustCode makes them available to outside agents. Started threads may run at Full access.",
};

export const PLATFORM_LABEL: Record<OutsidePlatform, string> = {
  "claude-desktop": "Claude Desktop",
  cursor: "Cursor",
  codex: "Codex",
  other: "Another app",
};

export interface OutsideGrant {
  id: string;
  name: string;
  platform: OutsidePlatform;
  scope: OutsideScope;
  /** Ask in the app before each call that changes something. */
  ask: boolean;
  createdAt: string;
  lastUsedAt: string | null;
  lastTool: string | null;
}

export interface OutsidePending {
  id: string;
  clientId: string;
  clientName: string;
  tool: string;
  summary: string;
  askedAt: string;
}

export interface OutsideCall {
  at: string;
  clientId: string;
  clientName: string;
  scope: OutsideScope;
  tool: string;
  outcome: string;
  durationMs: number | null;
  detail: string | null;
}

export interface OutsideStatus {
  grants: OutsideGrant[];
  pending: OutsidePending[];
  recent: OutsideCall[];
  command: string;
  maxGrants: number;
  /** False where the engine has no local socket for the bridge (Windows). */
  available: boolean;
}

export interface OutsideSnippet {
  format: "json" | "toml";
  where: string;
  text: string;
}

/** What `create` answers: the token and the config, shown once. */
export interface OutsideCreated {
  grant: OutsideGrant;
  token: string;
  snippet: OutsideSnippet;
}

function scopeOf(value: unknown): OutsideScope {
  return OUTSIDE_SCOPES.find((scope) => scope === value) ?? "read_only";
}

function platformOf(value: unknown): OutsidePlatform {
  return OUTSIDE_PLATFORMS.find((platform) => platform === value) ?? "other";
}

export function parseGrant(value: Json | undefined): OutsideGrant | null {
  const grant = obj(value);
  const id = str(grant?.id);
  if (!grant || !id) return null;
  return {
    id,
    name: str(grant.name) ?? id,
    platform: platformOf(grant.platform),
    scope: scopeOf(grant.scope),
    ask: bool(grant.ask) ?? true,
    createdAt: str(grant.createdAt) ?? "",
    lastUsedAt: str(grant.lastUsedAt) ?? null,
    lastTool: str(grant.lastTool) ?? null,
  };
}

export function parseOutsideStatus(value: Json): OutsideStatus {
  const root = obj(value) ?? {};
  const pending: OutsidePending[] = [];
  for (const entry of arr(root.pending)) {
    const item = obj(entry);
    const id = str(item?.id);
    if (!item || !id) continue;
    pending.push({
      id,
      clientId: str(item.clientId) ?? "",
      clientName: str(item.clientName) ?? "An app",
      tool: str(item.tool) ?? "",
      summary: str(item.summary) ?? str(item.tool) ?? "",
      askedAt: str(item.askedAt) ?? "",
    });
  }
  const recent: OutsideCall[] = [];
  for (const entry of arr(root.recent)) {
    const call = obj(entry);
    if (!call) continue;
    recent.push({
      at: str(call.at) ?? "",
      clientId: str(call.clientId) ?? "",
      clientName: str(call.clientName) ?? "",
      scope: scopeOf(call.scope),
      tool: str(call.tool) ?? "",
      outcome: str(call.outcome) ?? "",
      durationMs: num(call.durationMs) ?? null,
      detail: str(call.detail) ?? null,
    });
  }
  return {
    grants: arr(root.grants).flatMap((entry) => {
      const grant = parseGrant(entry);
      return grant ? [grant] : [];
    }),
    pending,
    recent,
    command: str(root.command) ?? "rustcode",
    maxGrants: num(root.maxGrants) ?? 20,
    available: bool(root.available) ?? true,
  };
}

export function parseCreated(value: Json): OutsideCreated | null {
  const root = obj(value);
  const grant = parseGrant(root?.grant);
  const token = str(root?.token);
  const snippet = obj(root?.snippet);
  if (!grant || !token || !snippet) return null;
  return {
    grant,
    token,
    snippet: {
      format: str(snippet.format) === "toml" ? "toml" : "json",
      where: str(snippet.where) ?? "",
      text: str(snippet.text) ?? "",
    },
  };
}

export function createParams(input: {
  name: string;
  platform: OutsidePlatform;
  scope: OutsideScope;
  ask: boolean;
}): JsonObject {
  return { name: input.name.trim(), platform: input.platform, scope: input.scope, ask: input.ask };
}

/** "never" or how long ago, for the Last used column. */
export function lastUsedLabel(lastUsedAt: string | null, now = Date.now()): string {
  if (!lastUsedAt) return "Not used yet";
  const then = Date.parse(lastUsedAt);
  if (Number.isNaN(then)) return "Not used yet";
  const seconds = Math.max(0, Math.round((now - then) / 1000));
  if (seconds < 60) return "Used just now";
  if (seconds < 3600) return `Used ${Math.round(seconds / 60)} min ago`;
  if (seconds < 86_400) return `Used ${Math.round(seconds / 3600)} h ago`;
  return `Used ${Math.round(seconds / 86_400)} d ago`;
}

/** How a call ended, in words. */
export function outcomeLabel(outcome: string): string {
  switch (outcome) {
    case "ok":
      return "Done";
    case "denied":
      return "Refused (access)";
    case "declined":
      return "Declined";
    case "unavailable":
      return "Not available";
    case "error":
      return "Failed";
    default:
      return outcome;
  }
}
