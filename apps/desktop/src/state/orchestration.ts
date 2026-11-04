// Threads that start and message other threads: the rules and records of
// orchestration M1 and v2 (docs/architecture/cross-session-orchestration.md,
// docs/orchestrator.md), after RustCodeKit's Orchestration.swift. The
// engine forwards an agent's tool call as a `_rustcode/orchestration`
// request; the app checks these rules, asks the user, and carries it out
// (state/host.ts). Pure: no store, no engine.

import type { Json, JsonObject } from "../acp/jsonrpc";
import type { ThreadStatus, Worktree } from "../acp/v2";

/** What the client advertises in `initialize` while the setting is on. */
export const CAPABILITY_VERSION = 3;
export const MAX_BYTES = 8 * 1024;
export const READ_BYTES = 16 * 1024;
export const MAILBOX = 32;
export const MAX_ACTIVE_LINKS = 4;
export const DEFAULT_WAIT_MS = 60_000;
export const MAX_WAIT_MS = 3_600_000;
export const MAX_LAST_TURNS = 10;

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
}

export const failure = (code: string, message: string): Failure => ({ code, message });
export const failureJson = (f: Failure): JsonObject => ({ error: { code: f.code, message: f.message } });

export interface HarnessChoice {
  harness: string;
  model?: string;
  effort?: string;
}

export type Call =
  | { tool: "list_sessions"; query: string }
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
  | { tool: "search_sessions"; query: string }
  | { tool: "list_worktrees" }
  | {
      tool: "update_session";
      target?: string;
      title?: string;
      pinned?: boolean;
      unread?: boolean;
      status?: SessionStatusChoice;
    }
  | { tool: "interrupt_session"; target: string }
  | { tool: "configure_session"; target?: string; model?: string; effort?: string }
  | { tool: "list_projects" }
  | { tool: "list_pull_requests"; state: PullStateChoice };

/** What `update_session` may set a thread's status to. */
export type SessionStatusChoice = "active" | "review" | "settled";
const SESSION_STATUSES: readonly string[] = ["active", "review", "settled"];

/** What `list_pull_requests` may ask for. */
export type PullStateChoice = "open" | "closed" | "merged" | "all";
const PULL_STATES: readonly string[] = ["open", "closed", "merged", "all"];
/** Pull requests listed, and projects: an agent's context pays for each. */
export const MAX_PULL_REQUESTS = 30;
export const MAX_PROJECTS = 50;

const s = (v: Json | undefined) => (typeof v === "string" ? v : undefined);
const bytes = (text: string) => new TextEncoder().encode(text).length;

/** The call in a request's params, or the failure to answer with. */
export function parseCall(params: JsonObject): Call | Failure {
  const raw = params.args;
  const args = raw && typeof raw === "object" && !Array.isArray(raw) ? raw : {};
  switch (s(params.tool)) {
    case "list_sessions":
      return { tool: "list_sessions", query: s(args.query) ?? "" };
    case "list_harnesses":
      return { tool: "list_harnesses" };
    case "list_worktrees":
      return { tool: "list_worktrees" };
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
      return { tool: "search_sessions", query };
    }
    case "interrupt_session": {
      const target = s(args.session_id) ?? "";
      if (!target) return failure("invalid", "session_id is required");
      return { tool: "interrupt_session", target };
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
      if (!title && pinned === undefined && unread === undefined && !status) {
        return failure("invalid", "give at least one of title, pinned, unread or status");
      }
      return {
        tool: "update_session",
        target: s(args.session_id),
        title,
        pinned,
        unread,
        status: status as SessionStatusChoice | undefined,
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

export function listSessions(threads: RuleThread[], caller: RuleThread, query: string): JsonObject {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const shown = threads
    .filter((t) => t.id !== caller.id && sameProject(projectOf(t), projectOf(caller)))
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
      if (t.updatedAt) entry.updated_at = new Date(t.updatedAt).toISOString();
      if (t.worktree)
        entry.worktree = { path: t.worktree.path, branch: t.worktree.branch, repo: t.worktree.repo };
      return entry;
    }),
  };
}

const activeFrom = (book: LinkBook, id: string) =>
  book.links.filter((l) => l.orchestrator === id && isActive(l.state));

/** No chains: a thread working for another one cannot pass work on. */
function chained(book: LinkBook, caller: RuleThread): Failure | null {
  return book.links.some((l) => l.target === caller.id && isActive(l.state))
    ? failure("not_allowed", "this thread is working for another thread and cannot involve others")
    : null;
}

/** Whether `caller` may message (or read) `target`, before the user is asked. */
export function checkMessage(
  caller: RuleThread,
  target: RuleThread | undefined,
  book: LinkBook,
  read = false,
): Failure | null {
  if (!target) return failure("not_found", "no thread has that id; use list_sessions");
  if (target.id === caller.id)
    return failure("not_allowed", `a thread cannot ${read ? "read" : "message"} itself`);
  if (!sameProject(projectOf(target), projectOf(caller))) {
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
    return failure("busy", `this thread already has ${MAX_ACTIVE_LINKS} links under way; wait for one first`);
  }
  if (book.links.filter((l) => l.target === target.id && l.state === "queued").length >= MAILBOX) {
    return failure("mailbox_full", `that thread has ${MAILBOX} messages waiting`);
  }
  return null;
}

/** Whether `caller` may start `count` threads, before the user is asked. */
export function checkStart(caller: RuleThread, book: LinkBook, count: number): Failure | null {
  const chain = chained(book, caller);
  if (chain) return chain;
  const active = activeFrom(book, caller.id).length;
  if (active + count > MAX_ACTIVE_LINKS) {
    return failure(
      "busy",
      `this thread may have ${MAX_ACTIVE_LINKS} links under way and has ${active}; wait for one first`,
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

/** `wait_session`'s answer for one link. */
export function linkResult(link: Link, state: LinkState, status?: ThreadStatus): JsonObject {
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
  if (state === "needs_user") entry.note = "That thread is waiting for the user to answer it there.";
  return entry;
}

/** What the user is asked, in words. */
export interface GrantAsk {
  kind: "message" | "read" | "start" | "fan_out" | "organize" | "interrupt" | "configure";
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
