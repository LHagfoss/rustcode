// Threads that start and message other threads: the rules and records of
// orchestration M1 and v2 (docs/architecture/cross-session-orchestration.md,
// docs/orchestrator.md), after RustCodeKit's Orchestration.swift. The
// engine forwards an agent's tool call as a `_rustcode/orchestration`
// request; the app checks these rules, asks the user, and carries it out
// (state/host.ts). Pure: no store, no engine.

import { bytes, type Failure, failure } from "./orchestrationCalls";
import { type LinkActivity, mergeEvents } from "./orchestrationWake";

export {
  type ActivityItem,
  activityOf,
  askingIds,
  failQueuedLinks,
  fillAnswers,
  foldWake,
  type GrantAsk,
  type GrantChoice,
  type HeardMark,
  type LinkActivity,
  latestReply,
  MACHINE_LOST_TURN,
  markDelivered,
  nextWakeAt,
  orphanedLinks,
  pendingWakeUnder,
  queueWake,
  quietBelow,
  quietTree,
  reopenLinks,
  restateLinks,
  retainDelegateCleanup,
  type StallNotice,
  supersededLinks,
  type TurnAnswer,
  undeliveredReason,
  unheard,
  type Wake,
  type WakeContext,
  type WakeReport,
  wakeFor,
  wakeIds,
  wakeMessage,
  wakeStillOwed,
  wakeSummary,
  wakesDue,
} from "./orchestrationWake";

import type { Json, JsonObject } from "../acp/jsonrpc";
import type { DelegatePull } from "../acp/rustcode";
import { plainPath } from "../acp/types";
import type { ThreadStatus, Worktree } from "../acp/v2";
import { answerPreview } from "./answerPreview";
import { formatDuration } from "./thread";

/**
 * What the client advertises in `initialize` while the setting is on.
 *
 * Version 14 adds `steer` to `message_session`, and version 15 `standalone` to
 * `start_session` and `fan_out` (issue 3443): a thread of the user's own at the
 * top level, rather than a delegate the caller waits on. A v14 app is never
 * offered `standalone`, and the engine refuses a call that carries one.
 * Version 16 gives `machine` to the rest of the thread tools and the project
 * reads (`answer_request`, `manage_queue`, `search_sessions`, `read_activity`,
 * `configure_session`, `update_session`, `worktree_status`, `list_worktrees`,
 * `list_pull_requests`, `read_project`, `clone_project`).
 * Version 18 gives `transfer_session` an `account` and a `model`.
 * Version 19 adds `create_loop`.
 * Version 20 adds `compact_session`.
 * Version 21 adds `update_loop`.
 * Version 22 adds `run_machine_command` across all connected machines.
 */
export const CAPABILITY_VERSION = 22;
/** The most of a child's answer `wait_session` carries; beyond it, `read_session` pages the rest. */
export const MAX_BYTES = 8 * 1024;
/** One `read_session` page. */
export const READ_BYTES = 16 * 1024;
export const MAILBOX = 32;
/**
 * The default number of delegates one thread may have working for it at once
 * (Settings › Delegation). A message to a thread it already has a link to is a
 * follow-up, not another slot.
 */
export const DEFAULT_DELEGATES_PER_THREAD = 8;
export const MIN_DELEGATES_PER_THREAD = 2;
export const MAX_DELEGATES_PER_THREAD = 32;
/**
 * The default number of delegates that may run at once across this app
 * instance (Settings › Delegation), whichever thread started them. A delegate
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
 * How long a settled child waits for its siblings before the parent hears about them, when the
 * batch window is off ({@link WAKE_BATCH_DEFAULT_S} of 0).
 */
export const WAKE_COALESCE_MS = 3_000;
/**
 * The default batch window, in seconds: finishes and stalls of one parent's delegates that arrive
 * within it reach the parent in one turn instead of one turn each (issue 3649). A parent's turn
 * resends its whole context, so each wake cost about as much as a message the user typed.
 */
export const WAKE_BATCH_DEFAULT_S = 60;
export const WAKE_BATCH_MAX_S = 600;
/**
 * How long a child's running turn may show no new activity before its parent is
 * told it looks stuck. A command with no timeout (a git editor with no TTY, a
 * script whose timers never end) can hold a turn for hours, so this is the
 * point where an advisory notice is worth the interruption.
 */
export const STALL_NOTICE_MS = 10 * 60_000;
/** The most of one child's answer a wake message carries, per child. */
export const WAKE_ANSWER_CHARS = 1024;
/**
 * The most of one child's answer a `wait_session` with `all` carries, per child: a wave of full
 * 8 KiB answers outgrows what a harness shows of one tool result. The engine's no-app path uses the
 * same cut (`rustcode/engine/src/orchestration/local.rs` `ALL_ANSWER_CHARS`).
 */
export const ALL_ANSWER_CHARS = 2048;

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
export const eventKey = (kind: LinkEventKind, at: number) => `${kind}@${at}`;

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
  // A failure's own words go in the event, so the tip says why, not only that it did.
  const worked = kind === "answered" ? workedTime(next, at) : undefined;
  const detail = worked
    ? `worked ${formatDuration(worked)}`
    : kind === "failed"
      ? answerPreview(next.error)
      : undefined;
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
  /** PR outcome retained independently of the removed checkout. */
  cleanup?: { at: number; branch?: string; pull?: { [K in keyof DelegatePull]: DelegatePull[K] } };
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
  /**
   * The engine opened this link and runs its turn itself, with no app attached
   * (`rustcode/engine/src/orchestration/local_threads.rs`). It settles it in the
   * store; the app does not interrupt it while the engine still lists it live.
   */
  engine?: boolean;
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
// Windows' verbatim prefix goes too (`plainPath`): it is a Win32 detail, not part of the folder, and
// keeping it made one project two (issue 3425).
export const cleanProjectPath = (path: string) =>
  (plainPath(path).replace(/\/+$/, "") || (path.startsWith("/") ? "/" : ""))
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
 * Reading across projects needs the setting. Messaging reaches any project:
 * the existing user approval policy gates delivery in the host.
 */
export function checkMessage(
  caller: RuleThread,
  target: RuleThread | undefined,
  book: LinkBook,
  read = false,
  acrossProjects = false,
  limits: DelegateLimits = DEFAULT_DELEGATE_LIMITS,
  remote = false,
  /** A one-way message (`wait: false`) opens no link, so it takes no slot. */
  oneWay = false,
): Failure | null {
  if (!target) return failure("not_found", "no thread has that id; use list_sessions");
  if (target.id === caller.id)
    return failure("not_allowed", `a thread cannot ${read ? "read" : "message"} itself`);
  if (read && !remote && !acrossProjects && !reachesThread(caller, target)) {
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
  if (!oneWay && !hasActiveTo(book, caller.id, target.id)) {
    const slots = slotRefusal(book, caller, 1, limits);
    if (slots) return slots;
  }
  if (book.links.filter((l) => l.target === target.id && l.state === "queued").length >= MAILBOX) {
    return failure("mailbox_full", `that thread has ${MAILBOX} messages waiting`);
  }
  return null;
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

/** Who sent a delivered message, by id: the sender's session, and the link it waits on if it does. */
export interface DeliveredSender {
  session: string;
  link?: string;
}

/**
 * The message as the other thread receives it: who it is from, then the text. With `sender`, the
 * line under the heading names the sender's session (and link), so two senders with the same title
 * are told apart; the heading line itself is unchanged, so older readers still find the title
 * (`message_origin.rs` in the engine reads the same line).
 */
export const delivered = (text: string, from: string, sender?: DeliveredSender) => {
  const ids = sender ? `\nsession ${sender.session}${sender.link ? ` · link_id ${sender.link}` : ""}` : "";
  return `From thread “${from}” (agent):${ids}\n\n${text}`;
};

/** A delivered message read back: the sender's title and ids, and the text below them. */
export interface DeliveredMessage {
  title: string;
  session?: string;
  link?: string;
  body: string;
}

const DELIVERED_HEAD = /^From thread “([^\n]*)” \(agent\):(?:\nsession (\S+)(?: · link_id (\S+))?)?\n\n/;

/** `text` as `delivered` wrote it, or null for a message no other thread sent. */
export function parseDelivered(text: string): DeliveredMessage | null {
  const m = DELIVERED_HEAD.exec(text);
  if (!m) return null;
  return {
    title: m[1] ?? "",
    ...(m[2] ? { session: m[2] } : {}),
    ...(m[3] ? { link: m[3] } : {}),
    body: text.slice(m[0].length),
  };
}

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
    `Thread ID: ${answer.session_id}`,
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

/** Account authority is separate from tool access, and survives restarts in the link book.
 * IDs (not labels, models or usage readings) define exactly what the user allowed.
 */
export const delegateAccountKey = (machine: string, harness: string, account: string | null) =>
  JSON.stringify(["delegate-account", machine, harness, account]);

export const accountsNeedApproval = (book: LinkBook, caller: string, keys: string[], trustAccounts = false) =>
  keys.some((key) => {
    if (trustAccounts) {
      try {
        const [kind, machine, , account] = JSON.parse(key);
        if (
          kind === "delegate-account" &&
          typeof machine === "string" &&
          machine.length > 0 &&
          typeof account === "string" &&
          account.length > 0
        )
          return false;
      } catch {
        /* An unknown key always needs an explicit grant. */
      }
    }
    return !hasGrant(book, caller, key);
  });

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

/** Cleanup retires every outstanding result for the target, including older
 * failed turns. Cancellation and heard marks survive delayed finishes/restarts. */
export function retireCleanedLinks(book: LinkBook, target: string, at = Date.now()): LinkBook {
  return {
    ...book,
    links: book.links.map((link) =>
      link.target === target
        ? noteEvent(
            advanceLink(link, { state: "cancelled", settledAt: at, notifiedAt: at }, at),
            "cleaned",
            at,
          )
        : link,
    ),
  };
}

/** After a restart: work that was queued or running did not finish here, and nothing restarts by itself. */
export function interruptActive(
  book: LinkBook,
  at = Date.now(),
  engineLive: ReadonlySet<string> = new Set(),
): LinkBook {
  return {
    ...book,
    // A link the engine runs the turn of (no app was attached when it started)
    // is not severed: its turn goes on, and the engine settles it in the store.
    links: book.links.map((l) =>
      isActive(l.state) && !engineLive.has(l.id) ? advanceLink(l, { state: "interrupted" }, at) : l,
    ),
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

/** Why a link to a thread that no longer exists failed: the row and the tip say the same. */
export const REASON_THREAD_DELETED = "That thread was deleted.";

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
          ? advanceLink(
              l,
              { state: "failed", answer: REASON_THREAD_DELETED, error: REASON_THREAD_DELETED },
              at,
            )
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
/** One link's row in a `wait_session` answer with `all`: its answer cut to {@link ALL_ANSWER_CHARS}. */
export function cutForAll(row: JsonObject): JsonObject {
  const answer = row.answer;
  if (typeof answer !== "string") return row;
  const chars = [...answer];
  if (chars.length <= ALL_ANSWER_CHARS) return row;
  return { ...row, answer: chars.slice(0, ALL_ANSWER_CHARS).join(""), answer_truncated: true };
}

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
  // Why it failed, apart from what the reply said: without it a caller cannot say more than
  // "failed", and the answer it did give reads as the reason (issue 3523).
  if (link.error !== undefined) entry.error = link.error;
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

export {
  type ApprovalChoice,
  type Call,
  DEFAULT_ACTIVITY_ITEMS,
  DEFAULT_MERGE_MS,
  DEFAULT_WAIT_MS,
  type Failure,
  failure,
  failureJson,
  failureMessage,
  type HandoffTarget,
  type HarnessChoice,
  MAX_ACTIVITY_ITEMS,
  MAX_DELEGATES,
  MAX_LAST_TURNS,
  MAX_MERGE_MS,
  MAX_MESSAGE_BYTES,
  MAX_ORGANIZED_THREADS,
  MAX_PROJECTS,
  MAX_PULL_REQUESTS,
  MAX_QUEUE_ITEMS,
  MAX_WAIT_MS,
  MAX_WORKTREE_PULLS,
  MAX_WORKTREES,
  type PullStateChoice,
  parseCall,
  parseSchedule,
  QUEUE_PREVIEW_CHARS,
  type QueueAction,
  type SessionStatusChoice,
  type TaskSchedule,
} from "./orchestrationCalls";
