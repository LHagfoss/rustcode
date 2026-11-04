// Threads that start and message other threads: the rules and records of
// orchestration M1 and v2 (docs/architecture/cross-session-orchestration.md,
// docs/orchestrator.md), after RustCodeKit's Orchestration.swift. The
// engine forwards an agent's tool call as a `_rustcode/orchestration`
// request; the app checks these rules, asks the user, and carries it out
// (state/host.ts). Pure: no store, no engine.

import type { Json, JsonObject } from "../acp/jsonrpc";
import type { ThreadStatus, Worktree } from "../acp/v2";
import { snoozeAt } from "./snooze";

/** What the client advertises in `initialize` while the setting is on. */
export const CAPABILITY_VERSION = 4;
export const MAX_BYTES = 8 * 1024;
export const READ_BYTES = 16 * 1024;
export const MAILBOX = 32;
export const MAX_ACTIVE_LINKS = 4;
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
  | { tool: "read_session"; target: string; lastTurns: number }
  | { tool: "message_session"; target: string; text: string }
  | { tool: "wait_session"; links: string[]; timeoutMs: number }
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
      return { tool: "read_session", target, lastTurns: n };
    }
    case "message_session": {
      const target = s(args.session_id) ?? "";
      const text = s(args.message) ?? "";
      if (!target || !text.trim()) return failure("invalid", "session_id and message are required");
      if (bytes(text) > MAX_BYTES) return failure("invalid", "the message is over 8 KiB");
      return { tool: "message_session", target, text };
    }
    case "wait_session": {
      const links = Array.isArray(args.link_ids)
        ? args.link_ids.flatMap((x) => (typeof x === "string" ? [x] : []))
        : [];
      const one = s(args.link_id);
      if (one && !links.includes(one)) links.unshift(one);
      if (!links.length) return failure("invalid", "link_id is required");
      const asked = typeof args.timeout_ms === "number" ? args.timeout_ms : DEFAULT_WAIT_MS;
      return { tool: "wait_session", links, timeoutMs: Math.min(Math.max(asked, 1), MAX_WAIT_MS) };
    }
    case "start_session": {
      const prompt = s(args.prompt) ?? "";
      if (!prompt.trim()) return failure("invalid", "prompt is required");
      if (bytes(prompt) > MAX_BYTES) return failure("invalid", "the prompt is over 8 KiB");
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
      if (bytes(prompt) > MAX_BYTES) return failure("invalid", "the prompt is over 8 KiB");
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

const activeFrom = (book: LinkBook, id: string) =>
  book.links.filter((l) => l.orchestrator === id && isActive(l.state));

/** How many more links a thread may open at once, for an answer to say. */
export const linksLeft = (book: LinkBook, id: string): number =>
  Math.max(0, MAX_ACTIVE_LINKS - activeFrom(book, id).length);

/** No chains: a thread working for another one cannot pass work on. */
function chained(book: LinkBook, caller: RuleThread): Failure | null {
  return book.links.some((l) => l.target === caller.id && isActive(l.state))
    ? failure("not_allowed", "this thread is working for another thread and cannot involve others")
    : null;
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
  const chain = chained(book, caller);
  if (chain) return chain;
  if (book.links.some((l) => l.target === target.id && l.orchestrator !== caller.id && isActive(l.state))) {
    return failure("target_linked", "another thread is already working with that one");
  }
  if (activeFrom(book, caller.id).length >= MAX_ACTIVE_LINKS) {
    return failure(
      "busy",
      `at most ${MAX_ACTIVE_LINKS} at once; wait_session on one first (this thread has ${MAX_ACTIVE_LINKS})`,
      0,
    );
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
export function checkStart(caller: RuleThread, book: LinkBook, count: number): Failure | null {
  const chain = chained(book, caller);
  if (chain) return chain;
  const active = activeFrom(book, caller.id).length;
  if (active + count > MAX_ACTIVE_LINKS) {
    const left = Math.max(0, MAX_ACTIVE_LINKS - active);
    return failure(
      "busy",
      `at most ${MAX_ACTIVE_LINKS} at once; wait_session on one first (this thread has ${active}, so ${count} would not fit, and ${left} ${left === 1 ? "slot is" : "slots are"} left)`,
      left,
    );
  }
  return null;
}

/** The message as the other thread receives it: who it is from, then the text. */
export const delivered = (text: string, from: string) => `From thread “${from}” (agent):\n\n${text}`;

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

/** A thread deleted: links from it are forgotten, links to it fail. */
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
  if (link.truncated) entry.truncated = true;
  if (link.filesChanged !== undefined) entry.files_changed = link.filesChanged;
  const activity = extra?.activity;
  if (activity) {
    entry.turns = activity.turns;
    if (activity.lastTool) entry.last_tool = activity.lastTool;
    if (activity.lastAt !== undefined && extra?.now !== undefined) {
      entry.idle_ms = Math.max(0, extra.now - activity.lastAt);
    }
  }
  if (state === "needs_user") entry.note = "That thread is waiting for the user to answer it there.";
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

/** What the decision needs of the app: states, pending requests, waits, parents. */
export interface WakeContext {
  /** Each link's state as shown (`needs_user` while its child waits on the user). */
  states: Readonly<Record<string, LinkState>>;
  /** Each child's pending request id, when it waits on one. */
  pending: Readonly<Record<string, string>>;
  /** The links each parent is blocked on in `wait_session` right now. */
  waiting: Readonly<Record<string, readonly string[]>>;
  /** The parents that still exist, with their title and whether their turn runs. */
  parents: Readonly<Record<string, { title: string; running: boolean }>>;
  now: number;
}

const stateOfLink = (link: Link, ctx: WakeContext) => ctx.states[link.id] ?? link.state;

/**
 * What to tell each parent, one message per parent: children that settled and
 * children waiting on the user. A link is reported once — the mark lives in the
 * book (`notifiedAt`, `askedRequest`), so a restart does not tell it again — and
 * a parent blocked on it in `wait_session` already has the answer. A thread
 * working for another is not woken (it cannot delegate, and its own parent is
 * the one to tell). Parallel children are coalesced: while any of a parent's
 * links still runs, a settled one waits up to `WAKE_COALESCE_MS` for its
 * siblings; with none active the message goes at once.
 */
export function wakesDue(book: LinkBook, ctx: WakeContext): Wake[] {
  const byParent = new Map<string, Link[]>();
  for (const link of book.links) {
    const list = byParent.get(link.orchestrator);
    if (list) list.push(link);
    else byParent.set(link.orchestrator, [link]);
  }
  const out: Wake[] = [];
  for (const [parent, links] of byParent) {
    const here = ctx.parents[parent];
    if (!here) continue;
    // A child works for another thread: its parent hears about it, not it.
    if (book.links.some((l) => l.target === parent && isActive(stateOfLink(l, ctx)))) continue;
    const waiting = new Set(ctx.waiting[parent] ?? []);
    const active = links.some((l) => isActive(stateOfLink(l, ctx)));
    const settled = links.filter((link) => {
      const state = stateOfLink(link, ctx);
      if (state !== "done" && state !== "failed") return false;
      if (link.notifiedAt !== undefined || waiting.has(link.id)) return false;
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
      !ctx.parents[link.orchestrator] &&
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
  for (const link of book.links) {
    const state = stateOfLink(link, ctx);
    if (state !== "done" && state !== "failed") continue;
    if (link.notifiedAt !== undefined) continue;
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
  return lines.join("\n");
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
  worktree?: boolean;
}

export type GrantChoice = "always" | "once" | "deny";
