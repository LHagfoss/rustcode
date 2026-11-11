// Threads that start and message other threads: the rules and records of
// orchestration M1 and v2 (docs/architecture/cross-session-orchestration.md,
// docs/orchestrator.md), after RustCodeKit's Orchestration.swift. The
// engine forwards an agent's tool call as a `_rustcode/orchestration`
// request; the app checks these rules, asks the user, and carries it out
// (state/host.ts). Pure: no store, no engine.

import type { Json, JsonObject } from "../acp/jsonrpc";
import type { ThreadStatus, Worktree } from "../acp/v2";
import { MAX_ATTACHMENT_FILES } from "./attachments";
import type { PermissionMode } from "./permissions";
import { snoozeAt } from "./snooze";
import { formatDuration, type QueuedPrompt, withWakeMark } from "./thread";

/** What the client advertises in `initialize` while the setting is on. */
export const CAPABILITY_VERSION = 13;
/** The most of a child's answer `wait_session` carries; beyond it, `read_session` pages the rest. */
export const MAX_BYTES = 8 * 1024;
/** The longest prompt or message a thread may hand another one (32 KiB). */
export const MAX_MESSAGE_BYTES = 32 * 1024;
/** One `read_session` page. */
export const READ_BYTES = 16 * 1024;
export const MAILBOX = 32;
/**
 * The default number of delegates one thread may have working for it at once
 * (Settings › Harnesses). A message to a thread it already has a link to is a
 * follow-up, not another slot.
 */
export const DEFAULT_DELEGATES_PER_THREAD = 8;
export const MIN_DELEGATES_PER_THREAD = 2;
export const MAX_DELEGATES_PER_THREAD = 32;
/**
 * The default number of delegates that may run at once across this app
 * instance (Settings › Harnesses), whichever thread started them. A delegate
 * takes one slot whether it is a child this thread started or a thread it
 * messaged.
 */
export const DEFAULT_DELEGATES_TOTAL = 16;
export const MIN_DELEGATES_TOTAL = 2;
export const MAX_DELEGATES_TOTAL = 64;
/**
 * The two settings the slot rules read: one thread's own limit and the
 * machine-wide total. Both count live delegates only, so a finished one frees
 * its slot.
 */
export interface DelegateLimits {
  perThread: number;
  total: number;
}
export const DEFAULT_DELEGATE_LIMITS: DelegateLimits = {
  perThread: DEFAULT_DELEGATES_PER_THREAD,
  total: DEFAULT_DELEGATES_TOTAL,
};
/**
 * The per-thread setting as it can actually run: the total may be the lower
 * cap, so a row that shows only its own number would promise more than fits.
 */
export const effectivePerThreadLimit = (limits: DelegateLimits): number =>
  Math.min(limits.perThread, limits.total);
/**
 * How deep delegation goes: a top-level thread (0) may start children (1), and
 * they may start grandchildren (2). A thread at the limit is told so, with the
 * limit in the reason, instead of being told it cannot delegate at all.
 */
export const MAX_DEPTH = 2;
/**
 * A wait returns as soon as a link finishes, so a long default costs nothing; a short one makes the
 * orchestrator ask again, and each ask resends its whole context (the engine's `DEFAULT_WAIT_MS`).
 */
export const DEFAULT_WAIT_MS = 20 * 60 * 1000;
export const MAX_WAIT_MS = 25 * 60 * 1000;
export const MAX_LAST_TURNS = 10;
/** How long a settled child waits for its siblings before the parent hears about them. */
export const WAKE_COALESCE_MS = 3_000;
/**
 * How long a child's running turn may show no new activity before its parent is
 * told it looks stuck. A command with no timeout (a git editor with no TTY, a
 * script whose timers never end) can hold a turn for hours, so this is the
 * point where an advisory notice is worth the interruption.
 */
export const STALL_NOTICE_MS = 10 * 60_000;
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

/**
 * A link's state as the app shows it now, with its target thread's own reading of
 * what it does (`activity`, the engine's status). The thread's live word wins over
 * the link's record: a link says what it was doing when it settled, or what a
 * restart could not resume (`interrupted`), while its thread may have kept
 * working. A link the user stopped, or one that failed, keeps its word: that
 * branch's work did not resume. One rule, so the strip, the delegate tree, the
 * popover and the sidebar read a link the same way.
 */
export function liveState(link: Link, activity?: ThreadStatus): LinkState {
  if (link.state === "failed" || link.state === "cancelled") return link.state;
  if (activity === "needs_input") return "needs_user";
  if (activity === "running") return "working";
  return link.state;
}

/**
 * One thing that happened to a link, and when: the thread started, the message
 * sent, the turn that began, the question it asked, the answer or the failure,
 * and a merge or a cleanup of its branch. The chip shows the last one as its
 * status and lists them all under its hover tip (`ui/AgentViews.tsx`).
 */
export type LinkEventKind =
  | "started"
  | "message"
  | "queued"
  | "working"
  | "asking"
  | "answered"
  | "failed"
  | "stopped"
  | "interrupted"
  | "merged"
  | "cleaned"
  | "handed_off";

export type LinkEvent = {
  kind: LinkEventKind;
  at: number;
  /** What the word gains: the harness and model it runs on, or the time it worked. */
  detail?: string;
};

/** The events one link keeps: enough for a branch's whole life, bounded for the saved book. */
export const MAX_LINK_EVENTS = 12;

/** The chip's word for each event; the link's state words are {@link LINK_LABEL}. */
export const LINK_EVENT_LABEL: Record<LinkEventKind, string> = {
  started: "Started",
  message: "Message sent",
  queued: "Queued",
  working: "Working",
  asking: "Asked you",
  answered: "Answered",
  failed: "Failed",
  stopped: "Stopped",
  interrupted: "Interrupted",
  merged: "Merged",
  cleaned: "Cleaned up",
  handed_off: "Handed off",
};

/** The event a link reaches when it newly takes a state. */
const EVENT_OF_STATE: Partial<Record<LinkState, LinkEventKind>> = {
  queued: "queued",
  working: "working",
  needs_user: "asking",
  done: "answered",
  failed: "failed",
  cancelled: "stopped",
  interrupted: "interrupted",
};

/** One event's identity: its kind at one moment. Two of them are the same event. */
const eventKey = (kind: LinkEventKind, at: number) => `${kind}@${at}`;

/** `link` with one more event, keeping the last {@link MAX_LINK_EVENTS}. */
export function noteEvent(link: Link, kind: LinkEventKind, at: number, detail?: string): Link {
  const events = link.events ?? [];
  // A moment already in the list is one event, wherever it sits: a state set again is not news, and
  // a repeat must not push a real event out of the bounded list (`working, asking, working` at one
  // moment is two rows, not three).
  const key = eventKey(kind, at);
  if (events.some((event) => eventKey(event.kind, event.at) === key)) return link;
  const next = [...events, { kind, at, ...(detail ? { detail } : {}) }];
  return { ...link, events: next.slice(-MAX_LINK_EVENTS) };
}

/**
 * The link after `change`, with an event for a state it newly reaches. Every
 * place that settles or resumes a link goes through this, so the chip's list
 * says the same thing whichever turn wrote the change.
 */
export function advanceLink(link: Link, change: Partial<Link>, at: number): Link {
  const next = { ...link, ...change };
  if (change.state === undefined || change.state === link.state) return next;
  const kind = EVENT_OF_STATE[change.state];
  if (!kind) return next;
  const worked = kind === "answered" ? workedTime(next, at) : undefined;
  const detail = worked ? `worked ${formatDuration(worked)}` : undefined;
  return noteEvent(next, kind, at, detail);
}

/**
 * How long a link actually worked: the turn's span less the time it spent
 * waiting on the user, which the app tracks apart (`waitedMs`, plus a wait
 * still open in `waitingSince`) so a five-minute approval is not five minutes
 * of work in the event that reports the answer.
 */
export function workedTime(link: Link, at: number): number | undefined {
  if (link.workedMs === undefined) return undefined;
  const waited =
    (link.waitedMs ?? 0) + (link.waitingSince === undefined ? 0 : Math.max(0, at - link.waitingSince));
  return Math.max(0, link.workedMs - waited);
}

/** An event's word with what it adds: `Answered · worked 2m 10s`. */
export const linkEventText = (event: LinkEvent): string =>
  event.detail ? `${LINK_EVENT_LABEL[event.kind]} · ${event.detail}` : LINK_EVENT_LABEL[event.kind];

/**
 * The chip's status word: the latest event's, or its state's for a link made
 * before the app kept events. `state` is the link as it shows now
 * ({@link liveState}): `needs_user` while its child waits on an approval even
 * though the link itself still says `working`, `working` while the thread it was
 * severed from keeps working, and the event's word only when nothing live speaks.
 */
export function linkStatusText(link: Link, state: LinkState = link.state): string {
  const last = link.events?.at(-1);
  // A wait reads as the question it asked, whether or not the book kept that event.
  if (state === "needs_user")
    return last?.kind === "asking" ? LINK_EVENT_LABEL.asking : LINK_LABEL.needs_user;
  // A live state outranks the last event: the `interrupted` mark a restart writes, or the answer of
  // a turn that ended, is not what the link does now.
  if (isActive(state)) return LINK_LABEL[state];
  // The wait ended without its own event (a book from before the list): the state's word stands.
  if (last?.kind === "asking") return LINK_LABEL[state];
  return last ? LINK_EVENT_LABEL[last.kind] : LINK_LABEL[state];
}

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
 * a thread it only messaged reads here. `states` is what each link shows now
 * ({@link liveState}, `state/host.ts` `stateOf`), so a link a restart marked
 * `interrupted` whose thread kept working still has its live row here.
 */
export function stripLinks(
  links: readonly Link[],
  shown: ReadonlySet<string>,
  states?: readonly LinkState[],
): Link[] {
  return links.filter((link, i) => isActive(states?.[i] ?? link.state) || !shown.has(link.target));
}

/** How many settled delegates the strip's popover lists under "Done"; the rest are one count. */
export const DONE_SHOWN = 10;

/**
 * The delegates of `orchestrator` that settled, one row per thread (a follow-up
 * is the same delegate), newest first, and how many more there are than `cap`.
 * The strip has one line for the live work; this is the history it unfolds
 * under "Done", without a day of delegating turned into 34 rows.
 */
export function doneLinks(
  links: readonly Link[],
  orchestrator: string,
  cap = DONE_SHOWN,
): { shown: Link[]; more: number } {
  const settledAt = (link: Link) => link.settledAt ?? link.createdAt;
  const newest = new Map<string, Link>();
  for (const link of links) {
    if (link.orchestrator !== orchestrator || isActive(link.state)) continue;
    const seen = newest.get(link.target);
    if (!seen || settledAt(link) > settledAt(seen)) newest.set(link.target, link);
  }
  const all = [...newest.values()].sort((one, other) => settledAt(other) - settledAt(one));
  const most = Math.max(0, cap);
  return { shown: all.slice(0, most), more: Math.max(0, all.length - most) };
}

/** One thread in a thread's links: the link that speaks for it now, and every message. */
export interface LinkThread {
  /** The thread the row opens. */
  target: string;
  /** The link that speaks for it now: live first, else the newest. */
  link: Link;
  /** What that link shows now, from the caller's reading or its own record. */
  state: LinkState;
  /** Every link to it, in the order given: a follow-up is the same delegate. */
  links: Link[];
}

/**
 * The threads `links` name, one row each, however many times one was messaged. A
 * follow-up is the same delegate: its row takes a live link when it has one, so
 * a settled message cannot make a busy thread read answered, and otherwise the
 * newest, so the row shows the latest state and time.
 */
export function linkThreads(links: readonly Link[], states?: readonly LinkState[]): LinkThread[] {
  const groups = new Map<string, { links: Link[]; states: LinkState[] }>();
  links.forEach((link, i) => {
    const state = states?.[i] ?? link.state;
    const group = groups.get(link.target);
    if (group) {
      group.links.push(link);
      group.states.push(state);
    } else {
      groups.set(link.target, { links: [link], states: [state] });
    }
  });
  return [...groups].map(([target, group]) => {
    let best = 0;
    for (let i = 1; i < group.links.length; i++) {
      const live = isActive(group.states[i]!);
      const bestLive = isActive(group.states[best]!);
      if (live !== bestLive) {
        if (live) best = i;
        continue;
      }
      if (group.links[i]!.createdAt >= group.links[best]!.createdAt) best = i;
    }
    return {
      target,
      link: group.links[best]!,
      state: group.states[best]!,
      links: group.links,
    };
  });
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
  /** The harness's display name, as the notice writes it (Pi, Codex, Claude Code). */
  harnessName?: string;
  /** The paired machine the target runs on; absent for this computer. */
  machine?: string;
  /** The named login the thread runs on, when it is not the harness's default. */
  account?: string;
  /** The model it runs on, for the first event's `harness · model`. */
  model?: string;
  /** What happened to this link, oldest first: the chip shows the last as its status. */
  events?: LinkEvent[];
  fanOut?: string;
  answer?: string;
  /** Why a turn failed, apart from the reply it gave: a failed row's preview is this, not the reply. */
  error?: string;
  truncated?: boolean;
  filesChanged?: number;
  createdAt: number;
  /** When the turn this link waits on started working, for the notice's time. */
  startedAt?: number;
  /**
   * When a restart severed this link and the app resumed its target (`store.ts`
   * `resumeInterrupted`): the turn it settles on began there, and the notice says
   * so (`resumed after a restart`).
   */
  resumedAt?: number;
  /** How long that turn worked, from its start to the link settling. Absent when not recorded. */
  workedMs?: number;
  /** When it settled (done or failed), for the coalescing window. */
  settledAt?: number;
  /**
   * When the approval it waits on now was asked: the delegate tree's clock stops
   * there, so a question left open costs the branch no time.
   */
  waitingSince?: number;
  /** How long it has waited on approvals before this one, which its work time leaves out. */
  waitedMs?: number;
  /** When the parent was told it settled: one delivery per link, kept in the book across restarts. */
  notifiedAt?: number;
  /** The child's pending request (its rpc id, as text) the parent was told about. */
  askedRequest?: string;
  /**
   * When a reported link's answer was replaced by the turn its own child's wake
   * started: the parent is told once more, and never a third time.
   */
  answerChangedAt?: number;
  /**
   * The child's activity timestamp the parent's stall notice covered: the same
   * silence is reported once, and a newer activity arms another notice.
   */
  stallNotifiedFor?: number;
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
  /** Named login of that harness (`HarnessAccount.id`); omitted inherits it. */
  account?: string;
}

export type Call =
  | { tool: "list_sessions"; query: string; allProjects: boolean; machine?: string }
  | { tool: "read_session"; target: string; lastTurns: number; offset: number; machine?: string }
  /** `attachments` are absolute paths, under the caller's project or worktree. */
  | { tool: "message_session"; target: string; text: string; attachments: string[]; machine?: string }
  /** `callId` is the engine's name for this call, so a dropped call can end its wait. */
  | { tool: "wait_session"; links: string[]; timeoutMs: number; callId?: string; machine?: string }
  | {
      tool: "start_session";
      prompt: string;
      title?: string;
      harness?: string;
      model?: string;
      effort?: string;
      /** Named login of that harness; omitted is the caller's own, else a new thread's default. */
      account?: string;
      worktree?: boolean;
      project?: string;
      /** The paired machine it runs on; omitted is this computer. */
      machine?: string;
    }
  | {
      tool: "fan_out";
      prompt: string;
      title?: string;
      harnesses: HarnessChoice[];
      /** The project on that machine; without one, the caller's path there or its only project. */
      project?: string;
      machine?: string;
    }
  /** `harness` narrows to one harness, `model` to the models whose id or name matches. */
  | { tool: "list_harnesses"; harness?: string; model?: string; machine?: string }
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
  | { tool: "interrupt_session"; target: string; machine?: string }
  | { tool: "configure_session"; target?: string; model?: string; effort?: string }
  | { tool: "transfer_session"; target: string; harness: string }
  | { tool: "list_pending_requests"; machine?: string }
  | {
      tool: "answer_request";
      target: string;
      requestId: string;
      /** For an approval: how far the answer reaches. */
      choice?: ApprovalChoice;
      /** For a question: free text, or the id of one of its options. */
      answer?: string;
    }
  | {
      tool: "organize_threads";
      targets: string[];
      title?: string;
      pinned?: boolean;
      unread?: boolean;
      /** `true` settles the threads, `false` brings them back. */
      archived?: boolean;
    }
  | { tool: "list_projects"; machine?: string }
  | { tool: "list_pull_requests"; state: PullStateChoice }
  | { tool: "thread_usage"; since?: string; groupBy: UsageGroup; project?: string }
  | { tool: "query_data"; sql: string; limit: number }
  | { tool: "read_activity"; target: string; after?: string; limit: number; order?: "newest" }
  | {
      tool: "manage_queue";
      target: string;
      action: QueueAction;
      /** One of this thread's own queued messages, from `list`. */
      queueId?: string;
      /** What it says instead, for `edit`. */
      text?: string;
    }
  | { tool: "thread_status"; target?: string; machine?: string }
  | { tool: "merge_when_green"; target: string; timeoutMs: number; callId?: string }
  | { tool: "cleanup_worktree"; target: string; settle: boolean }
  /** Where worktrees stand: one thread's, or every worktree this thread reaches. */
  | { tool: "worktree_status"; target?: string; allProjects: boolean }
  /** Move a thread's work between the user's checkout and a worktree of its own. */
  | { tool: "handoff_worktree"; target: string; to: HandoffTarget }
  /** The computers this RustCode is paired with (orchestration version 12). */
  | { tool: "list_machines" }
  | { tool: "read_project"; path: string }
  | { tool: "create_project"; path: string; init?: boolean }
  | { tool: "clone_project"; url: string; parent?: string; name?: string }
  | {
      tool: "update_project";
      path: string;
      name?: string;
      harness?: string;
      model?: string;
    }
  | { tool: "remove_project"; path: string }
  | {
      tool: "schedule_task";
      prompt: string;
      schedule: TaskSchedule;
      project?: string;
      name?: string;
      modelProfile?: string;
    }
  | { tool: "list_scheduled_tasks" }
  | { tool: "delete_scheduled_task"; id: string }
  | { tool: "run_scheduled_task_now"; id: string };

/** What `update_session` may set a thread's status to. */
export type SessionStatusChoice = "active" | "review" | "settled";
const SESSION_STATUSES: readonly string[] = ["active", "review", "settled"];

/** What `answer_request` may answer an approval with. */
export type ApprovalChoice = "allow_once" | "allow_always" | "deny";
const APPROVAL_CHOICES: readonly string[] = ["allow_once", "allow_always", "deny"];
/** Threads one `organize_threads` call changes. */
export const MAX_ORGANIZED_THREADS = 50;

/** What `list_pull_requests` may ask for. */
export type PullStateChoice = "open" | "closed" | "merged" | "all";
const PULL_STATES: readonly string[] = ["open", "closed", "merged", "all"];
/** How `thread_usage` groups its rows. */
export type UsageGroup = "harness" | "model" | "day" | "project" | "thread";
const USAGE_GROUPS: readonly string[] = ["harness", "model", "day", "project", "thread"];
/** Rows `query_data` may bring back in one answer. */
export const MAX_DATA_ROWS = 1000;
/** Items `read_activity` returns by default, and at most. */
export const DEFAULT_ACTIVITY_ITEMS = 50;
export const MAX_ACTIVITY_ITEMS = 200;

/** Where `handoff_worktree` may move a thread's work. */
export type HandoffTarget = "checkout" | "worktree";
const HANDOFF_TARGETS: readonly string[] = ["checkout", "worktree"];

/** What `manage_queue` may do to a message this thread queued on another one. */
export type QueueAction = "list" | "edit" | "cancel" | "steer";
const QUEUE_ACTIONS: readonly string[] = ["list", "edit", "cancel", "steer"];
/** Queued messages one `manage_queue` list carries: a queue holds a handful. */
export const MAX_QUEUE_ITEMS = 50;
/** How much of a queued message a list shows: enough to know which one it is. */
export const QUEUE_PREVIEW_CHARS = 200;

/** What `schedule_task` may ask the scheduler for: a cron, or one time. */
export type TaskSchedule =
  | { kind: "cron"; expression: string; timezone?: string }
  | { kind: "at"; at: string };

/** How long `merge_when_green` waits for checks by default, and at most: 25 minutes a call, like `wait_session`. */
export const DEFAULT_MERGE_MS = MAX_WAIT_MS;
export const MAX_MERGE_MS = MAX_WAIT_MS;

/** Pull requests listed, and projects: an agent's context pays for each. */
export const MAX_PULL_REQUESTS = 30;
export const MAX_PROJECTS = 50;
/** Delegates `thread_status` reports in one answer: one gh read each, run together. */
export const MAX_DELEGATES = 12;
/** Worktree threads `worktree_status` reports in one answer. */
export const MAX_WORKTREES = 20;
/** Pull requests `worktree_status` reads: one gh read each, run together, capped like `thread_status`. */
export const MAX_WORKTREE_PULLS = MAX_DELEGATES;

const s = (v: Json | undefined) => (typeof v === "string" ? v : undefined);
/** `all_projects`: the user's threads in every project, when the setting allows it. */
const allProjects = (args: JsonObject) => args.all_projects === true;
const bytes = (text: string) => new TextEncoder().encode(text).length;
/** The paired machine a call acts on: an id or name `list_machines` gave, absent for this one. */
const machineOf = (args: JsonObject) => s(args.machine)?.trim() || undefined;
/** The optional field, present only when the call named a machine. */
const machineField = (args: JsonObject) => {
  const machine = machineOf(args);
  return machine ? { machine } : {};
};
/**
 * A list of paths, at most `max`: the strings that are one, or null when the
 * value is not a list of paths within it.
 */
function pathsOf(value: Json | undefined, max = MAX_ATTACHMENT_FILES): string[] | null {
  if (value === undefined || value === null) return [];
  if (!Array.isArray(value) || value.length > max) return null;
  const paths = value.flatMap((one) => (typeof one === "string" && one.trim() ? [one.trim()] : []));
  return paths.length === value.length ? paths : null;
}

/** The call in a request's params, or the failure to answer with. */
export function parseCall(params: JsonObject): Call | Failure {
  const raw = params.args;
  const args = raw && typeof raw === "object" && !Array.isArray(raw) ? raw : {};
  const tool = s(params.tool) ?? "";
  switch (tool) {
    case "list_sessions":
      return {
        tool: "list_sessions",
        query: s(args.query) ?? "",
        allProjects: allProjects(args),
        ...machineField(args),
      };
    case "list_harnesses": {
      const harness = s(args.harness)?.trim() || undefined;
      const model = s(args.model)?.trim() || undefined;
      return {
        tool: "list_harnesses",
        ...(harness ? { harness } : {}),
        ...(model ? { model } : {}),
        ...machineField(args),
      };
    }
    case "list_worktrees":
      return { tool: "list_worktrees", allProjects: allProjects(args) };
    case "worktree_status": {
      const target = s(args.session_id)?.trim() || undefined;
      return {
        tool: "worktree_status",
        ...(target ? { target } : {}),
        allProjects: allProjects(args),
      };
    }
    case "handoff_worktree": {
      const target = s(args.session_id) ?? "";
      if (!target) return failure("invalid", "session_id is required");
      // The direction an orchestrator means when it says "the user takes over"
      // is the checkout, so it is the default.
      const to = s(args.to) ?? "checkout";
      if (!HANDOFF_TARGETS.includes(to)) {
        return failure("invalid", `to must be one of ${HANDOFF_TARGETS.join(", ")}`);
      }
      return { tool: "handoff_worktree", target, to: to as HandoffTarget };
    }
    case "list_projects":
      return { tool: "list_projects", ...machineField(args) };
    case "list_machines":
      return { tool: "list_machines" };
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
      return { tool: "interrupt_session", target, ...machineField(args) };
    }
    case "transfer_session": {
      const target = s(args.session_id) ?? "";
      const harness = s(args.harness) ?? "";
      if (!target || !harness) return failure("invalid", "session_id and harness are required");
      return { tool: "transfer_session", target, harness };
    }
    case "list_pending_requests":
      return { tool: "list_pending_requests", ...machineField(args) };
    case "answer_request": {
      const target = s(args.session_id) ?? "";
      // The list prints `request_id` as a string, and the engine refuses anything else
      // (`orchestration.rs`: a name is a non-empty string), so a number is refused here too.
      const requestId = (s(args.request_id) ?? "").trim();
      if (!target || !requestId) {
        return failure(
          "invalid",
          "session_id and request_id (as list_pending_requests prints it) are required",
        );
      }
      const choice = s(args.choice);
      const answer = s(args.answer);
      if (choice && !APPROVAL_CHOICES.includes(choice)) {
        return failure("invalid", `choice must be one of ${APPROVAL_CHOICES.join(", ")}`);
      }
      if (choice && answer) {
        return failure("invalid", "give choice for an approval or answer for a question, not both");
      }
      if (!choice && !answer?.trim()) {
        return failure("invalid", "choice (an approval) or answer (a question) is required");
      }
      return {
        tool: "answer_request",
        target,
        requestId,
        ...(choice ? { choice: choice as ApprovalChoice } : {}),
        ...(answer ? { answer: answer.trim() } : {}),
      };
    }
    case "organize_threads": {
      const targets = pathsOf(args.session_ids, MAX_ORGANIZED_THREADS);
      if (!targets?.length) {
        return failure("invalid", `session_ids must be 1 to ${MAX_ORGANIZED_THREADS} thread ids`);
      }
      const title = s(args.title)?.trim();
      const pinned = typeof args.pinned === "boolean" ? args.pinned : undefined;
      const unread = typeof args.unread === "boolean" ? args.unread : undefined;
      const archived = typeof args.archived === "boolean" ? args.archived : undefined;
      if (!title && pinned === undefined && unread === undefined && archived === undefined) {
        return failure("invalid", "give at least one of title, pinned, unread or archived");
      }
      return {
        tool: "organize_threads",
        targets: [...new Set(targets)],
        title: title || undefined,
        pinned,
        unread,
        archived,
      };
    }
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
    case "thread_status": {
      const target = s(args.session_id)?.trim() || undefined;
      return { tool: "thread_status", target, ...machineField(args) };
    }
    case "merge_when_green": {
      const target = s(args.session_id) ?? "";
      if (!target) return failure("invalid", "session_id is required");
      const asked = typeof args.timeout_ms === "number" ? Math.floor(args.timeout_ms) : DEFAULT_MERGE_MS;
      return {
        tool: "merge_when_green",
        target,
        timeoutMs: Math.min(Math.max(asked, 1), MAX_MERGE_MS),
        callId: s(params.callId),
      };
    }
    case "cleanup_worktree": {
      const target = s(args.session_id) ?? "";
      if (!target) return failure("invalid", "session_id is required");
      // Settling after cleanup is the default; a caller can keep the thread as it is.
      return {
        tool: "cleanup_worktree",
        target,
        settle: typeof args.settle === "boolean" ? args.settle : true,
      };
    }
    case "read_activity": {
      const target = s(args.session_id) ?? "";
      if (!target) return failure("invalid", "session_id is required");
      const after = s(args.after)?.trim() || undefined;
      if (args.order != null && args.order !== "oldest" && args.order !== "newest") {
        return failure("invalid", "order must be oldest or newest");
      }
      const asked = typeof args.limit === "number" ? Math.floor(args.limit) : DEFAULT_ACTIVITY_ITEMS;
      // Clamped, not refused: an agent asking for 500 items gets the 200 cap.
      return {
        tool: "read_activity",
        target,
        after,
        ...(args.order === "newest" ? { order: "newest" as const } : {}),
        limit: Math.min(Math.max(asked, 1), MAX_ACTIVITY_ITEMS),
      };
    }
    case "manage_queue": {
      const target = s(args.session_id) ?? "";
      if (!target) return failure("invalid", "session_id is required");
      const action = s(args.action) ?? "";
      if (!QUEUE_ACTIONS.includes(action)) {
        return failure("invalid", `action must be one of ${QUEUE_ACTIONS.join(", ")}`);
      }
      if (action === "list") return { tool: "manage_queue", target, action: action as QueueAction };
      const queueId = (s(args.queue_id) ?? "").trim();
      if (!queueId) return failure("invalid", `queue_id is required to ${action}`);
      const text = s(args.text) ?? "";
      if (action === "edit") {
        if (!text.trim()) return failure("invalid", "text is required to edit");
        if (bytes(text) > MAX_MESSAGE_BYTES) return failure("invalid", "the message is over 32 KiB");
        return { tool: "manage_queue", target, action: action as QueueAction, queueId, text };
      }
      return { tool: "manage_queue", target, action: action as QueueAction, queueId };
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
      return { tool: "read_session", target, lastTurns: n, offset, ...machineField(args) };
    }
    case "message_session": {
      const target = s(args.session_id) ?? "";
      const text = s(args.message) ?? "";
      if (!target || !text.trim()) return failure("invalid", "session_id and message are required");
      if (bytes(text) > MAX_MESSAGE_BYTES) return failure("invalid", "the message is over 32 KiB");
      const attachments = pathsOf(args.attachments);
      if (attachments === null) {
        return failure("invalid", `attachments must be at most ${MAX_ATTACHMENT_FILES} absolute paths`);
      }
      return { tool: "message_session", target, text, attachments, ...machineField(args) };
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
        ...machineField(args),
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
        account: s(args.account)?.trim() || undefined,
        worktree: typeof args.worktree === "boolean" ? args.worktree : undefined,
        project: s(args.project),
        ...machineField(args),
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
        harnesses.push({
          harness,
          model: s(o.model),
          effort: s(o.effort),
          account: s(o.account)?.trim() || undefined,
        });
      }
      if (harnesses.length < 2 || harnesses.length > 4)
        return failure("invalid", "harnesses takes 2 to 4 entries");
      return {
        tool: "fan_out",
        prompt,
        title: s(args.title)?.trim() || undefined,
        harnesses,
        project: s(args.project),
        ...machineField(args),
      };
    }
    case "read_project":
    case "remove_project": {
      const path = (s(args.path) ?? "").trim();
      if (!path) return failure("invalid", "path is required");
      return tool === "read_project" ? { tool: "read_project", path } : { tool: "remove_project", path };
    }
    case "create_project": {
      const path = (s(args.path) ?? "").trim();
      if (!path) return failure("invalid", "path is required");
      return {
        tool: "create_project",
        path,
        init: typeof args.init === "boolean" ? args.init : undefined,
      };
    }
    case "clone_project": {
      const url = (s(args.url) ?? "").trim();
      if (!url) return failure("invalid", "url is required");
      return {
        tool: "clone_project",
        url,
        parent: s(args.parent)?.trim() || undefined,
        name: s(args.name)?.trim() || undefined,
      };
    }
    case "update_project": {
      const path = (s(args.path) ?? "").trim();
      if (!path) return failure("invalid", "path is required");
      const change = {
        name: s(args.name)?.trim() || undefined,
        harness: s(args.harness)?.trim() || undefined,
        model: s(args.model)?.trim() || undefined,
      };
      if (!change.name && !change.harness && !change.model) {
        return failure("invalid", "give at least one of name, harness or model");
      }
      return { tool: "update_project", path, ...change };
    }
    case "schedule_task": {
      const prompt = s(args.prompt) ?? "";
      if (!prompt.trim()) return failure("invalid", "prompt is required");
      if (bytes(prompt) > MAX_MESSAGE_BYTES) return failure("invalid", "the prompt is over 32 KiB");
      const raw =
        args.schedule && typeof args.schedule === "object" && !Array.isArray(args.schedule)
          ? (args.schedule as Record<string, unknown>)
          : {};
      const schedule = parseSchedule(raw);
      if (typeof schedule === "string") return failure("invalid", schedule);
      return {
        tool: "schedule_task",
        prompt,
        schedule,
        project: s(args.project)?.trim() || undefined,
        name: s(args.name)?.trim() || undefined,
        modelProfile: s(args.model_profile)?.trim() || undefined,
      };
    }
    case "list_scheduled_tasks":
      return { tool: "list_scheduled_tasks" };
    case "delete_scheduled_task":
    case "run_scheduled_task_now": {
      const id = (s(args.id) ?? "").trim();
      if (!id) return failure("invalid", "id is required");
      return tool === "delete_scheduled_task"
        ? { tool: "delete_scheduled_task", id }
        : { tool: "run_scheduled_task_now", id };
    }
    default:
      return failure("invalid", "unknown tool");
  }
}

/**
 * `schedule_task`'s schedule, checked here the way the engine checks it: a
 * five-field cron for a repeating task, an ISO 8601 time with its offset for
 * one that runs once.
 */
export function parseSchedule(raw: Record<string, unknown>): TaskSchedule | string {
  const kind = typeof raw.kind === "string" ? raw.kind : "";
  if (kind === "cron") {
    const expression = typeof raw.expression === "string" ? raw.expression.trim() : "";
    if (expression.split(/\s+/).filter(Boolean).length !== 5) {
      return "schedule.expression must be a five-field cron expression";
    }
    const timezone = typeof raw.timezone === "string" ? raw.timezone.trim() : "";
    return { kind: "cron", expression, ...(timezone ? { timezone } : {}) };
  }
  if (kind === "at") {
    const at = typeof raw.at === "string" ? raw.at.trim() : "";
    if (!at || Number.isNaN(Date.parse(at))) {
      return "schedule.at must be an ISO 8601 time with its offset";
    }
    return { kind: "at", at };
  }
  return "schedule.kind must be cron or at";
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
  /** The repository `folder` belongs to, as the engine resolved it (`_meta.rustcode.repo`). */
  repo?: string;
  /** The thread that started this one (`_meta.rustcode.parent`), durable past a pruned link. */
  parent?: string;
  updatedAt?: number;
  running: boolean;
  harness?: string;
  status?: ThreadStatus;
}

// macOS links /tmp, /var and /etc into /private, and the engine reports the resolved path: one place.
export const cleanProjectPath = (path: string) =>
  path
    .replace(/\/+$/, "")
    .replace(/\/\.\//g, "/")
    .replace(/^\/private(?=\/(?:tmp|var|etc)(?:\/|$))/, "");

const clean = cleanProjectPath;

/** A thread's project: its worktree's repository, else its folder. */
export const projectOf = (t: Pick<RuleThread, "folder" | "worktree">) => clean(t.worktree?.repo || t.folder);

/** Two folders are one project when they are the same place. */
export const sameProject = (a: string, b: string) => !!a && clean(a) === clean(b);

/**
 * The reach a thread's tools have: its repository, where a main checkout and
 * every worktree of it meet, else its folder. `projectOf` stays the stored
 * project — a worktree's own path — which `thread_usage` and `list_projects`
 * report, so this is the wider scope only the orchestration tools compare.
 */
export const reachOf = (t: Pick<RuleThread, "folder" | "worktree" | "repo">) =>
  clean(t.worktree?.repo || t.repo || t.folder);

/** Whether two threads are one project for the orchestration tools (issue 3150). */
export const sameReach = (a: RuleThread, b: RuleThread) => sameProject(reachOf(a), reachOf(b));

/**
 * Whether `target` is work `caller` handed out itself: a thread it started is
 * reachable wherever it runs, even in another repository.
 */
export const startedBy = (caller: RuleThread, target: RuleThread) =>
  !!target.parent && target.parent === caller.id;

/**
 * Whether a caller's orchestration tools may see `target` at all: the same
 * project, or a thread the caller started wherever it runs.
 */
export const reachesThread = (caller: RuleThread, target: RuleThread) =>
  sameReach(caller, target) || startedBy(caller, target);

/** `list_sessions`: the other threads of the caller's project, newest first, metadata only. */
/** Why a thread may not manage `target`, the project rule every management tool shares. */
export function checkManage(caller: RuleThread, target: RuleThread | undefined): Failure | null {
  if (!target) return failure("not_found", "no thread has that id; use list_sessions");
  if (!reachesThread(caller, target)) {
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
    .filter((t) => allProjects || reachesThread(caller, t))
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
 * or that opens a thread to another, stays in the caller's own project — except
 * a thread on a machine the call named (`remote`), whose folders are not this
 * project's and whose card is the gate.
 */
export function checkMessage(
  caller: RuleThread,
  target: RuleThread | undefined,
  book: LinkBook,
  read = false,
  acrossProjects = false,
  limits: DelegateLimits = DEFAULT_DELEGATE_LIMITS,
  remote = false,
): Failure | null {
  if (!target) return failure("not_found", "no thread has that id; use list_sessions");
  if (target.id === caller.id)
    return failure("not_allowed", `a thread cannot ${read ? "read" : "message"} itself`);
  if (!remote && !(read && acrossProjects) && !reachesThread(caller, target)) {
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
  // limit or the machine-wide total.
  if (!hasActiveTo(book, caller.id, target.id)) {
    const slots = slotRefusal(book, caller, 1, limits);
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
      "reading usage across projects is off; turn on “Let agents read threads in other projects” in Settings › Harnesses",
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
        "SQL reads every project, so it needs “Let agents read threads in other projects” in Settings › Harnesses",
      );
}

/** Whether `caller` may start `count` threads, before the user is asked. */
export function checkStart(
  caller: RuleThread,
  book: LinkBook,
  count: number,
  limits: DelegateLimits = DEFAULT_DELEGATE_LIMITS,
): Failure | null {
  const chain = chainRefusal(book, caller);
  if (chain) return chain;
  return slotRefusal(book, caller, count, limits);
}

/** The message as the other thread receives it: who it is from, then the text. */
export const delivered = (text: string, from: string) => `From thread “${from}” (agent):\n\n${text}`;

/**
 * What a delegate is asked to end with. Its final message is what `wait_session` hands the
 * orchestrator, and that stays in the orchestrator's context for every later request, so a short
 * report is what keeps an orchestrator cheap; the full detail stays in the delegate's own thread.
 */
export const DELEGATE_REPORT =
  "When you finish, end with a short report for the orchestrator, at most about 12 lines: the outcome, the branch and pull request, the checks you ran and their result, and anything left open. No code, diffs or logs in it; it reads your thread if it needs more.";

/** A task as a new delegate receives it: attributed, with the report it ends with. */
export const delegated = (text: string, from: string) => `${delivered(text, from)}\n\n${DELEGATE_REPORT}`;

/** How full the caller's context is when it is first told, then again at each step past that. */
export const CONTEXT_NOTE_STEP = 0.25;

/**
 * The note an orchestrator's answer carries once its context passes another quarter (25%, 50%,
 * 75%), and the step it is now at; `null` when it is below the next step or the size is unknown.
 * Told once per step, so the note is not paid for on every call.
 */
export function contextNote(
  usage: { used: number; size: number } | undefined,
  toldStep: number,
): { note: string; step: number } | null {
  if (!usage || !(usage.size > 0) || !(usage.used >= 0)) return null;
  const share = usage.used / usage.size;
  const step = Math.floor(share / CONTEXT_NOTE_STEP);
  if (step < 1 || step <= toldStep) return null;
  const percent = Math.round(share * 100);
  const k = (n: number) => `${Math.round(n / 1000)}k`;
  return {
    step,
    note: `This thread's context is ${percent}% full (${k(usage.used)} of ${k(usage.size)} tokens), and every call resends it. Leave reading and checking to delegates from here. If more work is ahead, write a short handoff (open link ids, branches, pull requests, next steps) and suggest the user continue it in a new thread.`,
  };
}

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

export function updateLink(book: LinkBook, id: string, change: Partial<Link>, at = Date.now()): LinkBook {
  return { ...book, links: book.links.map((l) => (l.id === id ? advanceLink(l, change, at) : l)) };
}

/**
 * The newest link between two threads with one more event: a merge or a
 * cleanup happens to the branch, not to a turn, so it lands on the link that
 * points at that thread when the tool call itself made no new one.
 */
export function noteLinkEvent(
  book: LinkBook,
  orchestrator: string,
  target: string,
  kind: LinkEventKind,
  at = Date.now(),
): LinkBook {
  let newest: Link | undefined;
  for (const link of book.links) {
    if (link.orchestrator !== orchestrator || link.target !== target) continue;
    if (!newest || link.createdAt >= newest.createdAt) newest = link;
  }
  if (!newest) return book;
  const noted = noteEvent(newest, kind, at);
  if (noted === newest) return book;
  return { ...book, links: book.links.map((l) => (l.id === newest.id ? noted : l)) };
}

/** After a restart: work that was queued or running did not finish here, and nothing restarts by itself. */
export function interruptActive(book: LinkBook, at = Date.now()): LinkBook {
  return {
    ...book,
    links: book.links.map((l) => (isActive(l.state) ? advanceLink(l, { state: "interrupted" }, at) : l)),
  };
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
    const events = parseLinkEvents(link.events);
    links.push({
      ...(link as unknown as Link),
      createdAt: typeof link.createdAt === "number" ? link.createdAt : Date.now(),
      ...(events ? { events } : {}),
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

/** What a stored event list is read as: known kinds only, bounded, oldest first. */
function parseLinkEvents(raw: Json | undefined): LinkEvent[] | undefined {
  if (!Array.isArray(raw)) return undefined;
  const events = raw.flatMap((entry): LinkEvent[] => {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) return [];
    const event = entry as Record<string, Json | undefined>;
    const kind = event.kind;
    if (typeof kind !== "string" || !Object.hasOwn(LINK_EVENT_LABEL, kind)) return [];
    if (typeof event.at !== "number" || !Number.isFinite(event.at)) return [];
    const detail = typeof event.detail === "string" && event.detail ? { detail: event.detail } : {};
    return [{ kind: kind as LinkEventKind, at: event.at, ...detail }];
  });
  return events.slice(-MAX_LINK_EVENTS);
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
  const events = mergeEvents(base.events, other.events);
  return {
    ...other,
    ...base,
    ...(events ? { events } : {}),
    ...(base.notifiedAt === undefined && other.notifiedAt !== undefined
      ? { notifiedAt: other.notifiedAt }
      : {}),
    ...(base.askedRequest === undefined && other.askedRequest !== undefined
      ? { askedRequest: other.askedRequest }
      : {}),
    // A stall notice is a mark like the others: the later of the two copies stands.
    ...(Math.max(base.stallNotifiedFor ?? -1, other.stallNotifiedFor ?? -1) >= 0
      ? {
          stallNotifiedFor: Math.max(base.stallNotifiedFor ?? -1, other.stallNotifiedFor ?? -1),
        }
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

/**
 * The live delegates across every thread: one per target, however many links
 * point at it. This is what the machine-wide total counts.
 */
export const activeDelegates = (book: LinkBook): number =>
  new Set(book.links.filter((l) => isActive(l.state)).map((l) => l.target)).size;

/** The most live delegates one thread has right now, for Settings' usage line. */
export function busiestThreadDelegates(book: LinkBook): number {
  let most = 0;
  for (const targets of threadTargets(book).values()) most = Math.max(most, targets.size);
  return most;
}

/** The live targets of every thread, by orchestrator id. */
function threadTargets(book: LinkBook): Map<string, Set<string>> {
  const byThread = new Map<string, Set<string>>();
  for (const link of book.links) {
    if (!isActive(link.state)) continue;
    const targets = byThread.get(link.orchestrator) ?? new Set<string>();
    targets.add(link.target);
    byThread.set(link.orchestrator, targets);
  }
  return byThread;
}

/** Whether `orchestrator` already has a live link to `target`. */
const hasActiveTo = (book: LinkBook, orchestrator: string, target: string) =>
  book.links.some((l) => l.orchestrator === orchestrator && l.target === target && isActive(l.state));

/**
 * How many more delegates fit, for an answer to say: the smaller of what this
 * thread may open (its own limit) and what the machine has room for (the
 * total), so a retry with fewer never trips the other cap instead.
 */
export const linksLeft = (
  book: LinkBook,
  id: string,
  limits: DelegateLimits = DEFAULT_DELEGATE_LIMITS,
): number =>
  Math.min(
    Math.max(0, limits.perThread - activeTargets(book, id).size),
    Math.max(0, limits.total - activeDelegates(book)),
  );

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
 * The `busy` refusal when `count` more delegates would not fit: this thread's
 * own limit, or the machine-wide total. A follow-up to a thread it already
 * works with passes `count: 0`, so it never takes a slot. The reason names the
 * limit that refused it and the current count; `links_left` is the smaller of
 * the two rooms, so the agent can retry with fewer instead of guessing.
 */
function slotRefusal(
  book: LinkBook,
  caller: RuleThread,
  count: number,
  limits: DelegateLimits = DEFAULT_DELEGATE_LIMITS,
): Failure | null {
  const left = linksLeft(book, caller.id, limits);
  const active = activeTargets(book, caller.id).size;
  if (active + count > limits.perThread) {
    return failure(
      "busy",
      `at most ${limits.perThread} delegates per thread at once, and this thread has ${active}: ${count} more would not fit (${left} ${left === 1 ? "slot is" : "slots are"} left in all); wait_session on one first`,
      left,
    );
  }
  const running = activeDelegates(book);
  if (running + count > limits.total) {
    return failure(
      "busy",
      `at most ${limits.total} delegates at once across this app, and ${running} are running: ${count} more would not fit (${left} ${left === 1 ? "slot is" : "slots are"} left in all); wait_session on one first`,
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

/**
 * The clock of a delegate that now waits on the user: every live link to that
 * thread stops counting work time at `at`. A link already waiting keeps its
 * moment — a harness can ask twice in one wait — and a settled one is untouched.
 */
export function waitStarted(links: readonly Link[], thread: string, at: number): Link[] {
  let changed = false;
  const next = links.map((link) => {
    if (link.target !== thread || !isActive(link.state) || link.waitingSince !== undefined) return link;
    changed = true;
    return noteEvent({ ...link, waitingSince: at }, "asking", at);
  });
  return changed ? next : (links as Link[]);
}

/**
 * Its wait ended at `at` (the user answered, or the turn was stopped): the time
 * it took is folded into `waitedMs`, so the clock resumes from where it froze
 * instead of jumping over the wait.
 */
export function waitEnded(links: readonly Link[], thread: string, at: number): Link[] {
  let changed = false;
  const next = links.map((link) => {
    if (link.target !== thread || link.waitingSince === undefined) return link;
    changed = true;
    const { waitingSince, ...rest } = link;
    // Answered: the turn it was waited on runs again, which is the event after "Asked you".
    const resumed = noteEvent(rest, "working", at);
    return { ...resumed, waitedMs: (link.waitedMs ?? 0) + Math.max(0, at - waitingSince) };
  });
  return changed ? next : (links as Link[]);
}

export function forget(book: LinkBook, thread: string, at = Date.now()): LinkBook {
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
          ? advanceLink(l, { state: "failed", answer: "That thread was deleted." }, at)
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
    if (activity.lastCommand) entry.last_command = activity.lastCommand;
    if (activity.lastAt !== undefined && extra?.now !== undefined) {
      const idle = Math.max(0, extra.now - activity.lastAt);
      entry.idle_ms = idle;
      // The same silence the stall notice reports: a timed-out wait says so
      // without the caller having to compare idle_ms with a threshold.
      if ((state === "working" || state === "queued") && idle >= STALL_NOTICE_MS) entry.stalled = true;
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
  /** The tool call's input, for the command a stall notice names. */
  input?: Json;
}

/** What a child was last doing, and how many turns it finished (`wait_session` timeout). */
export interface LinkActivity {
  lastAt?: number;
  lastTool?: string;
  /** The last tool's command, when it reported one. */
  lastCommand?: string;
  turns: number;
}

/** A tool call's command, when its input names one. */
function commandOf(input: Json | undefined): string | undefined {
  if (!input || typeof input !== "object" || Array.isArray(input)) return undefined;
  const command = input.command ?? input.cmd;
  return typeof command === "string" && command.trim() ? command.trim() : undefined;
}

/** The last activity in a thread, from the items the app already holds: no polling. */
export function activityOf(items: readonly ActivityItem[]): LinkActivity {
  let lastAt: number | undefined;
  let lastTool: string | undefined;
  let lastCommand: string | undefined;
  let turns = 0;
  for (const item of items) {
    const at = item.endedAt ?? item.startedAt ?? item.at;
    if (at !== undefined && (lastAt === undefined || at > lastAt)) lastAt = at;
    if (item.kind === "tool") {
      lastTool = item.toolName ?? item.title ?? lastTool;
      lastCommand = commandOf(item.input) ?? lastCommand;
    }
    if (item.kind === "turnEnd") turns += 1;
  }
  return { lastAt, lastTool, ...(lastCommand ? { lastCommand } : {}), turns };
}

/** One child that has shown no progress while its turn still runs, and what it last ran. */
export interface StallNotice {
  link: Link;
  /** The activity timestamp this notice is about; the mark that stops a repeat. */
  at: number;
  idleMs: number;
  lastTool?: string;
  lastCommand?: string;
}

/** A parent that should hear about its children, and what about them. */
export interface Wake {
  parent: string;
  parentTitle: string;
  /** Children that settled and were not reported yet. */
  settled: Link[];
  /** Children waiting on the user, whose request was not reported yet. */
  asking: Link[];
  /** Children still running that have shown no progress for {@link STALL_NOTICE_MS}. */
  stalled?: StallNotice[];
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
   * The links each parent's queued wake rows already carry, by parent: they are reported when that
   * row's turn starts (`markDelivered`), so no second wake may report them first. A queue is the
   * app's memory only, so a row that is lost to a restart leaves its links unmarked and due again.
   */
  queued?: Readonly<Record<string, readonly string[]>>;
  /**
   * The threads these rules need, by id: every link's orchestrator and every
   * link's target, with the title a wake message names and whether a turn runs.
   * A thread that is missing is gone.
   */
  threads: Readonly<Record<string, { title: string; running: boolean }>>;
  /** Each link target's latest reply, for a finished link that holds no answer. */
  answers?: Readonly<Record<string, string>>;
  /** Each link target's last activity, for the stall notice and `wait_session`'s idle time. */
  activity?: Readonly<Record<string, LinkActivity>>;
  now: number;
}

const stateOfLink = (link: Link, ctx: WakeContext) => ctx.states[link.id] ?? link.state;

/**
 * A child that still runs but has shown nothing for {@link STALL_NOTICE_MS}.
 * Only a working link whose target turn runs counts: a queued link has not
 * started, a settled one has answered, and one waiting on the user is not stuck.
 */
function stallNotice(link: Link, ctx: WakeContext): StallNotice | undefined {
  if (stateOfLink(link, ctx) !== "working" || !ctx.threads[link.target]?.running) return undefined;
  const activity = ctx.activity?.[link.target];
  const at = activity?.lastAt ?? link.startedAt;
  if (at === undefined) return undefined;
  const idleMs = Math.max(0, ctx.now - at);
  if (idleMs < STALL_NOTICE_MS) return undefined;
  return {
    link,
    at,
    idleMs,
    ...(activity?.lastTool ? { lastTool: activity.lastTool } : {}),
    ...(activity?.lastCommand ? { lastCommand: activity.lastCommand } : {}),
  };
}

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
 * stopped child is not a reason to wait: nothing will ever report it. A result
 * a newer link to the same thread replaced is not owed either — the newer answer
 * is — which is what lets the tree settle without marking the old one for good:
 * should that successor be stopped or interrupted, this one is owed again.
 */
export function pendingWakeUnder(book: LinkBook, ctx: WakeContext, id: string): boolean {
  return book.links.some((link) => {
    if (link.orchestrator !== id) return false;
    const state = stateOfLink(link, ctx);
    if (state === "done" || state === "failed") {
      if (link.notifiedAt !== undefined) return false;
      return !replaced(link, book, ctx);
    }
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
 * What a parent reads about a message it queued on `title` that never started: the turn ahead of
 * it failed. The message is not sent into a thread that just failed (it would meet the same full
 * context or harness error); the parent hears why, and `message_session` reopens the thread when it
 * sends again.
 */
export function undeliveredReason(title: string, failure: string | undefined): string {
  const why = failure?.trim() ? ` (${failure.trim()})` : "";
  return `Not delivered: “${title}” failed its turn before this message started${why}. It was taken out of the queue; message the thread again to retry.`;
}

/**
 * Two copies of one link's events as one list: the same moment written in both
 * copies is one event, and a copy that knows newer ones keeps them.
 */
function mergeEvents(
  one: readonly LinkEvent[] | undefined,
  other: readonly LinkEvent[] | undefined,
): LinkEvent[] | undefined {
  const all = [...(one ?? []), ...(other ?? [])];
  if (!all.length) return undefined;
  // One row per moment across both copies, including a moment one copy repeats itself: the first
  // copy of it wins, and its duplicate does not survive the merge.
  const seen = new Set<string>();
  const merged: LinkEvent[] = [];
  for (const event of all) {
    const key = eventKey(event.kind, event.at);
    if (seen.has(key)) continue;
    seen.add(key);
    merged.push(event);
  }
  return merged.sort((a, b) => a.at - b.at).slice(-MAX_LINK_EVENTS);
}

/**
 * The book after a turn of `target` failed: every message still queued on it is failed with
 * {@link undeliveredReason}, so no row says Queued for a message that will never start on its own
 * (a failed turn holds the queue, `thread.ts` `holdsQueue`). Answers the links it failed, whose
 * messages the caller takes out of the target's queue.
 */
export function failQueuedLinks(
  book: LinkBook,
  target: string,
  failure: string | undefined,
  now: number,
): { book: LinkBook; failed: string[] } {
  const failed: string[] = [];
  const links = book.links.map((link) => {
    if (link.target !== target || link.state !== "queued") return link;
    failed.push(link.id);
    return advanceLink(
      link,
      {
        state: "failed" as const,
        answer: undeliveredReason(link.targetTitle || "the thread", failure),
        truncated: false,
        settledAt: now,
      },
      now,
    );
  });
  return failed.length ? { book: { ...book, links }, failed } : { book, failed };
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
      // The answer is a later turn's: the time of the turn already reported is not its own.
      return {
        ...replaced,
        settledAt: now,
        startedAt: undefined,
        workedMs: undefined,
        notifiedAt: undefined,
        answerChangedAt: now,
      };
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
  const replaced = replacedIds(book, ctx);
  return book.links.filter((link) => {
    const state = stateOfLink(link, ctx);
    if (state !== "done" && state !== "failed") return false;
    if (link.notifiedAt !== undefined) return false;
    return replaced.has(link.id);
  });
}

/** A link a restart or the user's Stop ended: it never answered anything. */
const stoppedLink = (link: Link, ctx: WakeContext) => {
  const state = stateOfLink(link, ctx);
  return state === "cancelled" || state === "interrupted";
};

/**
 * Whether a newer link to the same thread replaced `link`: the parent messaged that thread since, so
 * this answer is out of date and the newer turn's is the one it needs.
 *
 * A successor that was stopped never answered, so it replaces nothing: the older answer is still the
 * last real one. Nothing is written down for a replaced link either (`wakeTick` leaves it unmarked) —
 * the successor can itself be stopped or interrupted by a restart, and then the older answer is owed
 * again, which only a rule read afresh can tell.
 */
function replaced(link: Link, book: LinkBook, ctx: WakeContext): boolean {
  if (stoppedLink(link, ctx)) return false;
  return book.links.some(
    (other) =>
      other.orchestrator === link.orchestrator &&
      other.target === link.target &&
      other.createdAt > link.createdAt &&
      !stoppedLink(other, ctx),
  );
}

/** The ids of the links a newer one to the same thread replaced, heard about or not ({@link replaced}). */
function replacedIds(book: LinkBook, ctx: WakeContext): Set<string> {
  return new Set(book.links.filter((link) => replaced(link, book, ctx)).map((link) => link.id));
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
 *
 * A link a queued wake row already carries is left to that row (`ctx.queued`): it
 * is reported when the row's turn starts, so a second wake would be the same
 * result twice.
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
    const carried = new Set(ctx.queued?.[parent] ?? []);
    const active = links.some((l) => isActive(stateOfLink(l, ctx)));
    const settled = links.filter((link) => {
      const state = stateOfLink(link, ctx);
      if (state !== "done" && state !== "failed") return false;
      if (link.notifiedAt !== undefined || waiting.has(link.id)) return false;
      if (carried.has(link.id)) return false;
      if (stale.has(link.id)) return false;
      if (!quietTree(book, ctx, link.target)) return false;
      return !active || ctx.now - (link.settledAt ?? 0) >= WAKE_COALESCE_MS;
    });
    const asking = links.filter((link) => {
      if (stateOfLink(link, ctx) !== "needs_user" || waiting.has(link.id)) return false;
      if (carried.has(link.id)) return false;
      const request = ctx.pending[link.id];
      return request !== undefined && link.askedRequest !== request;
    });
    // A child that has gone quiet with its turn still running: told once per
    // activity timestamp, so a resumed child can go quiet again and be told again.
    const stalled = links.flatMap((link) => {
      const notice = stallNotice(link, ctx);
      if (!notice || link.stallNotifiedFor === notice.at) return [];
      return [notice];
    });
    if (settled.length || asking.length || stalled.length) {
      out.push({ parent, parentTitle: here.title, settled, asking, stalled });
    }
  }
  return out;
}

/**
 * What a wake row promised, by the two things a wake can say: a child finished, or a child waits on
 * the user. The row's own words, so what it may mark is what it reported — not what its links happen
 * to be when it finally runs.
 */
export interface WakeReport {
  /** Links the row named as finished (or failed). */
  settled: Iterable<string>;
  /** Links the row named as waiting on the user. */
  asking: Iterable<string>;
}

/**
 * The book after a queued wake row's turn starts: the links it reported are heard now, and nothing
 * else is. This is where a queued wake marks, not where the row is written: the queue is the app's
 * memory only, so a mark written there would outlive the row and the notice with it (`host.ts`
 * `sendWake`, `store.ts` `runPrompt`).
 *
 * Each link is marked by what the row said about it. A child the row named as *asking* that finished
 * meanwhile is left unmarked: its finish is still owed, and the ordinary wake reports it once the row
 * has run (a question is its own notice, and a finish is another). A child the row named as finished
 * that is still running, or that now asks something, is left unmarked for the same reason the other
 * way round. The call is idempotent, so a row delivered twice cannot double-mark.
 */
export function markDelivered(book: LinkBook, reported: WakeReport, ctx: WakeContext, now: number): LinkBook {
  const settled = new Set(reported.settled);
  const asking = new Set(reported.asking);
  let changed = false;
  const links = book.links.map((link) => {
    const state = stateOfLink(link, ctx);
    if (settled.has(link.id)) {
      // What it was told: this child finished. Only a finish can be marked as one.
      if (state !== "done" && state !== "failed") return link;
      if (link.notifiedAt !== undefined) return link;
      changed = true;
      return { ...link, notifiedAt: now };
    }
    if (!asking.has(link.id)) return link;
    // What it was told: this child waits on the user. A finish it reached since is a separate notice.
    if (state !== "needs_user") return link;
    const request = ctx.pending[link.id];
    if (request === undefined || link.askedRequest === request) return link;
    changed = true;
    return { ...link, askedRequest: request };
  });
  return changed ? { ...book, links } : book;
}

/**
 * Whether a queued wake row still has something to tell its parent: its turn starting now would mark
 * at least one of the links it names. A row whose children the parent already heard about — the app
 * wrote the mark and then died before the queue was written back — is nothing to run, and it is
 * dropped when the queue is read back (`state/host.ts`), so a restart cannot report the same finish
 * twice. It judges a row exactly as its turn would ({@link markDelivered}), so the two cannot
 * disagree about what the row promised.
 */
export function wakeStillOwed(
  book: LinkBook,
  row: { links: readonly string[]; asking: readonly string[] },
  ctx: WakeContext,
): boolean {
  if (!row.links.length) return false;
  const asking = new Set(row.asking);
  const reported: WakeReport = { settled: row.links.filter((id) => !asking.has(id)), asking };
  return markDelivered(book, reported, ctx, ctx.now) !== book;
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
  const carried = new Set(Object.values(ctx.queued ?? {}).flat());
  for (const link of book.links) {
    const state = stateOfLink(link, ctx);
    if (state === "working" && ctx.threads[link.target]?.running) {
      const at = ctx.activity?.[link.target]?.lastAt ?? link.startedAt;
      if (at === undefined || link.stallNotifiedFor === at) continue;
      const due = at + STALL_NOTICE_MS;
      if (due > ctx.now && (next === null || due < next)) next = due;
      continue;
    }
    if (state !== "done" && state !== "failed") continue;
    if (link.notifiedAt !== undefined || stale.has(link.id) || carried.has(link.id)) continue;
    if (!quietTree(book, ctx, link.target)) continue;
    const at = (link.settledAt ?? 0) + WAKE_COALESCE_MS;
    if (at <= ctx.now) continue;
    if (next === null || at < next) next = at;
  }
  return next;
}

/** The link ids a wake reports: what a queued wake keeps, so a newer one can be folded into it. */
export function wakeIds(wake: Wake): string[] {
  return [...wake.settled, ...wake.asking, ...(wake.stalled ?? []).map((notice) => notice.link)].map(
    (link) => link.id,
  );
}

/** The ids a wake names as waiting on the user, of {@link wakeIds}: what the row said it was. */
export function askingIds(wake: Wake): string[] {
  return wake.asking.map((link) => link.id);
}

/**
 * The wake those links make now, with each link's state as shown. It is how a wake waiting for a busy
 * parent is rebuilt around a newer one: one turn reports everything, not a queue of turns each saying
 * "Delegate finished".
 *
 * It follows `wakesDue`'s rules, so a rebuild never restates what that rule withdrew: a settled link a
 * newer one to the same thread replaced is out of date, and one the parent is blocked on in
 * `wait_session` is already in that call's answer. The two rules it leaves out are the ones about
 * timing and the mark: the coalescing window (this row already waited for it), and `notifiedAt` — a
 * queued wake's links are marked when its turn starts, not when the row is written. A link still
 * marked new is judged like `wakesDue` does, though: a settled child whose own subtree still works is
 * not finished for its parent yet. A link the row being rebuilt already carries (`promised`), or one
 * delivered before (`notifiedAt`), keeps its line whatever its thread does meanwhile, or the notice
 * the parent is waiting for would vanish with nothing else to deliver it.
 *
 * Null when nothing is left to report.
 */
export function wakeFor(
  parent: string,
  parentTitle: string,
  ids: Iterable<string>,
  book: LinkBook,
  ctx: WakeContext,
  promised?: ReadonlySet<string>,
): Wake | null {
  const want = new Set(ids);
  const replaced = replacedIds(book, ctx);
  const waiting = new Set(ctx.waiting[parent] ?? []);
  const settled: Link[] = [];
  const asking: Link[] = [];
  const stalled: StallNotice[] = [];
  for (const link of book.links) {
    if (link.orchestrator !== parent || !want.has(link.id)) continue;
    if (waiting.has(link.id)) continue;
    const state = stateOfLink(link, ctx);
    if (state === "done" || state === "failed") {
      if (replaced.has(link.id)) continue;
      // Reported already, or promised by the row being rebuilt: its line stands whatever its thread
      // does meanwhile, because nothing else would deliver that answer.
      const told = link.notifiedAt !== undefined || !!promised?.has(link.id);
      if (!told && !quietTree(book, ctx, link.target)) continue;
      settled.push(link);
    } else if (state === "needs_user") {
      asking.push(link);
    } else {
      const notice = stallNotice(link, ctx);
      if (notice) stalled.push(notice);
    }
  }
  return settled.length || asking.length || stalled.length
    ? { parent, parentTitle, settled, asking, stalled }
    : null;
}

/**
 * A queued wake with a newer one folded into it: one wake turn for everything that finished, instead
 * of a queue of turns each saying "Delegate finished". Its text is rebuilt from the links the waiting
 * row already carried plus the new wake's ({@link wakeFor}), so a child that asked and then finished
 * reads once, as finished. The row keeps its place among the user's own messages; when nothing is
 * left to report it is returned as it is.
 *
 * Null when the row cannot be rebuilt faithfully: one of the links it already reports is gone from
 * the book (pruned after many finished children), so rewriting its text would drop a report the parent
 * is still waiting for, or the row names no links at all (written by a build before the row kept them,
 * or by a preview). `queueWake` then queues the new wake beside it instead, and the old row is left as
 * it was.
 */
export function foldWake(
  waiting: QueuedPrompt,
  wake: Wake,
  book: LinkBook,
  ctx: WakeContext,
  now: number,
): QueuedPrompt | null {
  // No `wakeLinks`: what it reports cannot be read back, so its text must not be rewritten.
  if (!waiting.wakeLinks?.length) return null;
  const carried = waiting.wakeLinks;
  const known = new Set(
    book.links.filter((link) => link.orchestrator === wake.parent).map((link) => link.id),
  );
  if (carried.some((id) => !known.has(id))) return null;
  const merged = wakeFor(
    wake.parent,
    wake.parentTitle,
    [...carried, ...wakeIds(wake)],
    book,
    ctx,
    new Set(carried),
  );
  if (!merged) return waiting;
  return {
    ...waiting,
    text: wakeMessage(merged, now),
    wakeLinks: wakeIds(merged),
    wakeAsking: askingIds(merged),
  };
}

/**
 * A busy parent's queue with this wake in it: folded into the newest wake row that can take it, so one
 * turn reports everything that finished, else a row of its own. The user's own messages keep their
 * places, and the row keeps the id it was queued with, so nothing a reader is looking at moves.
 *
 * Nothing is marked here: the row carries its links and the marks are written when its turn starts
 * (`markDelivered`). The queue lives in memory, so a mark written now would survive a restart that the
 * row does not, and the wake would be lost.
 */
export function queueWake(
  queue: readonly QueuedPrompt[],
  wake: Wake,
  book: LinkBook,
  ctx: WakeContext,
  now: number,
  id: string,
): QueuedPrompt[] {
  const rows = [...queue];
  for (let at = rows.length - 1; at >= 0; at--) {
    const row = rows[at]!;
    if (!row.wake) continue;
    const folded = foldWake(row, wake, book, ctx, now);
    // A row whose own reports the book no longer holds: leave it whole and try the one before it.
    if (!folded) continue;
    rows[at] = folded;
    return rows;
  }
  return [
    ...rows,
    {
      id,
      text: wakeMessage(wake, now),
      wake: true,
      wakeLinks: wakeIds(wake),
      wakeAsking: askingIds(wake),
    },
  ];
}

/** A wake's compact first line, and the transcript's: "2 delegates finished · 1 delegate asks". */
export function wakeSummary(wake: Wake): string {
  const parts: string[] = [];
  if (wake.settled.length) {
    const failed = wake.settled.filter((link) => link.state === "failed").length;
    if (wake.settled.length === 1) {
      const link = wake.settled[0]!;
      const how = link.state === "failed" ? "failed" : "finished";
      parts.push(
        `Delegate ${how}${link.resumedAt !== undefined ? " after a restart" : ""}: ${link.targetTitle}`,
      );
    } else if (failed === wake.settled.length) {
      parts.push(`${wake.settled.length} delegates failed`);
    } else {
      parts.push(`${wake.settled.length} delegates finished`);
      if (failed) parts.push(`${failed} failed`);
    }
  }
  if (wake.asking.length === 1) {
    parts.push(`Delegate asks: ${wake.asking[0]!.targetTitle}`);
  } else if (wake.asking.length > 1) {
    parts.push(`${wake.asking.length} delegates need you`);
  }
  if (wake.stalled?.length === 1) {
    parts.push(`Delegate stalled: ${wake.stalled[0]!.link.targetTitle}`);
  } else if (wake.stalled && wake.stalled.length > 1) {
    parts.push(`${wake.stalled.length} delegates stalled`);
  }
  return parts.join(" · ");
}

/** One line of text cut at `max` characters, with an ellipsis when it was longer. */
const preview = (text: string, max: number): string =>
  text.length > max ? `${text.slice(0, max).trimEnd()}…` : text;

/** One delegate, as the wake message names it: link id, status, title, harness, worked time, thread, changes. */
const linkLine = (link: Link, state: LinkState, now: number) => {
  const bits = [`link_id ${link.id}`, state, `“${link.targetTitle}”`];
  // The turn this settled on began with the restart resume, not with the work before the quit.
  if (link.resumedAt !== undefined) bits.push("resumed after a restart");
  const harness = link.harnessName ?? link.harness;
  if (harness) bits.push(harness);
  if (link.workedMs) bits.push(`worked ${formatDuration(link.workedMs)}`);
  bits.push(`session ${link.target}`);
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
  if (wake.stalled?.length) {
    lines.push("No progress while the turn still runs:", "");
    for (const { link, idleMs, lastTool, lastCommand } of wake.stalled) {
      const tool = lastTool
        ? `${lastTool}${lastCommand ? `: ${preview(lastCommand, 120)}` : ""}`
        : "none reported";
      lines.push(
        `- Delegate “${link.targetTitle}” has made no progress for ${formatDuration(idleMs)}; last tool: ${tool}. It may be stuck on a command that does not exit; interrupt_session it or message it.`,
      );
    }
    lines.push("");
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
  kind:
    | "message"
    | "read"
    | "start"
    | "fan_out"
    | "organize"
    | "answer"
    | "interrupt"
    | "transfer"
    | "configure"
    | "merge"
    | "cleanup"
    | "handoff"
    | "project"
    | "schedule";
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
  /** The paired machine a start or a message acts on, named on the card; absent for this computer. */
  machine?: string;
}

export type GrantChoice = "always" | "once" | "deny";
