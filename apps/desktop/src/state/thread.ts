// Per-thread state and the pure reducer that folds ACP `session/update`s into
// a transcript. No React and no I/O here, so tests drive it directly.

import type { Json } from "../acp/jsonrpc";
import type {
  ConfigOption,
  FileDiff,
  HarnessDenial,
  PermissionRequest,
  PlanEntry,
  SessionHarness,
  SessionUpdate,
  SessionUsage,
  ToolStatus,
} from "../acp/types";
import type { McpApp } from "../lib/mcpApp";
import type { ComposeExtras, SentAttachment } from "./compose";
import { activeStep, advancePlanClocks, type PlanClocks, type SteerNote } from "./steer";
import { subagentOf, type ToolItem } from "./threadFormat";
import { orchestrationTool } from "./toolNames";
import { type ChildActivity, isQuiet, isRunning, type ThreadTurn, type TurnKind } from "./turn";

// What the transcript says, read off its items (state/threadFormat.ts).
export * from "./threadFormat";

// The running turn lives in `turn.ts`; callers read it from here with the rest of the thread.
export {
  type ChildActivity,
  isQuiet,
  isRunning,
  isWoken,
  queueHeld,
  type ThreadTurn,
  type TurnKind,
} from "./turn";

export type TranscriptItem =
  | {
      kind: "user";
      id: string;
      text: string;
      at?: number;
      steered?: boolean;
      attachments?: SentAttachment[];
      /** Sent by a Smart UI view's button (`rustcode.send`). */
      fromView?: boolean;
      /** The app woke the thread for its children: a divider row, not a message the user typed. */
      wake?: boolean;
      /**
       * The app carried the thread on after compacting it (`CONTINUE_MARK`): a divider row, not a
       * message the user typed, and no text of it is theirs to recall.
       */
      continued?: boolean;
      /**
       * Typed here and sent from the composer: its row flies out of the field on its first sighting
       * (`lib/sendFlight.ts`). A replayed item, a wake, a message from another thread or a view and
       * the one a thread was made for all leave it out, so they only rise in.
       */
      sent?: boolean;
    }
  | { kind: "assistant"; id: string; text: string; at?: number; turnId?: string }
  | {
      kind: "thought";
      id: string;
      text: string;
      summary: boolean;
      startedAt: number;
      durationMs?: number;
      done: boolean;
    }
  | {
      kind: "tool";
      id: string;
      title: string;
      toolKind?: string;
      status: ToolStatus;
      input?: Json;
      detail?: string;
      summary?: string;
      output?: string;
      /** The interactive view (MCP App) the result embeds (`lib/mcpApp.ts`). */
      app?: McpApp;
      diffs: FileDiff[];
      exitCode?: number;
      /** Where the harness saved the whole output when the row holds only a preview. */
      fullOutput?: string;
      /** The harness's own tool name, such as Claude Code's `Agent`. */
      toolName?: string;
      /**
       * The access mode that let this call through without asking (an
       * orchestration call on a Full-access thread): the row says so itself,
       * instead of a notice above it splitting the run of calls.
       */
      autoAllowed?: string;
      /** The harness refused this call itself; the row says who, why and where it is changed. */
      denial?: HarnessDenial;
      /** The subagent call (its tool call id) this call was made under. */
      parent?: string;
      /** When the app saw it start and end; a replayed call has both at load time. */
      startedAt?: number;
      endedAt?: number;
      /**
       * Stretches of this call spent waiting on the user's approval, in order.
       * A wait still open has no `to`; the trace draws each one apart from the
       * call's work and measures the work alone (`lib/trace.ts`).
       */
      waits?: ToolWait[];
      /**
       * Its work goes on after the call returned: `agent` for a subagent sent
       * to the background (the turn waits for it), `command` for a command the
       * harness backgrounded, which the row says and the thread's own waiting
       * line reports once it is idle.
       */
      background?: "agent" | "command";
    }
  | {
      kind: "notice";
      id: string;
      text: string;
      tone: "warning" | "error" | "info";
      resumable?: boolean;
      /** A compaction that is running: the row shows a spinner until its card replaces it. */
      progress?: boolean;
      /** The row was caused by a setting and links to it (RustCode's auto-compaction). */
      setting?: "auto-compact";
      /**
       * The same notice said this many times in a row, drawn as one row (`×6`). Set when the
       * transcript draws the row (`state/fold.ts` `collapseNotices`), not by what appended it.
       */
      count?: number;
    }
  /**
   * A question the harness asked and the answer given, kept as a row once it resolves. The
   * engine's history has no permission events, so this is the app's own record: it stays while
   * the thread is open and is not replayed when the thread is loaded again.
   */
  | {
      kind: "question";
      id: string;
      /** The question's short header, when it had one. */
      header?: string;
      text: string;
      /** What was answered: the options chosen, or the words written. */
      answer: string;
      /** The names of the files the answer carried. */
      files?: string[];
      at?: number;
    }
  /**
   * The app's own answer to a bare "status" (issue 3651): where each delegate stands. No model
   * turn ran for it, and the row says so. Not kept by the engine, so a reload drops it.
   */
  | { kind: "status"; id: string; at: number; rows: import("./delegateStatus").DelegateStatusRow[] }
  /** Where the thread moved to another harness, which got the conversation so far. */
  | { kind: "handoff"; id: string; from: string; to: string; model?: string }
  /** Where the thread's model or effort changed. */
  | { kind: "switch"; id: string; change: import("../acp/types").ChoiceChange }
  | {
      kind: "turnEnd";
      id: string;
      elapsedMs?: number;
      at?: number;
      /** When the app saw this turn begin. A replayed turn has none, and so no Trace. */
      startedAt?: number;
      /** The usage the engine reported while the turn ran, for its Trace. */
      usage?: TurnUsage;
      outcome: "completed" | "cancelled" | "failed";
    };

/**
 * What a turn's tracer reads: the session's totals when the turn began, and one
 * sample for each usage update the engine sent while it ran. Every number is
 * cumulative, so the tracer subtracts the one before to price a round.
 */
export interface TurnUsage {
  before?: SessionUsage;
  samples: { at: number; usage: SessionUsage }[];
}

/**
 * The baseline a turn's usage samples are subtracted from. A thread's first
 * turn has no earlier usage to subtract: the engine's totals are then its own,
 * so they start at zero rather than leaving the turn unpriced.
 */
function usageBaseline(state: ThreadState): TurnUsage {
  if (state.usage) return { before: state.usage, samples: [] };
  const first = !state.items.some((item) => item.kind === "turnEnd");
  return {
    before: first
      ? { used: 0, size: 0, estimated: false, inputTokens: 0, outputTokens: 0, cachedTokens: 0, cost: 0 }
      : undefined,
    samples: [],
  };
}

/** A stretch of a tool call the app spent waiting on the user's approval. */
export interface ToolWait {
  from: number;
  /** Unset while the approval still waits. */
  to?: number;
}

/**
 * The user's decision is what held this call: the approval arrived, so its
 * clock starts. A request that arrives before the call's own notification is
 * picked up when the call is added (`toolCall`), so both orders work.
 */
export function waitingOn(state: ThreadState, toolCallId: string, at: number): ThreadState {
  const index = state.toolIndex[toolCallId];
  const item = index === undefined ? undefined : state.items[index];
  if (index === undefined || item?.kind !== "tool") return state;
  // One approval per call, and a second notice does not restart a wait already running.
  if (item.waits?.some((wait) => wait.to === undefined)) return state;
  const items = replaceAt(state.items, index, { ...item, waits: [...(item.waits ?? []), { from: at }] });
  return { ...state, items };
}

/** The approval was answered (or the turn stopped): the wait closes at `at`. */
export function settledWait(state: ThreadState, toolCallId: string, at: number): ThreadState {
  const index = state.toolIndex[toolCallId];
  const item = index === undefined ? undefined : state.items[index];
  if (index === undefined || item?.kind !== "tool" || !item.waits) return state;
  const open = item.waits.findLastIndex((wait) => wait.to === undefined);
  if (open < 0) return state;
  const waits = item.waits.map((wait, i) =>
    i === open ? { from: wait.from, to: Math.max(wait.from, at) } : wait,
  );
  return { ...state, items: replaceAt(state.items, index, { ...item, waits }) };
}

/** Every wait the thread still holds closes at `at` (a stopped turn, or one that ended). */
export function settleEveryWait(state: ThreadState, at: number): ThreadState {
  let changed = false;
  const items = state.items.map((item) => {
    if (item.kind !== "tool" || !item.waits?.some((wait) => wait.to === undefined)) return item;
    changed = true;
    return {
      ...item,
      waits: item.waits.map((wait) =>
        wait.to === undefined ? { from: wait.from, to: Math.max(wait.from, at) } : wait,
      ),
    };
  });
  return changed ? { ...state, items } : state;
}

/**
 * The row a resolved question leaves behind: what was asked and what was answered. Built here,
 * not in the reducer, because a permission request never reaches the transcript otherwise.
 */
export function questionItem(
  request: PermissionRequest,
  answer: string,
  at = Date.now(),
  files: readonly string[] = [],
): TranscriptItem {
  const question = request.question;
  return {
    kind: "question",
    id: `question-${request.rpcId}`,
    header: question?.header,
    text: question?.text || request.title,
    answer,
    ...(files.length ? { files: [...files] } : {}),
    at,
  };
}

export interface QueuedPrompt {
  id: string;
  text: string;
  extras?: ComposeExtras;
  fromView?: boolean;
  /** The app woke the thread for its children: it runs as a wake turn when its turn comes. */
  wake?: boolean;
  /**
   * The links a queued wake reports, so a newer wake can be folded into it: a busy parent runs one
   * wake turn for everything that finished (state/host.ts `sendWake`).
   */
  wakeLinks?: string[];
  /**
   * The links that row named as waiting on the user, of `wakeLinks`. What a row promised decides what
   * may be marked when it runs (state/orchestration.ts `markDelivered`): a child reported as asking
   * that finished meanwhile is still owed its finish, and a question is its own notice.
   */
  wakeAsking?: string[];
  /** Delivered for another thread (orchestration link id): its answer goes back there. */
  link?: string;
  /**
   * The row is the app's own restart resume (`store.ts` `resumeInterrupted`), and these are the
   * links the quit severed: its turn starts them working again and settles them when it ends, the
   * way a link's own turn does. An empty list is still a resume, so a thread waiting on a turn
   * that outlived the window carries one.
   */
  resume?: string[];
  /**
   * The thread that queued it for this one (`message_session`), so that thread can list, edit,
   * cancel or steer it while it waits (`manage_queue`, state/host.ts). A row with no `from` is the
   * user's own, or one an older build queued: no caller may touch it.
   */
  from?: string;
  /** When it was queued, for the row that says what is still waiting. */
  queuedAt?: number;
  /**
   * It stops the turn it waits behind instead of waiting for the next one (`store.ts`
   * `restartWith`): a stopped turn leaves the queue for Resume otherwise, but this message starts
   * as soon as the stop lands.
   */
  restart?: boolean;
}

/**
 * The marker a wake message ends with. The engine keeps the text a turn was started with, not the
 * app's flag, so a wake turn read back after a reload is still a wake: a quiet line, never a message
 * that reads as the user's own.
 */
export const WAKE_MARK = "<!-- rustcode:wake -->";

/** The message an app-woken turn carries: the wake text, marked so history knows what it is. */
export const withWakeMark = (text: string) => `${text}\n\n${WAKE_MARK}`;

/** Whether a watched marker is the wake message's. */
export const isWakeMessage = (text: string) => text.includes(WAKE_MARK);

/**
 * The marker the continue-after-a-compaction prompt ends with. It is kept in the prompt's own text
 * because the engine keeps the text a turn started with, not the app's flag: a thread reloaded
 * after an automatic compaction still shows a divider, never a message that reads as the user's own.
 */
export const CONTINUE_MARK = "<!-- rustcode:continued -->";

/** The prompt that carries the thread on after the app compacted it, marked as the app's own. */
export const withContinueMark = (text: string) => `${text}\n\n${CONTINUE_MARK}`;

/**
 * The marker the prompt that continues a thread after an app restart ends with: it reads as the
 * same kind of divider ("Resumed after restart"), and survives a reload for the same reason.
 */
export const RESUME_MARK = "<!-- rustcode:resumed -->";

/**
 * The marker a linked pull request's news ends with (`manage_pull_links`): the app tells the thread,
 * so it reads as a notice row ("Pull request #12: Merged"), never as a message the user typed.
 */
export const PULL_MARK = "<!-- rustcode:pull -->";

/** A linked pull request's news for its thread, marked as the app's own. */
export const withPullMark = (text: string) => `${text}\n\n${PULL_MARK}`;

/**
 * The marker the note that a workflow run ended carries (`state/workflows.ts`): the app tells the
 * thread that started the run, so it reads as a notice row ("Workflow review: done").
 */
export const WORKFLOW_MARK = "<!-- rustcode:workflow -->";

/** A workflow run's ending for the thread that started it, marked as the app's own. */
export const withWorkflowMark = (text: string) => `${text}\n\n${WORKFLOW_MARK}`;

/**
 * Whether a message is the app's own notice: continuing the thread after a compaction or a restart,
 * a linked pull request's news, or a workflow run's end. Each shows as a notice row of its first line.
 */
export const isContinueMessage = (text: string) =>
  text.includes(CONTINUE_MARK) ||
  text.includes(RESUME_MARK) ||
  text.includes(PULL_MARK) ||
  text.includes(WORKFLOW_MARK);

/** Every hidden origin marker (`<!-- rustcode:pull -->`, wake, continued, resumed, workflow). */
const ORIGIN_MARKS = /\s*<!-- rustcode:[a-z-]+ -->/g;

/** A message's text without its hidden origin markers: they are for history, never for the eye. */
export const stripMarks = (text: string) => text.replace(ORIGIN_MARKS, "").trim();

/**
 * A queued app notice (a pull request's news, a workflow's end, a continue or resume prompt) as the
 * line its notice row shows: the first line, with no marker. A queued one reads like the delivered one.
 */
export const noticeLine = (text: string) => wakeLine(stripMarks(text));

/**
 * Whether a tool row's call is still in flight: it has not answered and its turn still runs. A turn
 * that ended (or a thread replayed from history) leaves no call in flight, so a row that never got
 * its answer settles instead of spinning for good.
 */
export const toolInFlight = (status: ToolStatus | undefined, turnRunning: boolean) =>
  turnRunning && (status === "pending" || status === "in_progress");

/** A wake message's first line: its compact summary ("2 delegates finished · 1 delegate asks"). */
export const wakeLine = (text: string) => text.split("\n")[0] ?? text;

export const TURN_PHASE_LABELS = {
  waiting: "Waiting for an update",
  writing: "Writing a reply",
  tool: "Running a tool",
  approval: "Waiting for approval",
} as const;

export interface ThreadState {
  id: string;
  cwd: string;
  title?: string;
  items: TranscriptItem[];
  /** Index by tool call id into `items`. */
  toolIndex: Record<string, number>;
  plan: PlanEntry[];
  /** When each plan step came up and finished, for the card's clocks (`state/steer.ts`). */
  planClocks: PlanClocks;
  /** The steering notes this thread's turns read, and where each took effect. */
  steers: SteerNote[];
  usage?: SessionUsage;
  configOptions: ConfigOption[];
  permissions: PermissionRequest[];
  queue: QueuedPrompt[];
  /** A stopped or failed turn left the queue waiting: it keeps its order until Resume starts it. */
  queueHeld?: boolean;
  /**
   * Rows a steer just took out of the queue: they keep the stack's place while they glide out
   * toward the transcript, then they are dropped (`ui/QueuedMessages.tsx` `LeavingQueuedRow`).
   */
  departing?: QueuedPrompt[];
  /**
   * The turn that runs, or none while the thread is idle. Everything that lives only as long as a
   * turn is in it, so ending one ({@link endTurn}) leaves nothing behind. Read it through
   * {@link isRunning}, {@link isWoken} and {@link isQuiet}.
   */
  turn?: ThreadTurn;
  /** Timing from the engine, including turns started by another window. */
  enginePhase?: { label: string; lastActivityAt: number };
  /**
   * Background commands the turn parked on; it resumes when they finish. Not part of {@link turn}:
   * the engine reports it while a turn of the app still runs and after it ended. Only a new
   * activity report clears it: starting another turn does not finish background work.
   */
  waiting?: number;
  /** Lifecycle of the thread history replay, separate from turn failures. */
  load: ThreadLoad;
  harness?: SessionHarness;
  /** The worktree the thread runs in, from `session/new` or the list. */
  worktree?: import("../acp/v2").Worktree;
  /** What went wrong, as the thread shows it: a failed turn. */
  error?: string;
  /** The failure as the harness or engine sent it, when {@link error} says it in other words. */
  errorDetail?: string;
  /** Turn timing seen on replayed replies, closed into a `turnEnd` row at the next user message. */
  pendingTurn?: { id?: string; elapsedMs?: number; at?: number; outcome?: string };
}

export type ThreadLoad =
  | { phase: "idle" }
  | { phase: "loading" }
  | { phase: "loaded" }
  | { phase: "failed"; error: string };

export const isLoaded = (
  thread: ThreadState | undefined,
): thread is ThreadState & { load: { phase: "loaded" } } => thread?.load.phase === "loaded";
export const isLoading = (
  thread: ThreadState | undefined,
): thread is ThreadState & { load: { phase: "loading" } } => thread?.load.phase === "loading";
export const openError = (thread: ThreadState | undefined): string | undefined =>
  thread?.load.phase === "failed" ? thread.load.error : undefined;

let counter = 0;
export const localId = (prefix: string) => `${prefix}-${Date.now().toString(36)}-${(++counter).toString(36)}`;

export function emptyThread(id: string, cwd: string, title?: string): ThreadState {
  return {
    id,
    cwd,
    title,
    items: [],
    toolIndex: {},
    plan: [],
    planClocks: {},
    steers: [],
    configOptions: [],
    permissions: [],
    queue: [],
    load: { phase: "idle" },
  };
}

/**
 * Closes the turn a message cut short. A harness reports the timing on its last chunk, and an
 * interrupted turn often sends none: the clock the app started the turn with is the fallback, so
 * the turn still says how long it ran.
 */
function closePendingTurn(state: ThreadState, now = Date.now()): ThreadState {
  const turn = state.pendingTurn;
  const started = state.turn?.startedAt;
  // Nothing to close: no timing came back and no turn of this app runs.
  if (!turn && started === undefined) return state;
  const outcome = turn?.outcome === "cancelled" || turn?.outcome === "failed" ? turn.outcome : "completed";
  const elapsedMs = turn?.elapsedMs ?? (started !== undefined ? Math.max(0, now - started) : undefined);
  const settled = settleEveryWait(state, now);
  return {
    ...settled,
    pendingTurn: undefined,
    // The footer below takes the samples; the turn, if it still runs, starts a new count.
    turn: settled.turn && { ...settled.turn, usage: undefined },
    items: [
      ...settled.items,
      {
        kind: "turnEnd",
        id: localId("turn"),
        elapsedMs,
        at: turn?.at ?? now,
        // The app watched this turn run only if it started it: a replay has no start.
        startedAt: turn ? undefined : started,
        usage: turn ? undefined : state.turn?.usage,
        outcome,
      },
    ],
  };
}

/**
 * Marks open thoughts as done, so their clocks stop. Called per streamed chunk,
 * so it looks back only to the last reply or tool call: adding either closed
 * every thought before it. O(tail), not O(thread).
 */
function finishThoughts(items: TranscriptItem[], now: number): TranscriptItem[] {
  let next: TranscriptItem[] | null = null;
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i]!;
    if (item.kind === "assistant" || item.kind === "tool") break;
    if (item.kind === "thought" && !item.done) {
      next ??= writable(items, copyItems);
      next[i] = { ...item, done: true, durationMs: item.durationMs ?? now - item.startedAt };
    }
  }
  return next ?? items;
}

/** Marks every open thought in the thread as done (a load or a turn ends). */
function finishAllThoughts(items: TranscriptItem[], now: number): TranscriptItem[] {
  let changed = false;
  const next = items.map((item) => {
    if (item.kind === "thought" && !item.done) {
      changed = true;
      return { ...item, done: true, durationMs: item.durationMs ?? now - item.startedAt };
    }
    return item;
  });
  return changed ? next : items;
}

// Arrays made while folding a batch of engine lines (`foldingBatch`). Nothing
// outside the batch has seen them yet, so later updates in the same batch may
// change them in place: a replay of thousands of updates then copies the
// transcript once, not once per update (that was O(updates × items)).
let owned: WeakSet<object> | null = null;

/** Run `fn` with in-place changes allowed on arrays and maps it creates; they are frozen in effect after. */
export function foldingBatch(fn: () => void): void {
  const outer = owned;
  owned = outer ?? new WeakSet();
  try {
    fn();
  } finally {
    owned = outer;
  }
}

function writable<T extends object>(value: T, copy: (v: T) => T): T {
  if (owned?.has(value)) return value;
  const next = copy(value);
  owned?.add(next);
  return next;
}

const copyItems = (items: TranscriptItem[]) => items.slice();

function append(items: TranscriptItem[], item: TranscriptItem): TranscriptItem[] {
  const next = writable(items, copyItems);
  next.push(item);
  return next;
}

function replaceAt(items: TranscriptItem[], index: number, item: TranscriptItem): TranscriptItem[] {
  const next = writable(items, copyItems);
  next[index] = item;
  return next;
}

function replaceLast(items: TranscriptItem[], item: TranscriptItem): TranscriptItem[] {
  return replaceAt(items, items.length - 1, item);
}

/**
 * The rows a quiet turn does not write. A compaction answers nothing, so its
 * thoughts, its call and the summary it streams are not the thread's record:
 * the notice saying it compacted is (`appendNotice`, or the harness's card).
 */
function quietHides(state: ThreadState, update: SessionUpdate): boolean {
  switch (update.kind) {
    case "user":
    case "agent":
    case "thought":
    case "toolCall":
      return true;
    // A call already in the transcript keeps updating (a background command of
    // an earlier turn); one this turn started never reaches a row.
    case "toolCallUpdate":
      return state.toolIndex[update.id] === undefined;
    default:
      return false;
  }
}

/** Fold one update into the thread. Pure: returns the same object when nothing changed. */
export function applyUpdate(state: ThreadState, update: SessionUpdate, now = Date.now()): ThreadState {
  // A quiet turn leaves the transcript where it was, above a notice row.
  if (isQuiet(state) && quietHides(state, update)) return state;
  const last = state.items[state.items.length - 1];
  switch (update.kind) {
    case "user": {
      const closed = closePendingTurn(state, now);
      const items = finishThoughts(closed.items, now);
      return {
        ...closed,
        items: append(items, {
          kind: "user",
          id: localId("user"),
          text: update.text,
          at: update.at,
          // A replay of history has no app flags: the message says itself whether it is a wake.
          wake: isWakeMessage(update.text) || undefined,
          continued: isContinueMessage(update.text) || undefined,
        }),
      };
    }
    case "agent": {
      if (!update.text) return state;
      const pendingTurn = update.turn
        ? {
            id: update.turn.id,
            elapsedMs: update.turn.elapsedMs,
            at: update.turn.endedAt,
            outcome: update.turn.outcome,
          }
        : state.pendingTurn;
      const items = finishThoughts(state.items, now);
      const tail = items[items.length - 1];
      if (tail?.kind === "assistant") {
        return {
          ...state,
          pendingTurn,
          items: replaceLast(items, { ...tail, text: tail.text + update.text }),
        };
      }
      return {
        ...state,
        pendingTurn,
        items: append(items, {
          kind: "assistant",
          id: localId("reply"),
          text: update.text.replace(/^\n+/, ""),
          at: update.at ?? now,
        }),
      };
    }
    case "thought": {
      if (last?.kind === "thought" && !last.done) {
        return {
          ...state,
          items: replaceLast(state.items, {
            ...last,
            text: last.text + update.text,
            durationMs: update.durationMs ?? last.durationMs,
          }),
        };
      }
      return {
        ...state,
        items: append(state.items, {
          kind: "thought",
          id: localId("thought"),
          text: update.text,
          summary: update.summary,
          startedAt: now,
          durationMs: update.durationMs,
          // A replayed thought carries its duration and is already over.
          done: update.durationMs !== undefined && !isRunning(state),
        }),
      };
    }
    case "toolCall": {
      const items = finishThoughts(state.items, now);
      const existing = state.toolIndex[update.id];
      // An approval can arrive before the call it is about: its clock starts here.
      const approval = state.permissions.find((p) => p.toolCallId === update.id && !p.question);
      const waits: ToolWait[] | undefined = approval ? [{ from: approval.at ?? now }] : undefined;
      const tool: TranscriptItem = {
        kind: "tool",
        id: update.id,
        title: update.title,
        toolKind: update.toolKind,
        status: update.status ?? "pending",
        input: update.input,
        detail: update.detail,
        summary: update.summary,
        diffs: update.diffs,
        output: update.output,
        exitCode: update.exitCode,
        fullOutput: update.fullOutput,
        app: update.app,
        toolName: update.toolName,
        parent: update.parent,
        startedAt: now,
        endedAt: update.status === "completed" || update.status === "failed" ? now : undefined,
        waits,
      };
      if (existing !== undefined && items[existing]?.kind === "tool") {
        const before = items[existing] as Extract<TranscriptItem, { kind: "tool" }>;
        const merged = {
          ...before,
          ...tool,
          toolName: tool.toolName ?? before.toolName,
          exitCode: tool.exitCode ?? before.exitCode,
          fullOutput: tool.fullOutput ?? before.fullOutput,
          app: tool.app ?? before.app,
          parent: tool.parent ?? before.parent,
          startedAt: before.startedAt,
          endedAt: tool.endedAt ?? before.endedAt,
          waits: before.waits ?? waits,
          autoAllowed: before.autoAllowed,
        };
        return { ...state, items: replaceAt(items, existing, merged) };
      }
      const toolIndex = writable(state.toolIndex, (index) => ({ ...index }));
      toolIndex[update.id] = items.length;
      return { ...state, items: append(items, tool), toolIndex };
    }
    case "toolCallUpdate": {
      const index = state.toolIndex[update.id];
      const item = index === undefined ? undefined : state.items[index];
      if (index === undefined || item?.kind !== "tool") return state;
      const next = writable(state.items, copyItems);
      const output =
        update.output ??
        (update.outputDelta !== undefined ? (item.output ?? "") + update.outputDelta : item.output);
      const background = backgroundOf(item, update, output, isRunning(state));
      const ended = !background && (update.status === "completed" || update.status === "failed");
      next[index] = {
        ...item,
        parent: item.parent ?? update.parent,
        endedAt: item.endedAt ?? (ended ? now : undefined),
        title: update.title ?? item.title,
        toolKind: update.toolKind ?? item.toolKind,
        input: update.input ?? item.input,
        status: background
          ? "in_progress"
          : (update.status ?? (item.status === "pending" ? "in_progress" : item.status)),
        // A backgrounded command is done when it returns, and its row says where its work went;
        // a background agent is the one whose call stays open.
        background: background ?? (update.backgrounded ? "command" : item.background),
        output,
        app: update.app ?? item.app,
        diffs: update.diffs.length ? update.diffs : item.diffs,
        exitCode: update.exitCode ?? item.exitCode,
        fullOutput: update.fullOutput ?? item.fullOutput,
        denial: update.denial ?? item.denial,
      };
      return { ...state, items: next };
    }
    case "plan": {
      // Each step's clock runs while the turn is on it, so the card can say how long it has taken
      // and how long it took (`state/steer.ts`).
      const planClocks = advancePlanClocks(state.planClocks, update.entries, now, isRunning(state));
      return { ...state, plan: update.entries, planClocks };
    }
    case "title":
      return { ...state, title: update.title };
    case "configOptions":
      return { ...state, configOptions: update.options };
    case "usage": {
      // A usage update while the turn runs is one round's totals: the Trace
      // chip under the turn prices its model calls from them.
      const turn = state.turn;
      if (!turn?.usage) return { ...state, usage: update.usage };
      const samples = [...turn.usage.samples, { at: now, usage: update.usage }];
      return { ...state, usage: update.usage, turn: { ...turn, usage: { ...turn.usage, samples } } };
    }
    case "compaction": {
      // One quiet row, whichever harness compacted: "Compacting context…" while it runs, which the
      // card (or the reason it failed) then takes the place of.
      const text = update.text.split("\n")[0] ?? update.text;
      // A compaction RustCode started says so, and links to the setting that started it.
      const setting = /^compact(ed|ing) (automatically|context \(auto)/i.test(text)
        ? "auto-compact"
        : undefined;
      if (update.phase === "started") return startCompaction(state, text, setting);
      return settleCompaction(state, {
        kind: "notice",
        id: localId("compaction"),
        text,
        tone: update.phase === "failed" ? "error" : "info",
        ...(setting && update.phase !== "failed" ? { setting } : {}),
      });
    }
    case "handoff":
      return {
        ...state,
        items: [
          ...finishThoughts(state.items, now),
          { kind: "handoff", id: localId("handoff"), from: update.from, to: update.to, model: update.model },
        ],
      };
    case "switch": {
      // Picks with no message between them are one change, and undoing it leaves none.
      const items = state.items.at(-1)?.kind === "switch" ? state.items.slice(0, -1) : state.items;
      const { modelFrom, modelTo, effortFrom, effortTo } = update.change;
      if (modelFrom === modelTo && effortFrom === effortTo) return { ...state, items };
      return {
        ...state,
        items: [
          ...finishThoughts(items, now),
          { kind: "switch", id: localId("switch"), change: update.change },
        ],
      };
    }
    case "notice":
      return {
        ...state,
        items: [
          ...state.items,
          {
            kind: "notice",
            id: localId("notice"),
            text: update.text,
            tone: "warning",
            resumable: update.resumable,
          },
        ],
      };
    case "commands":
    case "unknown":
      return state;
  }
}

/**
 * The one way into a running turn: a fresh {@link ThreadTurn} replaces whatever an earlier one
 * left, and what only an idle thread has (a held queue and the last turn's error) goes with it.
 * Background commands belong to the thread and can outlive several turns.
 */
function startTurn(state: ThreadState, kind: TurnKind, now: number, usage: TurnUsage): ThreadState {
  return {
    ...state,
    turn: { kind, startedAt: now, usage },
    // A turn starts the queue: what a stopped turn held is running again.
    queueHeld: undefined,
    error: undefined,
    errorDetail: undefined,
  };
}

/**
 * The one way out of a turn without its footer (the connection that ran it went away): the thread
 * is idle, with nothing of the turn left. {@link finishTurn} is the way out that writes one.
 */
export function endTurn(state: ThreadState): ThreadState {
  return state.turn === undefined ? state : { ...state, turn: undefined };
}

/**
 * What the engine last sampled of the processes the running turn's harness started, or none when
 * they all exited. A sample that arrives after the turn ended belongs to no turn and is dropped.
 */
export function withChildActivity(state: ThreadState, children: ChildActivity | undefined): ThreadState {
  if (!state.turn) return state;
  return { ...state, turn: { ...state.turn, children } };
}

export function beginTurn(
  state: ThreadState,
  text: string,
  now = Date.now(),
  attachments?: SentAttachment[],
  fromView?: boolean,
  wake?: boolean,
  sent?: boolean,
  continued?: boolean,
): ThreadState {
  const closed = closePendingTurn(state);
  return {
    ...startTurn(closed, "user", now, usageBaseline(state)),
    items: [
      ...closed.items,
      {
        kind: "user",
        id: localId("user"),
        text,
        at: now,
        attachments: attachments?.length ? attachments : undefined,
        fromView: fromView || undefined,
        wake: wake || isWakeMessage(text) || undefined,
        continued: continued || isContinueMessage(text) || undefined,
        sent: sent || undefined,
      },
    ],
  };
}

/**
 * The engine resumed a turn on its own (the background commands it parked on
 * finished): it runs again, with a quiet line saying why. Whether it carries on
 * is the turn's own business, so the line does not claim it does.
 */
/**
 * A turn the app starts for the thread's own good (an automatic compaction):
 * it runs like any other, but adds no message the user did not write, and none
 * of the work it does. Without that, its thoughts and its compaction call land
 * in the turn above it, whose final answer then counts as folded work and
 * disappears under a `Worked for …` divider the reader never asked for.
 */
export function beginQuietTurn(state: ThreadState, now = Date.now()): ThreadState {
  // The answer above it ends here, whatever the turn's own timing left open.
  const closed = closePendingTurn(state);
  return startTurn(closed, "quiet", now, usageBaseline(state));
}

export function wakeTurn(state: ThreadState, now = Date.now()): ThreadState {
  if (isRunning(state)) return state;
  // The engine's own turn leaves a hold the user's stop put on the queue as it was.
  const { queueHeld } = state;
  return {
    ...startTurn(state, "woken", now, usageBaseline(state)),
    queueHeld,
    items: [
      ...state.items,
      { kind: "notice", id: localId("wake"), text: "Background task finished", tone: "info" },
    ],
  };
}

/**
 * Whether this update's call goes on after it returned, so its row keeps
 * running: a subagent launched in the background (Claude Code answers
 * `Async agent launched` at once, then holds the turn until the agent ends).
 * A command the harness backgrounds has finished, whatever its task does next:
 * the call answered with the task's id, the harness reports the task
 * separately, and the app's waiting line says the thread has work in flight. A
 * row that spun until a task update came back kept spinning when none did.
 */
function backgroundOf(
  item: ToolItem,
  update: Extract<SessionUpdate, { kind: "toolCallUpdate" }>,
  output: string | undefined,
  running: boolean,
): "agent" | undefined {
  if (update.status !== "completed" || !running) return undefined;
  if (subagentOf(item) && output?.trimStart().startsWith("Async agent launched")) return "agent";
  return undefined;
}

/**
 * Whether a turn that ended this way leaves the queue for the user to resume. Only a turn that
 * completed runs the queue on: after a stop or a failure the messages keep their order and wait, so
 * nothing goes out that the user did not ask for.
 */
export function holdsQueue(outcome: "completed" | "cancelled" | "failed"): boolean {
  return outcome !== "completed";
}

/**
 * A turn ended: thoughts stop, running tools are marked, and a footer row is
 * added.
 */
export function finishTurn(
  state: ThreadState,
  outcome: "completed" | "cancelled" | "failed",
  now = Date.now(),
  error?: string,
  quiet = false,
): ThreadState {
  const items = finishAllThoughts(settleEveryWait(state, now).items, now).map((item) =>
    item.kind === "tool" && (item.status === "pending" || item.status === "in_progress")
      ? { ...item, status: (outcome === "completed" ? "completed" : "failed") as ToolStatus, endedAt: now }
      : item,
  );
  const startedAt = state.turn?.startedAt;
  const elapsedMs = startedAt !== undefined ? now - startedAt : undefined;
  // A compaction still "running" when its turn ends did not finish. A failed turn says why; a
  // stopped one leaves nothing; the app's own (quiet) turn leaves it to the store, which waits for
  // the usage that proves the context moved before it says "Compacted automatically".
  const compactionOpen = items.some(isCompacting);
  const compacting = !compactionOpen
    ? items
    : outcome === "failed"
      ? settleCompaction(
          { ...state, items },
          {
            kind: "notice",
            id: localId("compaction"),
            text: `Context compaction failed${error ? `: ${error}` : "."}`,
            tone: "error",
          },
        ).items
      : outcome === "cancelled" || !quiet
        ? settleCompaction({ ...state, items }, null).items
        : items;
  return {
    ...endTurn(state),
    // A turn that completed runs the queue on (store.ts `runPrompt`); one the user stopped, or that
    // failed, leaves its messages in order for Resume. A message that stopped the turn (`restart`)
    // starts as soon as the stop lands.
    queueHeld: holdsQueue(outcome) && state.queue.length && !state.queue[0]?.restart ? true : undefined,
    pendingTurn: undefined,
    permissions: [],
    error,
    errorDetail: undefined,
    items: quiet
      ? compacting
      : [
          ...compacting,
          {
            kind: "turnEnd",
            id: localId("turn"),
            elapsedMs,
            at: now,
            startedAt,
            usage: state.turn?.usage,
            outcome,
          },
        ],
  };
}

/** The replay of `session/load` finished. */
export function finishLoad(state: ThreadState): ThreadState {
  const closed = closePendingTurn(state);
  return { ...closed, load: { phase: "loaded" }, items: finishAllThoughts(closed.items, Date.now()) };
}

/** The id of the one row a running compaction owns, so its card can take its place. */
const COMPACTING_ID = "compaction-running";

const isCompacting = (item: TranscriptItem) => item.kind === "notice" && item.id === COMPACTING_ID;

/** "Compacting context…" as a row, once: the engine and the app may both announce the same one. */
export function startCompaction(
  state: ThreadState,
  text = "Compacting context…",
  setting?: "auto-compact",
): ThreadState {
  if (state.items.some(isCompacting)) return state;
  return {
    ...state,
    items: [
      ...state.items,
      {
        kind: "notice",
        id: COMPACTING_ID,
        text,
        tone: "info",
        progress: true,
        ...(setting ? { setting } : {}),
      },
    ],
  };
}

/**
 * The compaction ended: its row (or `row`, when none was running) takes the place of the running
 * one, wherever the thread has got to since. `null` drops it without a trace.
 */
export function settleCompaction(
  state: ThreadState,
  row: Extract<TranscriptItem, { kind: "notice" }> | null,
): ThreadState {
  const at = state.items.findIndex(isCompacting);
  if (at < 0) return row ? { ...state, items: [...state.items, row] } : state;
  const items = row ? replaceAt(state.items, at, row) : state.items.filter((_, i) => i !== at);
  return { ...state, items };
}

/** One quiet row in the transcript, for something the app did on the thread's behalf. */
export function appendNotice(
  state: ThreadState,
  text: string,
  tone: "info" | "warning" = "info",
): ThreadState {
  return {
    ...state,
    items: [...state.items, { kind: "notice", id: localId("notice"), text, tone }],
  };
}

/**
 * A call the thread's access level let through without asking: the newest call
 * still running for that orchestration tool says so on its own row. `attached`
 * is false when no such row is in flight (the caller then says it another way).
 */
export function attachAutoAllowed(
  state: ThreadState,
  tool: string,
  mode: string,
): { state: ThreadState; attached: boolean } {
  const want = orchestrationTool(tool);
  if (!want) return { state, attached: false };
  for (let i = state.items.length - 1; i >= 0; i--) {
    const item = state.items[i]!;
    if (item.kind !== "tool") continue;
    if (item.status !== "pending" && item.status !== "in_progress") continue;
    // A row that already says why keeps its first answer: two calls in flight
    // mark two rows, not the same one twice.
    if (item.autoAllowed) continue;
    // Only RustCode's own server, or a bare name: another server's tool is not ours.
    if (orchestrationTool(item.title) !== want) continue;
    return {
      state: { ...state, items: replaceAt(state.items, i, { ...item, autoAllowed: mode }) },
      attached: true,
    };
  }
  return { state, attached: false };
}

/**
 * Whether an auto-allowed call still needs a notice of its own. A `start_session` call keeps a row
 * in the transcript once it ends: it becomes the card of the thread it started
 * (`startedLinks` → `StartedThreadsRow`), and with its thread working, that thread's row in the strip
 * above the composer. Either way the reader is looking at what the call did, so a notice beside it
 * only says it twice. Every other orchestration call's row goes quiet when it ends (it keeps no
 * `autoAllowed` mark once the call is done), so the notice is the only record of what the app did.
 */
export function autoAllowedNoticeNeeded(state: ThreadState, tool: string): boolean {
  const want = orchestrationTool(tool);
  if (want !== "start_session") return true;
  return !state.items.some((item) => item.kind === "tool" && orchestrationTool(item.title) === want);
}

/**
 * Add a steered message where the running turn will read it, and the note saying where that was: the
 * plan step the turn was on when it took the note, so the plan card can show it there, or none, and
 * the transcript draws a line under the message instead (`state/steer.ts`).
 */
export function appendSteer(
  state: ThreadState,
  text: string,
  now = Date.now(),
  attachments?: SentAttachment[],
): ThreadState {
  const id = localId("user");
  return {
    ...state,
    items: [
      ...state.items,
      {
        kind: "user",
        id,
        text,
        at: now,
        steered: true,
        attachments: attachments?.length ? attachments : undefined,
      },
    ],
    steers: [...state.steers, { id, text, at: now, step: activeStep(state.plan) }],
  };
}

/** A queued row a steer took: it glides out of the stack, then `dropDeparted` removes it. */
export function departQueued(state: ThreadState, item: QueuedPrompt): ThreadState {
  if (state.departing?.some((d) => d.id === item.id)) return state;
  return { ...state, departing: [item, ...(state.departing ?? [])] };
}

/** The row has finished gliding: the thread lets it go. */
export function dropDeparted(state: ThreadState, id: string): ThreadState {
  if (!state.departing?.some((d) => d.id === id)) return state;
  return { ...state, departing: state.departing.filter((d) => d.id !== id) };
}

/** Steers the turn never read go back to the front of the queue, and leave the transcript. */
export function requeueSteers(state: ThreadState, unused: string[]): ThreadState {
  if (!unused.length) return state;
  const wanted = unused.map((t) => t.trim());
  const dropped: string[] = [];
  const items = state.items.filter((item) => {
    if (item.kind !== "user" || !item.steered) return true;
    const at = wanted.indexOf(item.text.trim());
    if (at < 0) return true;
    wanted.splice(at, 1);
    dropped.push(item.id);
    return false;
  });
  return {
    ...state,
    items,
    // A note whose message went back to the queue took effect nowhere: it goes with it.
    steers: state.steers.filter((note) => !dropped.includes(note.id)),
    queue: [...unused.map((text) => ({ id: localId("queued"), text })), ...state.queue],
  };
}

/** The thread as it was just before item `itemId` (a rewind): later items go, tool indexes are rebuilt. */
export function truncateBefore(state: ThreadState, itemId: string): ThreadState {
  const at = state.items.findIndex((i) => i.id === itemId);
  if (at < 0) return state;
  const items = state.items.slice(0, at);
  const toolIndex: Record<string, number> = {};
  items.forEach((item, i) => {
    if (item.kind === "tool") toolIndex[item.id] = i;
  });
  const kept = new Set(items.map((item) => item.id));
  return {
    ...state,
    items,
    toolIndex,
    pendingTurn: undefined,
    plan: [],
    planClocks: {},
    steers: state.steers.filter((note) => kept.has(note.id)),
  };
}

/** Which of the thread's messages `itemId` is, counting from 0 as a replay shows them; -1 if none. */
export function userIndexOf(state: ThreadState, itemId: string): number {
  let n = 0;
  for (const item of state.items) {
    if (item.kind !== "user") continue;
    if (item.id === itemId) return n;
    n++;
  }
  return -1;
}

/** User messages after `itemId`. */
export function messagesAfter(state: ThreadState, itemId: string): number {
  const at = state.items.findIndex((i) => i.id === itemId);
  return at < 0 ? 0 : state.items.slice(at + 1).filter((i) => i.kind === "user").length;
}
