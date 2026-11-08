// Auto-compaction decided by RustCode and run with the harness's own compaction
// (issue 3170). The decision is pure: a clock and a usage gauge go in, one action
// comes out. The store owns the timers and the prompt, so this file stays
// testable without React, the engine or a model turn.
//
// Two rules, as the issue proposes:
//   1. right after a turn, while the prompt cache is warm, when the context is
//      above the size threshold;
//   2. shortly before the prompt cache expires, when the thread is above a
//      smaller threshold, so the next turn writes a small context instead of
//      the whole thread.
// Rule 1 is the default. Rule 2 is the opt-in one: it was the default and it
// compacted threads four and a half minutes after an answer the reader had not
// come back to, which read as the answer going missing.

import type { ThreadStatus } from "../acp/v2";

export type AutoCompactMode = "off" | "size" | "cache";

/** How a harness's own compaction and prompt cache behave. */
export interface AutoCompactPolicy {
  /** Rule 1: compact when the context passes the size threshold. */
  size: boolean;
  /** Rule 2: compact before the prompt cache expires. */
  cache: boolean;
}

export interface AutoCompactSettings {
  mode: AutoCompactMode;
  /** The context size in harness tokens at which rule 1 fires. */
  threshold: number;
}

export const AUTO_COMPACT_MODES: { value: AutoCompactMode; label: string }[] = [
  { value: "off", label: "Off" },
  { value: "size", label: "When context passes" },
  { value: "cache", label: "Also before the cache expires" },
];

export const AUTO_COMPACT_DEFAULT_THRESHOLD = 150_000;
export const AUTO_COMPACT_MIN_THRESHOLD = 20_000;
export const AUTO_COMPACT_MAX_THRESHOLD = 500_000;
/** Rule 2 has its own, smaller threshold: a cache rewrite is worth avoiding before the window is full. */
export const AUTO_COMPACT_CACHE_THRESHOLD = 60_000;
/** Claude Code's prompt cache expires after about five minutes; 4.5 leaves room for the request. */
export const AUTO_COMPACT_CACHE_IDLE_MS = 4.5 * 60_000;
/**
 * Rule 2 is a window, not "any time after the deadline": a restart, a wake
 * from sleep or an old thread opened later must not compact with a cold cache.
 */
export const AUTO_COMPACT_CACHE_WINDOW_MS = 60_000;
/**
 * Rule 2 pays one cache-warm read and a summary. A thread nobody has followed
 * up for an hour is finished in practice, so it does not pay for that summary.
 */
export const AUTO_COMPACT_FOLLOW_UP_MS = 60 * 60_000;

export const DEFAULT_AUTO_COMPACT: AutoCompactSettings = {
  // The size rule only: a compaction nobody asked for must not take a thread's
  // answer away from a reader who has not come back to it yet.
  mode: "size",
  threshold: AUTO_COMPACT_DEFAULT_THRESHOLD,
};

export function clampAutoCompactThreshold(value: number): number {
  if (!Number.isFinite(value)) return AUTO_COMPACT_DEFAULT_THRESHOLD;
  return Math.min(AUTO_COMPACT_MAX_THRESHOLD, Math.max(AUTO_COMPACT_MIN_THRESHOLD, Math.round(value)));
}

export function parseAutoCompactMode(value: unknown): AutoCompactMode {
  return value === "off" || value === "size" || value === "cache" ? value : DEFAULT_AUTO_COMPACT.mode;
}

/** The setting as app storage has it: a JSON object, or an older plain mode string. */
export function parseAutoCompactSettings(raw: string | null): AutoCompactSettings {
  if (!raw) return DEFAULT_AUTO_COMPACT;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed === "string") {
      return { ...DEFAULT_AUTO_COMPACT, mode: parseAutoCompactMode(parsed) };
    }
    if (parsed && typeof parsed === "object") {
      const value = parsed as { mode?: unknown; threshold?: unknown };
      const threshold =
        typeof value.threshold === "number" ? value.threshold : DEFAULT_AUTO_COMPACT.threshold;
      return {
        mode: parseAutoCompactMode(value.mode),
        threshold: clampAutoCompactThreshold(threshold),
      };
    }
  } catch {
    // Not JSON: a mode written by an older build.
  }
  return { ...DEFAULT_AUTO_COMPACT, mode: parseAutoCompactMode(raw) };
}

/**
 * What each harness gets by default. Claude Code is the one the issue is about:
 * both rules. Codex already compacts inside a turn, so only the cache rule.
 * The built-in harness compacts before a request on its own and has no harness
 * cache to expire here, so RustCode leaves it alone. Any other ACP agent that
 * advertises `/compact` gets the size rule only: its cache expiry is unknown.
 */
export function autoCompactPolicy(harnessId: string, kind?: string): AutoCompactPolicy {
  if (harnessId === "claude-code") return { size: true, cache: true };
  if (harnessId === "codex" || kind === "codex") return { size: false, cache: true };
  if (kind === "builtin" || harnessId === "rustcode") return { size: false, cache: false };
  return { size: true, cache: false };
}

export interface AutoCompactInput {
  settings: AutoCompactSettings;
  policy: AutoCompactPolicy;
  /** The harness advertises compaction (built-in, Codex, or `/compact` in its commands). */
  canCompact: boolean;
  /** The last `used` from a `usage_update`, the harness's own token count. */
  usedTokens?: number;
  /** The app's clock. */
  now: number;
  /** When the thread's last turn ended; the cache clock starts there. */
  idleSince?: number;
  status: ThreadStatus;
  running: boolean;
  queued: number;
  approvals: number;
  /** Background commands or a parked turn keep the thread from being idle. */
  waiting: boolean;
  /** The thread waits for its plan's limit to reset. */
  limitWaiting: boolean;
  /** The thread was snoozed by the user. */
  snoozed: boolean;
  /**
   * The thread's last answer is still unread: the row is flagged Done (or
   * Failed) until the user opens it. Compacting then would rewrite the
   * transcript of an answer they have not read yet, so it waits.
   */
  unread: boolean;
  /** The user has started typing in the thread since it went idle. */
  typing: boolean;
  /** An automatic compaction already ran in this idle period. */
  compactedThisIdle: boolean;
  /** The thread was started by another thread (`_meta.rustcode.parent`). */
  delegated: boolean;
  /** A live link from its orchestrator still waits on or talks to it. */
  orchestratorTalking: boolean;
}

export type AutoCompactDecision =
  | { action: "none"; reason: string }
  | { action: "compact"; rule: "size" | "cache"; before: number }
  | { action: "wait"; rule: "cache"; at: number; before: number };

const NONE = (reason: string): AutoCompactDecision => ({ action: "none", reason });

/** The one place the two rules and their guards are decided. */
export function autoCompactDecision(input: AutoCompactInput): AutoCompactDecision {
  const { mode } = input.settings;
  if (mode === "off") return NONE("off");
  if (!input.canCompact) return NONE("harness cannot compact");
  if (!input.policy.size && !input.policy.cache) return NONE("nothing to do on this harness");
  if (input.running || input.status === "running") return NONE("turn running");
  if (input.status === "needs_input" || input.approvals > 0) return NONE("approval pending");
  if (input.queued > 0) return NONE("message queued");
  if (input.waiting) return NONE("background task running");
  if (input.limitWaiting) return NONE("usage limit wait pending");
  if (input.snoozed) return NONE("thread snoozed");
  // A thread the reader has not opened since its turn ended keeps its answer as it is.
  if (input.unread) return NONE("the last answer is unread");
  if (input.typing) return NONE("user typing");
  if (input.status === "done") return NONE("thread settled");
  if (input.delegated && !input.orchestratorTalking) return NONE("orchestrator is not talking to it");
  if (input.compactedThisIdle) return NONE("already compacted this idle period");

  const before = input.usedTokens;
  if (before === undefined || before <= 0) return NONE("no context measured");

  if (input.policy.size && before >= input.settings.threshold) {
    return { action: "compact", rule: "size", before };
  }

  if (mode === "cache" && input.policy.cache) {
    const cacheThreshold = Math.min(input.settings.threshold, AUTO_COMPACT_CACHE_THRESHOLD);
    if (before < cacheThreshold) return NONE("below the cache threshold");
    if (input.idleSince === undefined) return NONE("no idle clock");
    // A thread nobody followed up within the hour does not pay for a summary.
    if (input.now - input.idleSince > AUTO_COMPACT_FOLLOW_UP_MS) return NONE("no follow-up within the hour");
    const at = input.idleSince + AUTO_COMPACT_CACHE_IDLE_MS;
    if (input.now < at) return { action: "wait", rule: "cache", at, before };
    if (input.now > at + AUTO_COMPACT_CACHE_WINDOW_MS) return NONE("cache window passed");
    return { action: "compact", rule: "cache", before };
  }

  return NONE("below the threshold");
}

/** The card a compaction leaves says it ran on its own, not that the user asked. */
export function automaticCompactionText(text: string): string {
  const clean = text.replace(/^🧹\s*/, "").trim();
  if (/^compacted automatically\b/i.test(clean)) return clean;
  const rest = clean.replace(/^(history|context) compacted\b[:\s-]*/i, "").trim();
  return rest ? `Compacted automatically: ${rest}` : "Compacted automatically.";
}

/** The threshold the context bar marks, when rule 1 applies to this thread's harness. */
export function autoCompactMark(
  settings: AutoCompactSettings,
  policy: AutoCompactPolicy,
): number | undefined {
  return settings.mode !== "off" && policy.size ? settings.threshold : undefined;
}
