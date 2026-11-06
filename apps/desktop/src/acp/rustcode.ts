// RustCode extension answers the app reads besides the session: subagents
// (`_rustcode/agents`), pull requests (`_rustcode/pulls*`) and plan usage
// limits (`_rustcode/usage/limits`). Ported from RustCodeKit (Subagents.swift,
// PullRequests.swift, PlanUsage.swift).

import { arr, bool, type Json, num, obj, str } from "./jsonrpc";

// ---------------------------------------------------------------- subagents

export type AgentStatus =
  | "queued"
  | "running"
  | "cancelling"
  | "interrupted"
  | "completed"
  | "failed"
  | "cancelled"
  | "unknown";

export interface SubagentInfo {
  id: number;
  name: string;
  task: string;
  model?: string;
  agentType: string;
  status: AgentStatus;
  activeTurn: boolean;
  parentId?: number;
  depth: number;
  writeAccess: boolean;
  startedAt?: number;
  finishedAt?: number;
  lastActivity?: number;
  completion?: string;
  toolCalls: number;
  requests: number;
  inputTokens: number;
  outputTokens: number;
  cachedInputTokens: number;
  verificationCommand?: string;
  workspaceMode?: string;
  workspacePath?: string;
}

const STATUSES: AgentStatus[] = [
  "queued",
  "running",
  "cancelling",
  "interrupted",
  "completed",
  "failed",
  "cancelled",
];

export function parseAgent(value: Json | undefined): SubagentInfo | null {
  const o = obj(value);
  const id = num(o?.id);
  const name = str(o?.name);
  if (!o || id === undefined || !name) return null;
  const status = str(o.status) as AgentStatus | undefined;
  return {
    id,
    name,
    task: str(o.task) ?? "",
    model: str(o.model),
    agentType: str(o.agentType) ?? "default",
    status: status && STATUSES.includes(status) ? status : "unknown",
    activeTurn: bool(o.activeTurn) ?? false,
    parentId: num(o.parentId),
    depth: num(o.depth) ?? 1,
    writeAccess: bool(o.writeAccess) ?? false,
    startedAt: num(o.startedAtMs),
    finishedAt: num(o.finishedAtMs),
    lastActivity: num(o.lastActivityMs),
    completion: str(o.completion),
    toolCalls: num(o.toolCalls) ?? 0,
    requests: num(o.requests) ?? 0,
    inputTokens: num(o.inputTokens) ?? 0,
    outputTokens: num(o.outputTokens) ?? 0,
    cachedInputTokens: num(o.cachedInputTokens) ?? 0,
    verificationCommand: str(o.verificationCommand) || undefined,
    workspaceMode: str(o.workspaceMode),
    workspacePath: str(o.workspacePath),
  };
}

export function parseAgentList(result: Json | undefined): SubagentInfo[] {
  return arr(obj(result)?.agents).flatMap((a) => {
    const agent = parseAgent(a);
    return agent ? [agent] : [];
  });
}

export const agentActive = (a: SubagentInfo) =>
  a.activeTurn || a.status === "queued" || a.status === "running" || a.status === "cancelling";

export const AGENT_STATUS_LABEL: Record<AgentStatus, string> = {
  queued: "Queued",
  running: "Running",
  cancelling: "Cancelling…",
  interrupted: "Interrupted",
  completed: "Done",
  failed: "Failed",
  cancelled: "Cancelled",
  unknown: "Unknown",
};

/** The child a `spawn_agent` result names: its JSON receipt, or the older `subagent 3 started` line. */
export function spawnedAgentId(output: string | undefined): number | undefined {
  if (!output) return undefined;
  const m =
    /^\s*(?:spawn_agent:\s*)?\{[^}]*"agent_id"\s*:\s*(\d+)/.exec(output) ??
    /^\s*(?:spawn_agent:\s*)?subagent (\d+) started/.exec(output);
  return m ? Number(m[1]) : undefined;
}

/** `950`, `12.4k`, `1.2M`. */
export function compactCount(n: number): string {
  if (n < 1000) return String(n);
  if (n < 10_000) return `${(n / 1000).toFixed(1)}k`;
  if (n < 1_000_000) return `${Math.floor(n / 1000)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

/** How long the child ran, or has run so far, as `1m 05s`. */
export function agentElapsed(a: SubagentInfo, now = Date.now()): string | undefined {
  if (!a.startedAt) return undefined;
  const s = Math.max(0, Math.floor(((a.finishedAt ?? now) - a.startedAt) / 1000));
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;
}

// ---------------------------------------------------------------- pull requests

export interface PullChecks {
  passed: number;
  failed: number;
  pending: number;
  skipped: number;
}

export interface PullRequest {
  number: number;
  title: string;
  author: string;
  state: "open" | "closed" | "merged";
  draft: boolean;
  review?: string;
  checks: PullChecks;
  headRef: string;
  baseRef: string;
  headSha: string;
  updatedAt?: number;
  url?: string;
  additions: number;
  deletions: number;
  changedFiles: number;
}

export interface PullDetail extends PullRequest {
  body: string;
  checkRuns: { name: string; status: string; url?: string }[];
  files: { path: string; additions: number; deletions: number }[];
  activity: { author: string; kind: string; at?: number; body: string }[];
}

export interface PullError {
  code: string;
  message: string;
}

export function parsePull(value: Json | undefined): PullRequest | null {
  const o = obj(value);
  const number = num(o?.number);
  if (!o || number === undefined) return null;
  const state = str(o.state);
  const c = obj(o.checks);
  const updated = str(o.updatedAt);
  return {
    number,
    title: str(o.title) ?? "",
    author: str(o.author) ?? "",
    state: state === "closed" || state === "merged" ? state : "open",
    draft: bool(o.draft) ?? false,
    review: str(o.review),
    checks: {
      passed: num(c?.passed) ?? 0,
      failed: num(c?.failed) ?? 0,
      pending: num(c?.pending) ?? 0,
      skipped: num(c?.skipped) ?? 0,
    },
    headRef: str(o.headRef) ?? "",
    baseRef: str(o.baseRef) ?? "",
    headSha: str(o.headSha) ?? "",
    updatedAt: updated ? Date.parse(updated) || undefined : undefined,
    url: str(o.url),
    additions: num(o.additions) ?? 0,
    deletions: num(o.deletions) ?? 0,
    changedFiles: num(o.changedFiles) ?? 0,
  };
}

export function parsePullError(result: Json | undefined): PullError | null {
  const e = obj(obj(result)?.error);
  const code = str(e?.code);
  return code ? { code, message: str(e?.message) ?? "" } : null;
}

export function parsePullList(result: Json | undefined): { pulls: PullRequest[]; hasMore: boolean } {
  const list = obj(obj(result)?.list);
  return {
    pulls: arr(list?.pulls).flatMap((p) => {
      const pull = parsePull(p);
      return pull ? [pull] : [];
    }),
    hasMore: bool(list?.hasMore) ?? false,
  };
}

export function parsePullDetail(result: Json | undefined): PullDetail | null {
  const p = obj(obj(result)?.pull);
  const pull = parsePull(p);
  if (!pull || !p) return null;
  return {
    ...pull,
    body: str(p.body) ?? "",
    checkRuns: arr(p.checkRuns).map((c) => ({
      name: str(obj(c)?.name) ?? "",
      status: str(obj(c)?.status) ?? "pending",
      url: str(obj(c)?.url),
    })),
    files: arr(p.files).map((f) => ({
      path: str(obj(f)?.path) ?? "",
      additions: num(obj(f)?.additions) ?? 0,
      deletions: num(obj(f)?.deletions) ?? 0,
    })),
    activity: arr(p.activity).map((a) => {
      const at = str(obj(a)?.at);
      return {
        author: str(obj(a)?.author) ?? "",
        kind: str(obj(a)?.kind) ?? "",
        at: at ? Date.parse(at) || undefined : undefined,
        body: str(obj(a)?.body) ?? "",
      };
    }),
  };
}

/** `owner/name` from a pull request's GitHub URL. */
export function repositoryOf(url: string | undefined): string | undefined {
  const m = url ? /^https:\/\/github\.com\/([^/]+\/[^/]+)\/pull\/\d+/.exec(url) : null;
  return m?.[1];
}

export function pullErrorTitle(code: string): string {
  switch (code) {
    case "gh_missing":
      return "The GitHub CLI is not installed";
    case "not_connected":
      return "GitHub is not connected";
    case "not_a_repository":
      return "This folder has no GitHub repository";
    case "timed_out":
      return "GitHub did not answer in time";
    default:
      return "Pull requests could not be read";
  }
}

// ---------------------------------------------------------------- plan usage

export interface UsageWindow {
  usedPercent: number;
  minutes?: number;
  resetsAt?: number;
}

export interface PlanUsage {
  source: string;
  windows: UsageWindow[];
  /** When the client last got these numbers (ms). */
  fetchedAt?: number;
  /** The last refresh failed; these are the numbers from before it. */
  stale?: boolean;
}

export function parsePlanUsage(result: Json | undefined): PlanUsage | null {
  const o = obj(result);
  if (!o) return null;
  const windows = (["primary", "secondary"] as const).flatMap((key) => {
    const w = obj(o[key]);
    const used = num(w?.usedPercent);
    if (used === undefined) return [];
    const resets = num(w?.resetsAt);
    return [
      { usedPercent: used, minutes: num(w?.windowMinutes), resetsAt: resets ? resets * 1000 : undefined },
    ];
  });
  return windows.length ? { source: str(o.source) ?? "provider", windows } : null;
}

/**
 * One subscription in `_rustcode/usage/plans` (Settings › Usage): a harness
 * this machine is signed in to, with its windows when they could be read.
 */
export type PlanStatus = "ok" | "signedOut" | "notInstalled" | "unavailable" | "notAsked";

/**
 * Why a plan could not be read, as the engine names it (`_rustcode/usage/plans`
 * `reason`). It decides what the app does next:
 *
 * - `expired` and `refused` are fixed in the CLI itself (Claude Code renews an
 *   expired sign-in, or the user signs in again), so the background read keeps
 *   asking and Settings › Usage offers a forced Refresh.
 * - `forbidden` is Anthropic refusing this sign-in access to the usage endpoint:
 *   renewing it does not change that, so it is not retried on its own.
 * - `denied` is a macOS Keychain prompt the user declined: asking again on every
 *   poll would just prompt again, so only an explicit look retries it.
 * - `transient` is no answer or an unreadable one, which the next poll tries again.
 */
export type PlanFailure = "expired" | "refused" | "forbidden" | "denied" | "transient";

export interface UsagePlan {
  /** The subscription's own id (`claude`, `codex`); see `planHarness`. */
  id: string;
  name: string;
  /** The CLI it was read from ("Claude Code"). */
  via?: string;
  /** The harness it belongs to; names an account's row apart from the default one. */
  harness?: string;
  /** The named account it was read from, null or absent for the default login. */
  account?: string | null;
  status: PlanStatus;
  /** The tier the provider named ("max"). */
  plan?: string;
  error?: string;
  /** Why an `unavailable` row has no numbers; absent from engines before this. */
  reason?: PlanFailure;
  windows: (UsageWindow & { label: string })[];
}

/** The harness a plan belongs to: a subscription's id is not always the harness id. */
const PLAN_HARNESS: Record<string, string> = { claude: "claude-code" };

export function planHarness(id: string): string {
  return PLAN_HARNESS[id] ?? id;
}

/** The harness a plan row belongs to: the engine names it, older ones only by id. */
export function planOf(p: UsagePlan): string {
  return p.harness ?? planHarness(p.id);
}

/**
 * The cache key of a plan row: the harness for its default login, and the
 * account's id for a named one, so two accounts never share numbers.
 */
export function planKey(p: UsagePlan): string {
  return p.account ? `${planOf(p)}#${p.account}` : planOf(p);
}

/** The cache key of a harness's login, with the account when the thread uses one. */
export function harnessPlanKey(harness: string, account?: string | null): string {
  return account ? `${harness}#${account}` : harness;
}

export function parsePlans(result: Json | undefined): UsagePlan[] {
  return arr(obj(result)?.plans).flatMap((p) => {
    const o = obj(p);
    const id = str(o?.id);
    const status = str(o?.status);
    if (!o || !id || !status) return [];
    const windows = arr(o.windows).flatMap((w) => {
      const x = obj(w);
      const used = num(x?.usedPercent);
      if (used === undefined) return [];
      const resets = num(x?.resetsAt);
      return [
        {
          label: str(x?.label) ?? "Usage",
          usedPercent: used,
          minutes: num(x?.windowMinutes),
          resetsAt: resets ? resets * 1000 : undefined,
        },
      ];
    });
    return [
      {
        id,
        name: str(o.name) ?? id,
        via: str(o.via),
        harness: str(o.harness),
        account: str(o.account),
        status: status as PlanStatus,
        plan: str(o.plan),
        error: str(o.error),
        reason: planFailure(o.reason),
        windows,
      },
    ];
  });
}

/** The engine's failure reason, when this app knows it. */
const PLAN_FAILURES: readonly PlanFailure[] = ["expired", "refused", "forbidden", "denied", "transient"];

function planFailure(value: unknown): PlanFailure | undefined {
  const reason = str(value);
  return PLAN_FAILURES.find((known) => known === reason);
}

export function windowTitle(w: UsageWindow): string {
  const m = w.minutes;
  if (m === undefined) return "Usage";
  if (m === 10080) return "Weekly";
  if (m % 1440 === 0) return `${m / 1440}-day`;
  if (m % 60 === 0) return `${m / 60}-hour`;
  return `${m}-minute`;
}

export const leftPercent = (w: UsageWindow) => Math.max(0, Math.min(100, Math.round(100 - w.usedPercent)));

/** The shortest window, which limits work in the moment. */
export function shownWindow(usage: PlanUsage): UsageWindow | undefined {
  return [...usage.windows].sort((a, b) => (a.minutes ?? Infinity) - (b.minutes ?? Infinity))[0];
}

/** "resets in 2 h 14 min", or the clock time when it is more than a day away. */
export function resetLabel(w: UsageWindow, now = Date.now()): string | undefined {
  if (!w.resetsAt) return undefined;
  const ms = w.resetsAt - now;
  if (ms <= 0) return "resetting now";
  const minutes = Math.ceil(ms / 60_000);
  if (minutes < 60) return `resets in ${minutes} min`;
  if (minutes < 24 * 60) {
    const h = Math.floor(minutes / 60);
    const m = minutes % 60;
    return `resets in ${h} h${m ? ` ${m} min` : ""}`;
  }
  const day = new Date(w.resetsAt).toLocaleString(undefined, {
    weekday: "short",
    hour: "2-digit",
    minute: "2-digit",
  });
  return `resets ${day}`;
}

/** How often an idle app asks for fresh plan usage. */
export const PLAN_REFRESH_MS = 5 * 60_000;

/**
 * When to ask again: on the regular interval, or a few seconds after the
 * soonest window resets if that comes first, and never sooner than 5 s.
 */
export function nextPlanRefresh(plan: PlanUsage | null | undefined, now = Date.now()): number {
  const resets = (plan?.windows ?? []).flatMap((w) => (w.resetsAt && w.resetsAt > now ? [w.resetsAt] : []));
  const afterReset = resets.length ? Math.min(...resets) + 5_000 - now : Infinity;
  return Math.max(5_000, Math.min(PLAN_REFRESH_MS, afterReset));
}

// ---------------------------------------------------------------- recorded usage

/** Tokens one row holds. A count a harness did not report is absent, never 0. */
export interface UsageCounts {
  input?: number;
  output?: number;
  cached?: number;
  cacheWrite?: number;
}

/** A group's tokens, the USD a harness reported, and how many turns reported nothing. */
export interface UsageTotals extends UsageCounts {
  /** The USD a harness reported. Never a price RustCode computed. */
  cost?: number;
  turns: number;
  /** Turns with neither input nor output reported (the older, narrower count). */
  turnsWithoutTokens: number;
  /** Turns missing at least one of the four token counts. */
  turnsWithoutAnyTokens: number;
  turnsWithoutInputTokens: number;
  turnsWithoutOutputTokens: number;
  turnsWithoutCachedTokens: number;
  turnsWithoutCacheWriteTokens: number;
  turnsWithoutCost: number;
}

/** One row of `_rustcode/data/usage`, with the key its `group_by` chose. */
export interface UsageRow extends UsageTotals {
  harness: string;
  model?: string;
  thread?: string;
  title?: string;
  project?: string;
  day?: string;
  /** The thread that started this one (`threads.parent_id`). */
  parentId?: string;
  /** A parent known only by title, for a thread an older release started. */
  parentTitle?: string;
}

export interface UsageReport {
  groupBy: string;
  rows: UsageRow[];
  totals: UsageTotals;
  /** The answer stopped at the store's row cap, so its totals cover only what it returned. */
  truncated: boolean;
  /**
   * Thread rows carry their parent from the store, so the app need not join
   * them to a session list that may be capped or newer.
   */
  parentsReported: boolean;
}

function parseCounts(o: { [key: string]: Json } | undefined): UsageCounts {
  return {
    input: num(o?.input_tokens),
    output: num(o?.output_tokens),
    cached: num(o?.cached_tokens),
    cacheWrite: num(o?.cache_write_tokens),
  };
}

function parseTotals(o: { [key: string]: Json } | undefined): UsageTotals {
  return {
    ...parseCounts(o),
    cost: num(o?.cost_usd),
    turns: num(o?.turns) ?? 0,
    turnsWithoutTokens: num(o?.turns_without_tokens) ?? 0,
    turnsWithoutAnyTokens: num(o?.turns_without_any_tokens) ?? 0,
    turnsWithoutInputTokens: num(o?.turns_without_input_tokens) ?? 0,
    turnsWithoutOutputTokens: num(o?.turns_without_output_tokens) ?? 0,
    turnsWithoutCachedTokens: num(o?.turns_without_cached_tokens) ?? 0,
    turnsWithoutCacheWriteTokens: num(o?.turns_without_cache_write_tokens) ?? 0,
    turnsWithoutCost: num(o?.turns_without_cost) ?? 0,
  };
}

/**
 * `_rustcode/data/usage` (`engine/src/acp/data.rs`). The engine groups by one
 * key at a time; a null token or cost stays absent so the page can say so.
 */
export function parseUsageReport(result: Json | undefined): UsageReport {
  const root = obj(result);
  const raw = arr(root?.rows);
  return {
    groupBy: str(root?.group_by) ?? "",
    rows: raw.flatMap((entry) => {
      const o = obj(entry);
      if (!o) return [];
      return [
        {
          harness: str(o.harness) ?? "unknown",
          model: str(o.model) || undefined,
          thread: str(o.thread) || undefined,
          title: str(o.title) || undefined,
          project: str(o.project) || undefined,
          day: str(o.day) || undefined,
          parentId: str(o.parent_id) || undefined,
          parentTitle: str(o.parent_title) || undefined,
          ...parseTotals(o),
        },
      ];
    }),
    totals: parseTotals(obj(root?.totals)),
    truncated: root?.truncated === true,
    // A thread row always carries these keys, as null when it has no parent;
    // an older engine that does not know them has neither key.
    parentsReported: raw.some((entry) => {
      const row = obj(entry);
      return row?.parent_id !== undefined || row?.parent_title !== undefined;
    }),
  };
}

// ---------------------------------------------------------------- payload capture

/**
 * Whether the engine keeps the bodies it records (`docs/data-store.md` ›
 * Captured payloads), from `_rustcode/config/telemetry`
 * (`engine/src/acp/telemetry.rs`). Both are off by default and global: a
 * checked-out project cannot turn them on. The engine reads them when it
 * starts, so Settings offers a restart after a change.
 */
export interface PayloadCapture {
  /** `capture_provider_payloads`: each built-in harness request body. */
  providerPayloads: boolean;
  /** `capture_harness_rpc_payloads`: each message exchanged with a harness. */
  harnessRpcPayloads: boolean;
}

export function parsePayloadCapture(result: Json | undefined): PayloadCapture {
  const root = obj(result);
  // An older engine answers nothing, which is capture off.
  return {
    providerPayloads: bool(root?.providerPayloads) ?? false,
    harnessRpcPayloads: bool(root?.harnessRpcPayloads) ?? false,
  };
}
