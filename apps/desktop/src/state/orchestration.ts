// Threads that start and message other threads: the rules and records of
// orchestration M1 and v2 (docs/architecture/cross-session-orchestration.md,
// docs/orchestrator.md), after RustCodeKit's Orchestration.swift. The
// engine forwards an agent's tool call as a `_rustcode/orchestration`
// request; the app checks these rules, asks the user, and carries it out
// (state/host.ts). Pure: no store, no engine.

import type { Json, JsonObject } from "../acp/jsonrpc";
import type { ThreadStatus, Worktree } from "../acp/v2";
import type { PermissionMode } from "./permissions";
import { snoozeAt } from "./snooze";
import { withWakeMark } from "./thread";

/** What the client advertises in `initialize` while the setting is on. */
export const CAPABILITY_VERSION = 4;
/** The most of a child's answer `wait_session` carries; beyond it, `read_session` pages the rest. */
export const MAX_BYTES = 8 * 1024;
/** The longest prompt or message a thread may hand another one (32 KiB). */
export const MAX_MESSAGE_BYTES = 32 * 1024;
/** One `read_session` page. */
export const READ_BYTES = 16 * 1024;
export const MAILBOX = 32;
/**
 * The default number of distinct threads one thread may have working for it at
 * once (Settings › Agents); the tree below it holds twice as many. A message
 * to a thread it already has a link to is a follow-up, not another slot.
 */
export const MAX_ACTIVE_LINKS = 8;
export const MIN_ACTIVE_LINKS = 2;
export const MAX_ACTIVE_LIMIT = 16;
/**
 * How deep delegation goes: a top-level thread (0) may start children (1), and
 * they may start grandchildren (2). A thread at the limit is told so, with the
 * limit in the reason, instead of being told it cannot delegate at all.
 */
export const MAX_DEPTH = 2;
/** Active threads one delegation tree may hold, whatever their depth: twice a thread's limit. */
export const MAX_TREE_LINKS = MAX_ACTIVE_LINKS * 2;
/** How many distinct target threads a whole tree may hold, from one thread's limit. */
export const treeLimit = (limit: number) => limit * 2;
export const DEFAULT_WAIT_MS = 60_000;
export const MAX_WAIT_MS = 3_600_000;
export const MAX_LAST_TURNS = 10;
/** How long a settled child waits for its siblings before the parent hears about them. */
export const WAKE_COALESCE_MS = 3_000;
/** The most of one child's answer a wake message carries, per child. */
export const WAKE_ANSWER_CHARS = 1024;

export type LinkState = "queued" | "working" | "needs_user" | "done" | "failed" | "cancelled" | "interrupted";

export const LINK_LABEL: Record<LinkState, string> = {
  queued: "Queued",
  working: "Working",
  needs_user: "Needs you",
  done: "Done",
  failed: "Failed",
  cancelled: "Stopped",
  interrupted: "Interrupted",
};

export const isActive = (state: LinkState) =>
  state === "queued" || state === "working" || state === "needs_user";

/** How a group of links is doing, for the card that holds them. */
export interface LinkCounts {
  working: number;
  needs: number;
  failed: number;
}

export function linkCounts(states: LinkState[]): LinkCounts {
  return {
    // A queued link has not started yet and reads as working, as its own row does.
    working: states.filter((s) => s === "working" || s === "queued").length,
    needs: states.filter((s) => s === "needs_user").length,
    failed: states.filter((s) => s === "failed").length,
  };
}

/** What a group of links says after its count: `2 working · 1 needs you`, `2 failed`, `done`. */
export function linkGroupStatus(counts: LinkCounts): string {
  const parts = [
    counts.working ? `${counts.working} working` : "",
    counts.needs ? `${counts.needs} ${counts.needs === 1 ? "needs" : "need"} you` : "",
  ].filter(Boolean);
  if (parts.length) return parts.join(" · ");
  return counts.failed ? `${counts.failed} failed` : "done";
}

/**
 * The links the strip above the composer shows: everything still working,
 * because that is the live control (Stop lives there), and a settled link with
 * no row of its own in the transcript — a thread this one started reads there,
 * a thread it only messaged reads here.
 */
export function stripLinks(links: Link[], shown: ReadonlySet<string>): Link[] {
  return links.filter((link) => isActive(link.state) || !shown.has(link.target));
}

/** One thread working for another: from the message (or the start) to the answer. */
export interface Link {
  /** `<orchestrator>/<n>`. */
  id: string;
  orchestrator: string;
  target: string;
  orchestratorTitle: string;
  targetTitle: string;
  message: string;
  state: LinkState;
  /** Started the target thread (`start_session`, `fan_out`) rather than messaging an existing one. */
  started?: boolean;
  harness?: string;
  fanOut?: string;
  answer?: string;
  truncated?: boolean;
  filesChanged?: number;
  createdAt: number;
  /** When it settled (done or failed), for the coalescing window. */
  settledAt?: number;
  /** When the parent was told it settled: one delivery per link, kept in the book across restarts. */
  notifiedAt?: number;
  /** The child's pending request (its rpc id, as text) the parent was told about. */
  askedRequest?: string;
  /**
   * When a reported link's answer was replaced by the turn its own child's wake
   * started: the parent is told once more, and never a third time.
   */
  answerChangedAt?: number;
}

/** One prompt run on several harnesses at once, compared side by side. */
export interface FanOut {
  id: string;
  orchestrator: string;
  title: string;
  prompt: string;
  links: string[];
  createdAt: number;
  /** The thread the user kept; the others were marked done. */
  kept?: string;
}

export interface LinkBook {
  links: Link[];
  /** What each thread may do without asking again: target thread ids, `start`, `start:<project>`. */
  grants: Record<string, string[]>;
  counters: Record<string, number>;
  fanOuts: FanOut[];
}

export const emptyBook = (): LinkBook => ({ links: [], grants: {}, counters: {}, fanOuts: [] });

export interface Failure {
  code: string;
  message: string;
  /** How many more links the thread can open: sent with a `busy` refusal. */
  linksLeft?: number;
}

export const failure = (code: string, message: string, linksLeft?: number): Failure => ({
  code,
  message,
  ...(linksLeft === undefined ? {} : { linksLeft }),
});
export const failureJson = (f: Failure): JsonObject => ({
  error: {
    code: f.code,
    message: f.message,
    ...(f.linksLeft === undefined ? {} : { links_left: f.linksLeft }),
  },
});

export interface HarnessChoice {
  harness: string;
  model?: string;
  effort?: string;
}

export type Call =
  | { tool: "list_sessions"; query: string; allProjects: boolean }
  | { tool: "read_session"; target: string; lastTurns: number; offset: number }
  | { tool: "message_session"; target: string; text: string }
  /** `callId` is the engine's name for this call, so a dropped call can end its wait. */
  | { tool: "wait_session"; links: string[]; timeoutMs: number; callId?: string }
  | {
      tool: "start_session";
      prompt: string;
      title?: string;
      harness?: string;
      model?: string;
      effort?: string;
      worktree?: boolean;
      project?: string;
    }
  | { tool: "fan_out"; prompt: string; title?: string; harnesses: HarnessChoice[] }
  | { tool: "list_harnesses" }
  | { tool: "search_sessions"; query: string; allProjects: boolean }
  | { tool: "list_worktrees"; allProjects: boolean }
  | {
      tool: "update_session";
      target?: string;
      title?: string;
      pinned?: boolean;
      unread?: boolean;
      status?: SessionStatusChoice;
      /** When it comes back, in ms; `null` brings it back now. */
      snoozedUntil?: number | null;
    }
  | { tool: "interrupt_session"; target: string }
  | { tool: "configure_session"; target?: string; model?: string; effort?: string }
  | { tool: "transfer_session"; target: string; harness: string }
  | { tool: "list_pending_requests" }
  | { tool: "list_projects" }
  | { tool: "list_pull_requests"; state: PullStateChoice }
  | { tool: "thread_usage"; since?: string; groupBy: UsageGroup; project?: string }
  | { tool: "query_data"; sql: string; limit: number };

/** What `update_session` may set a thread's status to. */
export type SessionStatusChoice = "active" | "review" | "settled";
const SESSION_STATUSES: readonly string[] = ["active", "review", "settled"];

/** What `list_pull_requests` may ask for. */
export type PullStateChoice = "open" | "closed" | "merged" | "all";
const PULL_STATES: readonly string[] = ["open", "closed", "merged", "all"];
/** How `thread_usage` groups its rows. */
export type UsageGroup = "harness" | "model" | "day" | "project" | "thread";
const USAGE_GROUPS: readonly string[] = ["harness", "model", "day", "project", "thread"];
/** Rows `query_data` may bring back in one answer. */
export const MAX_DATA_ROWS = 1000;

/** Pull requests listed, and projects: an agent's context pays for each. */
export const MAX_PULL_REQUESTS = 30;
export const MAX_PROJECTS = 50;

const s = (v: Json | undefined) => (typeof v === "string" ? v : undefined);
/** `all_projects`: the user's threads in every project, when the setting allows it. */
const allProjects = (args: JsonObject) => args.all_projects === true;
const bytes = (text: string) => new TextEncoder().encode(text).length;

/** The call in a request's params, or the failure to answer with. */
export function parseCall(params: JsonObject): Call | Failure {
  const raw = params.args;
  const args = raw && typeof raw === "object" && !Array.isArray(raw) ? raw : {};
  switch (s(params.tool)) {
    case "list_sessions":
      return { tool: "list_sessions", query: s(args.query) ?? "", allProjects: allProjects(args) };
    case "list_harnesses":
      return { tool: "list_harnesses" };
    case "list_worktrees":
      return { tool: "list_worktrees", allProjects: allProjects(args) };
    case "list_projects":
      return { tool: "list_projects" };
    case "list_pull_requests": {
      const state = s(args.state) ?? "open";
      if (!PULL_STATES.includes(state)) {
        return failure("invalid", `state must be one of ${PULL_STATES.join(", ")}`);
      }
      return { tool: "list_pull_requests", state: state as PullStateChoice };
    }
    case "search_sessions": {
      const query = (s(args.query) ?? "").trim();
      if (!query) return failure("invalid", "query is required");
      return { tool: "search_sessions", query, allProjects: allProjects(args) };
    }
    case "interrupt_session": {
      const target = s(args.session_id) ?? "";
      if (!target) return failure("invalid", "session_id is required");
      return { tool: "interrupt_session", target };
    }
    case "transfer_session": {
      const target = s(args.session_id) ?? "";
      const harness = s(args.harness) ?? "";
      if (!target || !harness) return failure("invalid", "session_id and harness are required");
      return { tool: "transfer_session", target, harness };
    }
    case "list_pending_requests":
      return { tool: "list_pending_requests" };
    case "thread_usage": {
      const since = s(args.since)?.trim() || undefined;
      if (since && !/^\d{4}-\d{2}(-\d{2})?$/.test(since)) {
        return failure("invalid", "since must be YYYY-MM-DD or YYYY-MM");
      }
      const group = s(args.group_by) ?? "harness";
      if (!USAGE_GROUPS.includes(group)) {
        return failure("invalid", `group_by must be one of ${USAGE_GROUPS.join(", ")}`);
      }
      return {
        tool: "thread_usage",
        since,
        groupBy: group as UsageGroup,
        project: s(args.project)?.trim() || undefined,
      };
    }
    case "query_data": {
      const sql = (s(args.sql) ?? "").trim();
      if (!sql) return failure("invalid", "sql is required");
      const asked = typeof args.limit === "number" ? Math.floor(args.limit) : MAX_DATA_ROWS;
      return { tool: "query_data", sql, limit: Math.min(Math.max(asked, 1), MAX_DATA_ROWS) };
    }
    case "configure_session":
      return {
        tool: "configure_session",
        target: s(args.session_id),
        model: s(args.model),
        effort: s(args.effort),
      };
    case "update_session": {
      const status = s(args.status);
      if (status && !SESSION_STATUSES.includes(status)) {
        return failure("invalid", `status must be one of ${SESSION_STATUSES.join(", ")}`);
      }
      const title = s(args.title)?.trim();
      const pinned = typeof args.pinned === "boolean" ? args.pinned : undefined;
      const unread = typeof args.unread === "boolean" ? args.unread : undefined;
      let snoozedUntil: number | null | undefined;
      if (args.snoozed_until === null) snoozedUntil = null;
      else if (args.snoozed_until !== undefined) {
        snoozedUntil = snoozeAt(s(args.snoozed_until) ?? "", Date.now());
        if (Number.isNaN(snoozedUntil))
          return failure(
            "invalid",
            "snoozed_until must be 1h, evening, tomorrow, next_week or an ISO 8601 time",
          );
      }
      if (!title && pinned === undefined && unread === undefined && !status && snoozedUntil === undefined) {
        return failure("invalid", "give at least one of title, pinned, unread, status or snoozed_until");
      }
      return {
        tool: "update_session",
        target: s(args.session_id),
        title,
        pinned,
        unread,
        status: status as SessionStatusChoice | undefined,
        snoozedUntil,
      };
    }
    case "read_session": {
      const target = s(args.session_id) ?? "";
      if (!target) return failure("invalid", "session_id is required");
      const n = typeof args.last_turns === "number" ? Math.floor(args.last_turns) : 3;
      if (n < 1 || n > MAX_LAST_TURNS) return failure("invalid", `last_turns must be 1 to ${MAX_LAST_TURNS}`);
      const offset = typeof args.offset === "number" ? Math.floor(args.offset) : 0;
      // Bytes into the rendered output: a page is a UTF-8 slice of it, never a character split.
      if (!Number.isFinite(offset) || offset < 0)
        return failure("invalid", "offset must be a whole number of bytes");
      return { tool: "read_session", target, lastTurns: n, offset };
    }
    case "message_session": {
      const target = s(args.session_id) ?? "";
      const text = s(args.message) ?? "";
      if (!target || !text.trim()) return failure("invalid", "session_id and message are required");
      if (bytes(text) > MAX_MESSAGE_BYTES) return failure("invalid", "the message is over 32 KiB");
      return { tool: "message_session", target, text };
    }
    case "wait_session": {
      // One answer per link: a repeated id is asked about once.
      const links = Array.isArray(args.link_ids)
        ? [...new Set(args.link_ids.flatMap((x) => (typeof x === "string" ? [x] : [])))]
        : [];
      const one = s(args.link_id);
      if (one && !links.includes(one)) links.unshift(one);
      if (!links.length) return failure("invalid", "link_id is required");
      const asked = typeof args.timeout_ms === "number" ? args.timeout_ms : DEFAULT_WAIT_MS;
      return {
        tool: "wait_session",
        links,
        timeoutMs: Math.min(Math.max(asked, 1), MAX_WAIT_MS),
        callId: s(params.callId),
      };
    }
    case "start_session": {
      const prompt = s(args.prompt) ?? "";
      if (!prompt.trim()) return failure("invalid", "prompt is required");
      if (bytes(prompt) > MAX_MESSAGE_BYTES) return failure("invalid", "the prompt is over 32 KiB");
      return {
        tool: "start_session",
        prompt,
        title: s(args.title)?.trim() || undefined,
        harness: s(args.harness),
        model: s(args.model),
        effort: s(args.effort),
        worktree: typeof args.worktree === "boolean" ? args.worktree : undefined,
        project: s(args.project),
      };
    }
    case "fan_out": {
      const prompt = s(args.prompt) ?? "";
      if (!prompt.trim()) return failure("invalid", "prompt is required");
      if (bytes(prompt) > MAX_MESSAGE_BYTES) return failure("invalid", "the prompt is over 32 KiB");
      const list = Array.isArray(args.harnesses) ? args.harnesses : [];
      const harnesses: HarnessChoice[] = [];
      for (const entry of list) {
        const o = entry && typeof entry === "object" && !Array.isArray(entry) ? entry : {};
        const harness = s(o.harness);
        if (!harness) return failure("invalid", "each harnesses entry needs a harness");
        harnesses.push({ harness, model: s(o.model), effort: s(o.effort) });
      }
      if (harnesses.length < 2 || harnesses.length > 4)
        return failure("invalid", "harnesses takes 2 to 4 entries");
      return { tool: "fan_out", prompt, title: s(args.title)?.trim() || undefined, harnesses };
    }
    default:
      return failure("invalid", "unknown tool");
  }
}

export function isFailure(value: unknown): value is Failure {
  return !!value && typeof value === "object" && "code" in value && "message" in value && !("tool" in value);
}

/** A thread as the rules see it. */
export interface RuleThread {
  id: string;
  title: string;
  folder: string;
  worktree?: Worktree;
  updatedAt?: number;
  running: boolean;
  harness?: string;
  status?: ThreadStatus;
}

// macOS links /tmp, /var and /etc into /private, and the engine reports the resolved path: one place.
const clean = (path: string) =>
  path
    .replace(/\/+$/, "")
    .replace(/\/\.\//g, "/")
    .replace(/^\/private(?=\/(?:tmp|var|etc)(?:\/|$))/, "");

/** A thread's project: its worktree's repository, else its folder. */
export const projectOf = (t: Pick<RuleThread, "folder" | "worktree">) => clean(t.worktree?.repo || t.folder);

/** Two folders are one project when they are the same place. */
export const sameProject = (a: string, b: string) => !!a && clean(a) === clean(b);

/** `list_sessions`: the other threads of the caller's project, newest first, metadata only. */
/** Why a thread may not manage `target`, the project rule every management tool shares. */
export function checkManage(caller: RuleThread, target: RuleThread | undefined): Failure | null {
  if (!target) return failure("not_found", "no thread has that id; use list_sessions");
  if (!sameProject(projectOf(target), projectOf(caller))) {
    return failure(
      "not_allowed",
      "that thread is in another project; only this project's threads can be reached",
    );
  }
  return null;
}

/**
 * `list_sessions`: the other threads, newest first, metadata only. The caller's
 * project unless `allProjects`, where every entry names its project.
 */
export function listSessions(
  threads: RuleThread[],
  caller: RuleThread,
  query: string,
  allProjects = false,
): JsonObject {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const shown = threads
    .filter((t) => t.id !== caller.id)
    .filter((t) => allProjects || sameProject(projectOf(t), projectOf(caller)))
    .filter((t) => words.every((w) => t.title.toLowerCase().includes(w)))
    .sort((a, b) => (b.updatedAt ?? 0) - (a.updatedAt ?? 0))
    .slice(0, 50);
  return {
    sessions: shown.map((t) => {
      const entry: JsonObject = {
        session_id: t.id,
        title: t.title,
        working: t.running,
        status: t.status ?? (t.running ? "running" : "idle"),
        harness: t.harness ?? "rustcode",
      };
      // Across projects, which project a thread is in is what tells them apart.
      if (allProjects) entry.project = projectOf(t);
      if (t.updatedAt) entry.updated_at = new Date(t.updatedAt).toISOString();
      if (t.worktree)
        entry.worktree = { path: t.worktree.path, branch: t.worktree.branch, repo: t.worktree.repo };
      return entry;
    }),
  };
}

/**
 * Whether `caller` may message (or read) `target`, before the user is asked.
 * Reading reaches any project while the app allows it; everything that writes,
 * or that opens a thread to another, stays in the caller's own project.
 */
export function checkMessage(
  caller: RuleThread,
  target: RuleThread | undefined,
  book: LinkBook,
  read = false,
  acrossProjects = false,
  limit = MAX_ACTIVE_LINKS,
): Failure | null {
  if (!target) return failure("not_found", "no thread has that id; use list_sessions");
  if (target.id === caller.id)
    return failure("not_allowed", `a thread cannot ${read ? "read" : "message"} itself`);
  if (!(read && acrossProjects) && !sameProject(projectOf(target), projectOf(caller))) {
    return failure(
      "not_allowed",
      "that thread is in another project; only this project's threads can be reached",
    );
  }
  if (read) return null;
  const chain = chainRefusal(book, caller, target);
  if (chain) return chain;
  if (book.links.some((l) => l.target === target.id && l.orchestrator !== caller.id && isActive(l.state))) {
    return failure("target_linked", "another thread is already working with that one");
  }
  // A message to a thread this caller already has a live link to is a follow-up:
  // it queues on that thread, so it never counts against a slot, this thread's
  // limit or its tree's.
  if (!hasActiveTo(book, caller.id, target.id)) {
    const slots = slotRefusal(book, caller, 1, limit);
    if (slots) return slots;
  }
  if (book.links.filter((l) => l.target === target.id && l.state === "queued").length >= MAILBOX) {
    return failure("mailbox_full", `that thread has ${MAILBOX} messages waiting`);
  }
  return null;
}

/**
 * The project `thread_usage` reads: the one asked for, the caller's while
 * "Let agents read threads in other projects" is off, or none (every project)
 * while it is on. A failure when the setting is off and another project was
 * asked for by name — better than quietly answering about a different scope.
 */
export function usageScope(
  caller: RuleThread,
  acrossProjects: boolean,
  asked?: string,
): { project?: string; scope: "this project" | "every project" } | Failure {
  if (acrossProjects) return asked ? { project: asked, scope: "this project" } : { scope: "every project" };
  const own = projectOf(caller);
  if (asked && !sameProject(asked, own)) {
    return failure(
      "not_allowed",
      "reading usage across projects is off; turn on “Let agents read threads in other projects” in Settings › Agents",
    );
  }
  return { project: own, scope: "this project" };
}

/**
 * Whether `query_data` may run at all. SQL over the store cannot be narrowed to
 * one project safely (a join, a subquery, a view), so it needs the setting.
 */
export function checkQueryData(acrossProjects: boolean): Failure | null {
  return acrossProjects
    ? null
    : failure(
        "not_allowed",
        "SQL reads every project, so it needs “Let agents read threads in other projects” in Settings › Agents",
      );
}

/** Whether `caller` may start `count` threads, before the user is asked. */
export function checkStart(
  caller: RuleThread,
  book: LinkBook,
  count: number,
  limit = MAX_ACTIVE_LINKS,
): Failure | null {
  const chain = chainRefusal(book, caller);
  if (chain) return chain;
  return slotRefusal(book, caller, count, limit);
}

/** The message as the other thread receives it: who it is from, then the text. */
export const delivered = (text: string, from: string) => `From thread “${from}” (agent):\n\n${text}`;

/** One turn of a thread as `read_session` renders it. */
export interface ReadTurn {
  user: string;
  assistant: string;
  files_changed: number;
}

/** What `read_session` renders, before it is cut into pages. */
export interface ReadAnswer {
  session_id: string;
  title: string;
  status: string;
  project: string;
  turns: ReadTurn[];
}

/**
 * The whole `read_session` answer as text, which `offset` then pages. Nothing
 * here is cut: a page is a byte range of this string, so pages join exactly.
 */
export function renderRead(answer: ReadAnswer): string {
  const lines = [
    `Thread: ${answer.title}`,
    `Session: ${answer.session_id}`,
    `Status: ${answer.status}`,
    `Project: ${answer.project}`,
    `Turns: ${answer.turns.length}`,
    "",
  ];
  answer.turns.forEach((turn, i) => {
    lines.push(`Turn ${i + 1}`);
    lines.push("  user:");
    for (const line of String(turn.user).split("\n")) lines.push(`    ${line}`);
    lines.push("  assistant:");
    for (const line of String(turn.assistant).split("\n")) lines.push(`    ${line}`);
    lines.push(`  files changed: ${turn.files_changed}`);
    lines.push("");
  });
  return lines.join("\n");
}

/**
 * `text` from byte `offset`, at most `max` bytes, cut on a UTF-8 character
 * boundary. `nextOffset` is where the next page starts, or null at the end.
 * Concatenating the pages rebuilds `text` exactly. An offset inside a
 * character is refused rather than silently moved, as `zoom_context` does.
 */
export function pageOf(
  text: string,
  offset: number,
  max = READ_BYTES,
): { text: string; offset: number; nextOffset: number | null } | Failure {
  const raw = new TextEncoder().encode(text);
  if (!Number.isInteger(offset) || offset < 0 || offset > raw.length) {
    return failure("invalid", `offset ${offset} is out of range; this answer is ${raw.length} bytes`);
  }
  if (offset < raw.length && (raw[offset]! & 0xc0) === 0x80) {
    return failure("invalid", "offset must be on a UTF-8 character boundary; use the returned next_offset");
  }
  let end = Math.min(raw.length, offset + max);
  while (end > offset && end < raw.length && (raw[end]! & 0xc0) === 0x80) end -= 1;
  return {
    text: new TextDecoder().decode(raw.subarray(offset, end)),
    offset,
    nextOffset: end < raw.length ? end : null,
  };
}

/** `text` cut to `max` bytes on a character boundary, and whether it was. */
export function capped(text: string, max = MAX_BYTES): { text: string; truncated: boolean } {
  if (bytes(text) <= max) return { text, truncated: false };
  let out = "";
  let size = 0;
  for (const ch of text) {
    const n = bytes(ch);
    if (size + n > max) break;
    size += n;
    out += ch;
  }
  return { text: out, truncated: true };
}

export function nextId(book: LinkBook, orchestrator: string): { book: LinkBook; id: string } {
  const n = (book.counters[orchestrator] ?? 0) + 1;
  return { book: { ...book, counters: { ...book.counters, [orchestrator]: n } }, id: `${orchestrator}/${n}` };
}

export const hasGrant = (book: LinkBook, caller: string, key: string) =>
  book.grants[caller]?.includes(key) ?? false;

export function grant(book: LinkBook, caller: string, key: string): LinkBook {
  if (hasGrant(book, caller, key)) return book;
  return { ...book, grants: { ...book.grants, [caller]: [...(book.grants[caller] ?? []), key] } };
}

export function updateLink(book: LinkBook, id: string, change: Partial<Link>): LinkBook {
  return { ...book, links: book.links.map((l) => (l.id === id ? { ...l, ...change } : l)) };
}

/** After a restart: work that was queued or running did not finish here, and nothing restarts by itself. */
export function interruptActive(book: LinkBook): LinkBook {
  return { ...book, links: book.links.map((l) => (isActive(l.state) ? { ...l, state: "interrupted" } : l)) };
}

/** Links kept: every active one, and the latest 50 finished; fan-outs whose links are all gone go too. */
export function prune(book: LinkBook): LinkBook {
  const finished = new Set(
    book.links
      .filter((l) => !isActive(l.state))
      .sort((a, b) => b.createdAt - a.createdAt)
      .slice(0, 50)
      .map((l) => l.id),
  );
  const links = book.links.filter((l) => isActive(l.state) || finished.has(l.id));
  const ids = new Set(links.map((l) => l.id));
  return {
    ...book,
    links,
    fanOuts: book.fanOuts.filter((f) => f.links.some((id) => ids.has(id))).slice(-20),
  };
}

const LINK_STATES = Object.keys(LINK_LABEL) as LinkState[];

/** The book the data store answers with, or null when it is not a book at all. */
export function parseBook(raw: Json | undefined): LinkBook | null {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const object = raw as Record<string, Json | undefined>;
  const links: Link[] = [];
  for (const entry of Array.isArray(object.links) ? object.links : []) {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) continue;
    const link = entry as Record<string, Json | undefined>;
    if (typeof link.id !== "string" || typeof link.orchestrator !== "string") continue;
    if (typeof link.target !== "string" || typeof link.state !== "string") continue;
    if (!LINK_STATES.includes(link.state as LinkState)) continue;
    // The rest travels as it was written, so a field this build does not know survives.
    links.push({
      ...(link as unknown as Link),
      createdAt: typeof link.createdAt === "number" ? link.createdAt : Date.now(),
    });
  }
  const record = (value: Json | undefined) =>
    value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, Json>) : {};
  const grants: Record<string, string[]> = {};
  for (const [who, keys] of Object.entries(record(object.grants))) {
    if (Array.isArray(keys)) grants[who] = keys.filter((key): key is string => typeof key === "string");
  }
  const counters: Record<string, number> = {};
  for (const [who, n] of Object.entries(record(object.counters))) {
    if (typeof n === "number" && Number.isFinite(n)) counters[who] = n;
  }
  const fanOuts: FanOut[] = [];
  for (const entry of Array.isArray(object.fanOuts) ? object.fanOuts : []) {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) continue;
    const group = entry as Record<string, Json | undefined>;
    if (typeof group.id !== "string" || !Array.isArray(group.links)) continue;
    fanOuts.push(group as unknown as FanOut);
  }
  return { links, grants, counters, fanOuts };
}

/** When a link settled, or -1 while it has not: the later record of one link knows more. */
const settledAtOf = (link: Link) => (typeof link.settledAt === "number" ? link.settledAt : -1);

/**
 * One link in two copies: the record that settled (or settled last) says what
 * the link is and what it answered. The marks that keep a wake from being
 * delivered twice are kept from either copy, so a crash between a wake and its
 * save does not repeat it after a restart.
 */
function mergedLink(shared: Link, mine: Link): Link {
  const [base, other] = settledAtOf(mine) > settledAtOf(shared) ? [mine, shared] : [shared, mine];
  return {
    ...other,
    ...base,
    ...(base.notifiedAt === undefined && other.notifiedAt !== undefined
      ? { notifiedAt: other.notifiedAt }
      : {}),
    ...(base.askedRequest === undefined && other.askedRequest !== undefined
      ? { askedRequest: other.askedRequest }
      : {}),
  };
}

/**
 * Two copies of the book: the one the data store holds (shared with every
 * window) and this window's. A link is taken once: a record that settled beats
 * one that did not, and otherwise the store's is kept, so a second window
 * sees what another window already saved. A link only this window has —
 * started while the store was answering — is kept, and a grant or counter is
 * merged rather than lost, so nothing is opened twice under the same id.
 */
export function mergeBooks(shared: LinkBook, mine: LinkBook): LinkBook {
  const links = new Map(shared.links.map((link) => [link.id, link] as const));
  for (const link of mine.links) {
    const seen = links.get(link.id);
    if (!seen) links.set(link.id, link);
    else links.set(link.id, mergedLink(seen, link));
  }
  const grants: Record<string, string[]> = { ...shared.grants };
  for (const [who, keys] of Object.entries(mine.grants)) {
    grants[who] = [...new Set([...(grants[who] ?? []), ...keys])];
  }
  const counters: Record<string, number> = { ...shared.counters };
  for (const [who, n] of Object.entries(mine.counters)) {
    counters[who] = Math.max(counters[who] ?? 0, n);
  }
  const fanOuts = [...shared.fanOuts];
  const known = new Set(fanOuts.map((group) => group.id));
  for (const group of mine.fanOuts) if (!known.has(group.id)) fanOuts.push(group);
  return { links: [...links.values()], grants, counters, fanOuts };
}

/** The links a thread started that are still live. */
const activeFrom = (book: LinkBook, id: string) =>
  book.links.filter((l) => l.orchestrator === id && isActive(l.state));

/** The distinct threads a thread has working for it: follow-ups do not take another slot. */
export const activeTargets = (book: LinkBook, id: string): Set<string> =>
  new Set(activeFrom(book, id).map((l) => l.target));

/** Whether `orchestrator` already has a live link to `target`. */
const hasActiveTo = (book: LinkBook, orchestrator: string, target: string) =>
  book.links.some((l) => l.orchestrator === orchestrator && l.target === target && isActive(l.state));

/** How many more threads a thread may open at once, for an answer to say. */
export const linksLeft = (book: LinkBook, id: string, limit = MAX_ACTIVE_LINKS): number =>
  Math.max(0, limit - activeTargets(book, id).size);

/** The thread working for `id` right now, if any (a target has at most one). */
const parentLink = (book: LinkBook, id: string) =>
  book.links.find((l) => l.target === id && isActive(l.state));

/**
 * Every thread above `id`: the ones it works for, directly or further up. Safe
 * against a cycle in the book, which a broken record or an older release
 * could leave: the walk stops at a thread it has already seen.
 */
export function ancestorsOf(book: LinkBook, id: string): Set<string> {
  const above = new Set<string>();
  let at = id;
  for (;;) {
    const parent = parentLink(book, at);
    if (!parent || above.has(parent.orchestrator)) return above;
    above.add(parent.orchestrator);
    at = parent.orchestrator;
  }
}

/**
 * How far a thread sits under the top-level one: 0 when nobody is working for
 * it, 1 for a child, 2 for a grandchild. A book with a cycle in it answers
 * deeper than any limit rather than looping forever.
 */
export function depthOf(book: LinkBook, id: string): number {
  const above = ancestorsOf(book, id);
  // A cycle means the walk found its own start among the ancestors.
  if (above.has(id)) return MAX_DEPTH + 1;
  return above.size;
}

/** The top-level thread `id` works within: itself, or the highest thread above it. */
export function rootOf(book: LinkBook, id: string): string {
  let at = id;
  for (;;) {
    const parent = parentLink(book, at);
    if (!parent || parent.orchestrator === id) return at;
    at = parent.orchestrator;
  }
}

/**
 * The threads of `id`'s delegation tree, its root included: the whole tree is
 * capped ({@link MAX_TREE_LINKS}) so one thread cannot fill the machine with
 * grandchildren.
 */
export function treeOf(book: LinkBook, id: string): Set<string> {
  const ids = new Set([rootOf(book, id)]);
  for (let added = true; added; ) {
    added = false;
    for (const link of book.links) {
      if (!isActive(link.state) || ids.has(link.target) || !ids.has(link.orchestrator)) continue;
      ids.add(link.target);
      added = true;
    }
  }
  return ids;
}

/** Active threads in `id`'s tree, which the tree cap counts: one per target, not one per link. */
export function activeInTree(book: LinkBook, id: string): number {
  const ids = treeOf(book, id);
  return new Set(book.links.filter((l) => isActive(l.state) && ids.has(l.orchestrator)).map((l) => l.target))
    .size;
}

/**
 * Every active link under `id`, in tree order: what stopping `id` takes down
 * with it. Safe against a cycle: a thread is walked once.
 */
export function activeDescendants(book: LinkBook, id: string): Link[] {
  const out: Link[] = [];
  const seen = new Set([id]);
  let frontier = [id];
  while (frontier.length) {
    const next: string[] = [];
    for (const link of book.links) {
      if (!isActive(link.state) || seen.has(link.target)) continue;
      if (!frontier.includes(link.orchestrator)) continue;
      seen.add(link.target);
      out.push(link);
      next.push(link.target);
    }
    frontier = next;
  }
  return out;
}

/**
 * The `busy` refusal when `count` more threads would not fit: this thread's
 * limit at once, or its tree's twice that. A follow-up to a thread it already
 * works with passes `count: 0`, so it never takes a slot. `links_left` says
 * what does fit, so the agent can retry with fewer instead of guessing.
 */
function slotRefusal(
  book: LinkBook,
  caller: RuleThread,
  count: number,
  limit = MAX_ACTIVE_LINKS,
): Failure | null {
  const active = activeTargets(book, caller.id).size;
  if (active + count > limit) {
    const left = Math.max(0, limit - active);
    return failure(
      "busy",
      `at most ${limit} threads at once; wait_session on one first (this thread has ${active}, so ${count} would not fit, and ${left} ${left === 1 ? "slot is" : "slots are"} left)`,
      left,
    );
  }
  const tree = activeInTree(book, caller.id);
  const most = treeLimit(limit);
  if (tree + count > most) {
    const left = Math.max(0, most - tree);
    return failure(
      "busy",
      `this thread's tree holds at most ${most} active threads and has ${tree}: ${left} ${left === 1 ? "slot is" : "slots are"} left, so ${count} would not fit`,
      left,
    );
  }
  return null;
}

/**
 * Why `caller` may not hand work to another thread: it sits at the depth
 * limit, or (messaging) `target` already works for it. Null when it may.
 */
function chainRefusal(book: LinkBook, caller: RuleThread, target?: RuleThread): Failure | null {
  if (target && ancestorsOf(book, caller.id).has(target.id)) {
    return failure(
      "not_allowed",
      `“${target.title}” already works for this thread: messaging it would close a loop`,
    );
  }
  const depth = depthOf(book, caller.id);
  if (depth > MAX_DEPTH) {
    // A cycle left by a broken record says nothing useful; the depth does.
    return failure("not_allowed", "this thread's links loop; no other thread can be involved", 0);
  }
  if (depth === MAX_DEPTH) {
    return failure(
      "not_allowed",
      `this thread is ${depth} levels under the one that started the work, and delegation stops at ${MAX_DEPTH}: give the answer back instead, or ask the thread above you`,
    );
  }
  return null;
}
export function forget(book: LinkBook, thread: string): LinkBook {
  const grants = Object.fromEntries(
    Object.entries(book.grants)
      .filter(([k]) => k !== thread)
      .map(([k, v]) => [k, v.filter((x) => x !== thread)]),
  );
  return {
    ...book,
    grants,
    links: book.links
      .filter((l) => l.orchestrator !== thread)
      .map((l) =>
        l.target === thread && isActive(l.state)
          ? { ...l, state: "failed", answer: "That thread was deleted." }
          : l,
      ),
    fanOuts: book.fanOuts.filter((f) => f.orchestrator !== thread),
  };
}

/** A link's state as shown: a working turn that waits on the user needs them. */
export const shownState = (link: Link, targetWaits: boolean): LinkState =>
  link.state === "working" && targetWaits ? "needs_user" : link.state;

/**
 * `wait_session`'s answer for one link. With `activity` (what the app already
 * holds of the child's thread) it also says how long the child has been idle,
 * the last tool it ran and how many turns it finished, so a caller that gets a
 * timeout knows whether it is thinking, stuck or over.
 */
export function linkResult(
  link: Link,
  state: LinkState,
  status?: ThreadStatus,
  extra?: { now?: number; activity?: LinkActivity },
): JsonObject {
  const entry: JsonObject = {
    link_id: link.id,
    status: state,
    session_id: link.target,
    title: link.targetTitle,
  };
  if (status) entry.thread_status = status;
  if (link.answer !== undefined) entry.answer = link.answer;
  if (link.truncated) {
    // The answer was cut at the 8 KiB budget: the rest is in read_session on this thread.
    entry.answer_truncated = true;
  }
  if (link.filesChanged !== undefined) entry.files_changed = link.filesChanged;
  const activity = extra?.activity;
  if (activity) {
    entry.turns = activity.turns;
    if (activity.lastTool) entry.last_tool = activity.lastTool;
    if (activity.lastAt !== undefined && extra?.now !== undefined) {
      entry.idle_ms = Math.max(0, extra.now - activity.lastAt);
    }
  }
  // The user answers it wherever they see the card: in that thread's chat, or in the transcript of
  // the thread above it (the card is shown in both). The agent still cannot answer it.
  if (state === "needs_user")
    entry.note =
      "That thread is waiting for the user to answer it in the app, in its chat or the one above it.";
  return entry;
}

// ---------------------------------------------------------------- waking a parent

/** One item of a child's transcript, as the idle summary reads it. */
export interface ActivityItem {
  kind: string;
  at?: number;
  startedAt?: number;
  endedAt?: number;
  title?: string;
  toolName?: string;
}

/** What a child was last doing, and how many turns it finished (`wait_session` timeout). */
export interface LinkActivity {
  lastAt?: number;
  lastTool?: string;
  turns: number;
}

/** The last activity in a thread, from the items the app already holds: no polling. */
export function activityOf(items: readonly ActivityItem[]): LinkActivity {
  let lastAt: number | undefined;
  let lastTool: string | undefined;
  let turns = 0;
  for (const item of items) {
    const at = item.endedAt ?? item.startedAt ?? item.at;
    if (at !== undefined && (lastAt === undefined || at > lastAt)) lastAt = at;
    if (item.kind === "tool") lastTool = item.toolName ?? item.title ?? lastTool;
    if (item.kind === "turnEnd") turns += 1;
  }
  return { lastAt, lastTool, turns };
}

/** A parent that should hear about its children, and what about them. */
export interface Wake {
  parent: string;
  parentTitle: string;
  /** Children that settled and were not reported yet. */
  settled: Link[];
  /** Children waiting on the user, whose request was not reported yet. */
  asking: Link[];
}

/** What the decision needs of the app: states, pending requests, waits, threads. */
export interface WakeContext {
  /** Each link's state as shown (`needs_user` while its child waits on the user). */
  states: Readonly<Record<string, LinkState>>;
  /** Each child's pending request id, when it waits on one. */
  pending: Readonly<Record<string, string>>;
  /** The links each parent is blocked on in `wait_session` right now. */
  waiting: Readonly<Record<string, readonly string[]>>;
  /**
   * The threads these rules need, by id: every link's orchestrator and every
   * link's target, with the title a wake message names and whether a turn runs.
   * A thread that is missing is gone.
   */
  threads: Readonly<Record<string, { title: string; running: boolean }>>;
  /** Each link target's latest reply, for a finished link that holds no answer. */
  answers?: Readonly<Record<string, string>>;
  now: number;
}

const stateOfLink = (link: Link, ctx: WakeContext) => ctx.states[link.id] ?? link.state;

/**
 * Whether `id` still owes its parent an answer: a child of its own is active,
 * its own child result was not reported to it yet, or one of its turns runs. A
 * middle thread that ended its turn after starting a leaf has *not* finished
 * for its parent while the leaf works, or while the wake from the leaf is still
 * to come — the answer it gave was "I started it", not the real one.
 */
export function quietTree(book: LinkBook, ctx: WakeContext, id: string): boolean {
  if (ctx.threads[id]?.running) return false;
  if (book.links.some((l) => l.orchestrator === id && isActive(stateOfLink(l, ctx)))) return false;
  return !pendingWakeUnder(book, ctx, id);
}

/**
 * A child result `id` has not been told about yet: a wake for it is due or
 * about to be. Only `done` and `failed` wake a parent, so an interrupted or
 * stopped child is not a reason to wait: nothing will ever report it.
 */
export function pendingWakeUnder(book: LinkBook, ctx: WakeContext, id: string): boolean {
  return book.links.some((link) => {
    if (link.orchestrator !== id) return false;
    const state = stateOfLink(link, ctx);
    if (state === "done" || state === "failed") return link.notifiedAt === undefined;
    if (state !== "needs_user") return false;
    const request = ctx.pending[link.id];
    return request !== undefined && link.askedRequest !== request;
  });
}

/** What one turn of a thread answered. */
export interface TurnAnswer {
  text: string;
  truncated: boolean;
  filesChanged: number;
}

/** The last reply in a transcript, whatever turn it belongs to. */
export function latestReply(items: readonly { kind: string; text?: string }[]): string {
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i]!;
    if (item.kind === "assistant" && item.text?.trim()) return item.text;
  }
  return "";
}

/**
 * A finished link's answer, read from the thread as it is now. A turn's rows
 * reach the app a moment after its prompt returns, and a link that settled on
 * that turn can hold nothing; the target's last reply is still the answer, and
 * the wake message and `wait_session` must carry it rather than an empty line.
 */
export function fillAnswers(book: LinkBook, answers: Readonly<Record<string, string>>): LinkBook {
  let changed = false;
  const links = book.links.map((link) => {
    if (link.state !== "done" && link.state !== "failed") return link;
    if (link.answer?.trim()) return link;
    const reply = answers[link.target];
    if (!reply) return link;
    changed = true;
    const { text, truncated } = capped(reply);
    return { ...link, answer: text, truncated };
  });
  return changed ? { ...book, links } : book;
}

/**
 * The book after a turn of `target` ended. A link from a parent that settled
 * but was not reported yet takes this turn's answer: the parent's wake waits for
 * the target's whole subtree to be quiet, so the answer it carries must be the
 * last one, not the "I started a leaf" of a turn that ended first. A turn that
 * answered nothing leaves the answer it had. A link already reported is re-armed
 * once — and only when this turn was started by the wake from the target's own
 * child — so the real answer reaches the parent too; `answerChangedAt` keeps
 * that from repeating, so no wake loops.
 */
export function restateLinks(
  book: LinkBook,
  target: string,
  says: TurnAnswer,
  now: number,
  woken: boolean,
): LinkBook {
  return {
    ...book,
    links: book.links.map((link) => {
      if (link.target !== target) return link;
      if (link.state !== "done" && link.state !== "failed") return link;
      // A turn that answered nothing (its rows were applied later, or the harness said nothing)
      // leaves the link as it is: it must not erase the answer the parent is about to read.
      if (!says.text.trim()) return link;
      const replaced = {
        ...link,
        answer: says.text,
        truncated: says.truncated,
        filesChanged: says.filesChanged,
      };
      if (link.notifiedAt === undefined) return { ...replaced, settledAt: now };
      if (!woken || link.answerChangedAt !== undefined) return link;
      return { ...replaced, settledAt: now, notifiedAt: undefined, answerChangedAt: now };
    }),
  };
}

/**
 * A mark a wait set when it handed a link over: the field it wrote, so the one
 * mark can be taken back when the call that handed it over turns out to be gone.
 */
export interface HeardMark {
  id: string;
  notifiedAt?: number;
  askedRequest?: string;
}

/**
 * The book without the marks those waits set: the call that handed their links
 * over was dropped, so nobody read the answer it carried, and the ordinary wake
 * has to deliver it instead. Only the mark the wait itself wrote is cleared — a
 * newer one (another wait, or a wake) stands, so nothing is delivered twice.
 */
export function unheard(book: LinkBook, marks: readonly HeardMark[]): LinkBook {
  if (!marks.length) return book;
  const byId = new Map(marks.map((mark) => [mark.id, mark]));
  let changed = false;
  const links = book.links.map((link) => {
    const mark = byId.get(link.id);
    if (!mark) return link;
    if (mark.notifiedAt !== undefined) {
      if (link.notifiedAt !== mark.notifiedAt) return link;
      changed = true;
      const { notifiedAt: _cleared, ...rest } = link;
      return rest;
    }
    if (mark.askedRequest === undefined || link.askedRequest !== mark.askedRequest) return link;
    changed = true;
    const { askedRequest: _cleared, ...rest } = link;
    return rest;
  });
  return changed ? { ...book, links } : book;
}

/**
 * Settled links a newer link to the same target replaced: the parent messaged that thread again
 * since, so the older answer is out of date and its wake must not land after the newer one. Only
 * unreported links count: one the parent already heard about leaves nothing to fix.
 */
export function supersededLinks(book: LinkBook, ctx: WakeContext): Link[] {
  const newest = new Map<string, number>();
  const key = (link: Link) => `${link.orchestrator}\n${link.target}`;
  for (const link of book.links) {
    const at = newest.get(key(link)) ?? 0;
    if (link.createdAt > at) newest.set(key(link), link.createdAt);
  }
  return book.links.filter((link) => {
    const state = stateOfLink(link, ctx);
    if (state !== "done" && state !== "failed") return false;
    if (link.notifiedAt !== undefined) return false;
    return (newest.get(key(link)) ?? 0) > link.createdAt;
  });
}

/**
 * What to tell each parent, one message per parent: children that settled and
 * children waiting on the user. A link is reported once — the mark lives in the
 * book (`notifiedAt`, `askedRequest`), so a restart does not tell it again — and
 * a parent blocked on it in `wait_session` already has the answer. A settled link
 * a newer one replaced is not reported either ({@link supersededLinks}): the
 * answer the parent needs is the newer turn's. A thread that is itself a child is
 * woken the same way its top-level parent is: its children's results are its own
 * to read, and its parent hears from it when its own turn ends. A settled child
 * is reported only once its own subtree is quiet ({@link quietTree}): a middle
 * thread is not finished for its parent while the thread it started still works.
 * Parallel children are coalesced: while any of a parent's links still runs, a
 * settled one waits up to `WAKE_COALESCE_MS` for its siblings; with none active
 * the message goes at once.
 */
export function wakesDue(book: LinkBook, ctx: WakeContext): Wake[] {
  const byParent = new Map<string, Link[]>();
  for (const link of book.links) {
    const list = byParent.get(link.orchestrator);
    if (list) list.push(link);
    else byParent.set(link.orchestrator, [link]);
  }
  const stale = new Set(supersededLinks(book, ctx).map((l) => l.id));
  const out: Wake[] = [];
  for (const [parent, links] of byParent) {
    const here = ctx.threads[parent];
    if (!here) continue;
    const waiting = new Set(ctx.waiting[parent] ?? []);
    const active = links.some((l) => isActive(stateOfLink(l, ctx)));
    const settled = links.filter((link) => {
      const state = stateOfLink(link, ctx);
      if (state !== "done" && state !== "failed") return false;
      if (link.notifiedAt !== undefined || waiting.has(link.id)) return false;
      if (stale.has(link.id)) return false;
      if (!quietTree(book, ctx, link.target)) return false;
      return !active || ctx.now - (link.settledAt ?? 0) >= WAKE_COALESCE_MS;
    });
    const asking = links.filter((link) => {
      if (stateOfLink(link, ctx) !== "needs_user" || waiting.has(link.id)) return false;
      const request = ctx.pending[link.id];
      return request !== undefined && link.askedRequest !== request;
    });
    if (settled.length || asking.length) {
      out.push({ parent, parentTitle: here.title, settled, asking });
    }
  }
  return out;
}

/**
 * The book after a wake has been delivered: each reported link is marked heard
 * (and, for a question, with the request it was about) so no restart or tick
 * reports it twice.
 */
export function markWoken(book: LinkBook, wake: Wake, ctx: WakeContext, now: number): LinkBook {
  const settled = wake.settled.reduce((acc, l) => updateLink(acc, l.id, { notifiedAt: now }), book);
  return wake.asking.reduce(
    (acc, l) => updateLink(acc, l.id, { askedRequest: ctx.pending[l.id] ?? l.askedRequest }),
    settled,
  );
}

/** Settled links whose parent thread is gone: nothing will ever deliver them. */
export function orphanedLinks(book: LinkBook, ctx: WakeContext): Link[] {
  return book.links.filter(
    (link) =>
      !ctx.threads[link.orchestrator] &&
      link.notifiedAt === undefined &&
      (stateOfLink(link, ctx) === "done" || stateOfLink(link, ctx) === "failed"),
  );
}

/**
 * When a wake could next be due, for a timer to look again: the oldest settled
 * child still inside its coalescing window. Null when nothing waits.
 */
export function nextWakeAt(book: LinkBook, ctx: WakeContext): number | null {
  let next: number | null = null;
  const stale = new Set(supersededLinks(book, ctx).map((l) => l.id));
  for (const link of book.links) {
    const state = stateOfLink(link, ctx);
    if (state !== "done" && state !== "failed") continue;
    if (link.notifiedAt !== undefined || stale.has(link.id)) continue;
    if (!quietTree(book, ctx, link.target)) continue;
    const at = (link.settledAt ?? 0) + WAKE_COALESCE_MS;
    if (at <= ctx.now) continue;
    if (next === null || at < next) next = at;
  }
  return next;
}

/** A wake's compact first line, and the transcript's: "2 children finished · 1 needs you". */
export function wakeSummary(wake: Wake): string {
  const parts: string[] = [];
  if (wake.settled.length) {
    parts.push(`${wake.settled.length} ${wake.settled.length === 1 ? "child" : "children"} finished`);
  }
  if (wake.asking.length) {
    parts.push(`${wake.asking.length} ${wake.asking.length === 1 ? "needs" : "need"} you`);
  }
  return parts.join(" · ");
}

/** One child, as the wake message names it: link id, status, title, thread, what it changed. */
const linkLine = (link: Link, state: LinkState, now: number) => {
  const bits = [`link_id ${link.id}`, state, `“${link.targetTitle}”`, `session ${link.target}`];
  if (link.filesChanged) bits.push(`${link.filesChanged} file${link.filesChanged === 1 ? "" : "s"} changed`);
  if (link.settledAt) bits.push(`${Math.max(1, Math.round((now - link.settledAt) / 1000))}s ago`);
  return `- ${bits.join(" · ")}`;
};

/**
 * The message a woken parent gets: what finished, with each child's status,
 * title, link id, files and a bounded answer, and which children wait on the
 * user. The first line is the compact summary the transcript shows. It ends by
 * naming `read_session` / `wait_session` for the rest and says not to poll.
 */
export function wakeMessage(wake: Wake, now: number): string {
  const lines: string[] = [wakeSummary(wake), ""];
  if (wake.settled.length) {
    lines.push("Finished:", "");
    for (const link of wake.settled) {
      lines.push(linkLine(link, link.state, now));
      const { text, truncated } = capped((link.answer ?? "").trim(), WAKE_ANSWER_CHARS);
      if (!text) lines.push("  answer: (nothing it said came back)");
      else lines.push(`  answer: ${text.split("\n").join("\n  ")}${truncated ? " …" : ""}`);
    }
    lines.push("");
  }
  if (wake.asking.length) {
    lines.push("Waiting on you (the user answers these; you cannot):", "");
    for (const link of wake.asking) lines.push(linkLine(link, "needs_user", now));
    lines.push("", "list_pending_requests shows what it asks; ask the user if it is unclear.", "");
  }
  lines.push(
    "The rest is in read_session on a child's session id, or wait_session on a link_id (a settled link answers at once). This message is the notification: do not poll with sleeps.",
  );
  // The engine keeps the text, not the app's flag: the mark makes a wake turn a wake turn when its
  // history is replayed (state/thread.ts `isWakeMessage`), so it never reads as the user's message.
  return withWakeMark(lines.join("\n"));
}

/** What the user is asked, in words. */
export interface GrantAsk {
  kind: "message" | "read" | "start" | "fan_out" | "organize" | "interrupt" | "transfer" | "configure";
  caller: string;
  callerTitle: string;
  /** The thread messaged or read, or the new thread's title. */
  target: string;
  detail: string;
  project?: string;
  harnesses?: string[];
  /** The access a new thread starts at (`start`, `fan_out`): never more than the caller's own. */
  access?: PermissionMode;
  worktree?: boolean;
}

export type GrantChoice = "always" | "once" | "deny";
