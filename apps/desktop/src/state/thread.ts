// Per-thread state and the pure reducer that folds ACP `session/update`s into
// a transcript. No React and no I/O here, so tests drive it directly.

import type { Json } from "../acp/jsonrpc";
import type {
  ConfigOption,
  FileDiff,
  HarnessCapabilities,
  PermissionRequest,
  PlanEntry,
  SessionUpdate,
  SessionUsage,
  ToolStatus,
} from "../acp/types";
import { unwrapShell } from "../lib/command";
import type { ComposeExtras, SentAttachment } from "./compose";
import { displayTool, orchestrationTool, usefulSummary } from "./toolNames";

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
      diffs: FileDiff[];
      exitCode?: number;
      /** The harness's own tool name, such as Claude Code's `Agent`. */
      toolName?: string;
      /**
       * The access mode that let this call through without asking (an
       * orchestration call on a Full-access thread): the row says so itself,
       * instead of a notice above it splitting the run of calls.
       */
      autoAllowed?: string;
      /** The subagent call (its tool call id) this call was made under. */
      parent?: string;
      /** When the app saw it start and end; a replayed call has both at load time. */
      startedAt?: number;
      endedAt?: number;
      /**
       * Its work goes on after the call returned: `agent` for a subagent sent
       * to the background (the turn waits for it), `task` for a background
       * command (it outlives the turn; the engine completes the call when it ends).
       */
      background?: "agent" | "task";
    }
  | { kind: "notice"; id: string; text: string; tone: "warning" | "error" | "info"; resumable?: boolean }
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
      at?: number;
    }
  /** Where the thread moved to another harness, which got the conversation so far. */
  | { kind: "handoff"; id: string; from: string; to: string; model?: string }
  /** Where the thread's model or effort changed. */
  | { kind: "switch"; id: string; change: import("../acp/types").ChoiceChange }
  | {
      kind: "turnEnd";
      id: string;
      elapsedMs?: number;
      at?: number;
      outcome: "completed" | "cancelled" | "failed";
    };

/**
 * The row a resolved question leaves behind: what was asked and what was answered. Built here,
 * not in the reducer, because a permission request never reaches the transcript otherwise.
 */
export function questionItem(request: PermissionRequest, answer: string, at = Date.now()): TranscriptItem {
  const question = request.question;
  return {
    kind: "question",
    id: `question-${request.rpcId}`,
    header: question?.header,
    text: question?.text || request.title,
    answer,
    at,
  };
}

export interface QueuedPrompt {
  id: string;
  text: string;
  extras?: ComposeExtras;
  fromView?: boolean;
  /** Delivered for another thread (orchestration link id): its answer goes back there. */
  link?: string;
}

export interface ThreadState {
  id: string;
  cwd: string;
  title?: string;
  items: TranscriptItem[];
  /** Index by tool call id into `items`. */
  toolIndex: Record<string, number>;
  plan: PlanEntry[];
  usage?: SessionUsage;
  configOptions: ConfigOption[];
  permissions: PermissionRequest[];
  queue: QueuedPrompt[];
  running: boolean;
  turnStartedAt?: number;
  /** Background commands the last turn parked on; it resumes when they finish. */
  waiting?: number;
  /** The running turn is one the engine resumed itself, not a prompt the app sent. */
  woken?: boolean;
  /** The replay of `session/load` is still arriving. */
  loading: boolean;
  loaded: boolean;
  harness?: { id: string; capabilities: HarnessCapabilities };
  /** The worktree the thread runs in, from `session/new` or the list. */
  worktree?: import("../acp/v2").Worktree;
  error?: string;
  /** Turn timing seen on replayed replies, closed into a `turnEnd` row at the next user message. */
  pendingTurn?: { id?: string; elapsedMs?: number; at?: number; outcome?: string };
}

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
    configOptions: [],
    permissions: [],
    queue: [],
    running: false,
    loading: false,
    loaded: false,
  };
}

function closePendingTurn(state: ThreadState): ThreadState {
  const turn = state.pendingTurn;
  if (!turn) return state;
  const outcome = turn.outcome === "cancelled" || turn.outcome === "failed" ? turn.outcome : "completed";
  return {
    ...state,
    pendingTurn: undefined,
    items: [
      ...state.items,
      { kind: "turnEnd", id: localId("turn"), elapsedMs: turn.elapsedMs, at: turn.at, outcome },
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

/** Fold one update into the thread. Pure: returns the same object when nothing changed. */
export function applyUpdate(state: ThreadState, update: SessionUpdate, now = Date.now()): ThreadState {
  const last = state.items[state.items.length - 1];
  switch (update.kind) {
    case "user": {
      const closed = closePendingTurn(state);
      const items = finishThoughts(closed.items, now);
      return {
        ...closed,
        items: append(items, { kind: "user", id: localId("user"), text: update.text, at: update.at }),
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
          done: update.durationMs !== undefined && !state.running,
        }),
      };
    }
    case "toolCall": {
      const items = finishThoughts(state.items, now);
      const existing = state.toolIndex[update.id];
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
        toolName: update.toolName,
        parent: update.parent,
        startedAt: now,
        endedAt: update.status === "completed" || update.status === "failed" ? now : undefined,
      };
      if (existing !== undefined && items[existing]?.kind === "tool") {
        const before = items[existing] as Extract<TranscriptItem, { kind: "tool" }>;
        const merged = {
          ...before,
          ...tool,
          toolName: tool.toolName ?? before.toolName,
          parent: tool.parent ?? before.parent,
          startedAt: before.startedAt,
          endedAt: tool.endedAt ?? before.endedAt,
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
      const background = backgroundOf(item, update, output, state.running);
      const ended = !background && (update.status === "completed" || update.status === "failed");
      next[index] = {
        ...item,
        parent: item.parent ?? update.parent,
        endedAt: item.endedAt ?? (ended ? now : undefined),
        title: update.title ?? item.title,
        status: background
          ? "in_progress"
          : (update.status ?? (item.status === "pending" ? "in_progress" : item.status)),
        background: background ?? item.background,
        output,
        diffs: update.diffs.length ? update.diffs : item.diffs,
        exitCode: update.exitCode ?? item.exitCode,
      };
      return { ...state, items: next };
    }
    case "plan":
      return { ...state, plan: update.entries };
    case "title":
      return { ...state, title: update.title };
    case "configOptions":
      return { ...state, configOptions: update.options };
    case "usage":
      return { ...state, usage: update.usage };
    case "compaction":
      // One quiet row, whichever harness compacted.
      return {
        ...state,
        items: [
          ...state.items,
          {
            kind: "notice",
            id: localId("compaction"),
            text: update.text.split("\n")[0] ?? update.text,
            tone: "info",
          },
        ],
      };
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

export function beginTurn(
  state: ThreadState,
  text: string,
  now = Date.now(),
  attachments?: SentAttachment[],
  fromView?: boolean,
): ThreadState {
  const closed = closePendingTurn(state);
  return {
    ...closed,
    running: true,
    turnStartedAt: now,
    waiting: undefined,
    woken: undefined,
    error: undefined,
    items: [
      ...closed.items,
      {
        kind: "user",
        id: localId("user"),
        text,
        at: now,
        attachments: attachments?.length ? attachments : undefined,
        fromView: fromView || undefined,
      },
    ],
  };
}

/**
 * The engine resumed a turn on its own (the background commands it parked on
 * finished): it runs again, with a quiet line saying why.
 */
export function wakeTurn(state: ThreadState, now = Date.now()): ThreadState {
  if (state.running) return { ...state, waiting: undefined };
  return {
    ...state,
    running: true,
    woken: true,
    waiting: undefined,
    turnStartedAt: now,
    error: undefined,
    items: [
      ...state.items,
      { kind: "notice", id: localId("wake"), text: "Background work finished — continuing", tone: "info" },
    ],
  };
}

/**
 * Whether this update's call goes on in the background, so its row keeps
 * running: a call the harness marks backgrounded, or a subagent launched in the
 * background (Claude Code answers `Async agent launched` at once, then holds the
 * turn until the agent ends). Only while the turn runs: a replayed one is over.
 */
function backgroundOf(
  item: ToolItem,
  update: Extract<SessionUpdate, { kind: "toolCallUpdate" }>,
  output: string | undefined,
  running: boolean,
): "agent" | "task" | undefined {
  if (update.status !== "completed" || !running) return undefined;
  if (update.backgrounded) return "task";
  if (subagentOf(item) && output?.trimStart().startsWith("Async agent launched")) return "agent";
  return undefined;
}

/**
 * A turn ended: thoughts stop, running tools are marked, and a footer row is
 * added. A background command keeps running; the engine ends its call.
 */
export function finishTurn(
  state: ThreadState,
  outcome: "completed" | "cancelled" | "failed",
  now = Date.now(),
  error?: string,
): ThreadState {
  const items = finishAllThoughts(state.items, now).map((item) =>
    item.kind === "tool" &&
    (item.status === "pending" || item.status === "in_progress") &&
    !(item.background === "task" && outcome === "completed")
      ? { ...item, status: (outcome === "completed" ? "completed" : "failed") as ToolStatus, endedAt: now }
      : item,
  );
  const elapsedMs = state.turnStartedAt !== undefined ? now - state.turnStartedAt : undefined;
  return {
    ...state,
    running: false,
    turnStartedAt: undefined,
    woken: undefined,
    pendingTurn: undefined,
    permissions: [],
    error,
    items: [...items, { kind: "turnEnd", id: localId("turn"), elapsedMs, at: now, outcome }],
  };
}

/** The replay of `session/load` finished. */
export function finishLoad(state: ThreadState): ThreadState {
  const closed = closePendingTurn(state);
  return { ...closed, loading: false, loaded: true, items: finishAllThoughts(closed.items, Date.now()) };
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

/** Add a steered message where the running turn will read it. */
export function appendSteer(
  state: ThreadState,
  text: string,
  now = Date.now(),
  attachments?: SentAttachment[],
): ThreadState {
  return {
    ...state,
    items: [
      ...state.items,
      {
        kind: "user",
        id: localId("user"),
        text,
        at: now,
        steered: true,
        attachments: attachments?.length ? attachments : undefined,
      },
    ],
  };
}

/** Steers the turn never read go back to the front of the queue, and leave the transcript. */
export function requeueSteers(state: ThreadState, unused: string[]): ThreadState {
  if (!unused.length) return state;
  const wanted = unused.map((t) => t.trim());
  const items = state.items.filter((item) => {
    if (item.kind !== "user" || !item.steered) return true;
    const at = wanted.indexOf(item.text.trim());
    if (at < 0) return true;
    wanted.splice(at, 1);
    return false;
  });
  return {
    ...state,
    items,
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
  return { ...state, items, toolIndex, pendingTurn: undefined, plan: [] };
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

export interface TurnCompletion {
  files: { path: string; added: number; removed: number }[];
  added: number;
  removed: number;
  checks: { command: string; outcome: "passed" | "failed" | "unknown" }[];
}

/** A run of tests, a linter, a type check or a build: the program, or a runner and its verb. */
const CHECK_PROGRAM = /^(pytest|jest|vitest|tsc|eslint|ruff|mypy|clippy|swift-test|playwright)$/;
const CHECK_RUNNER =
  /^(cargo|bun|bunx|npm|pnpm|yarn|npx|deno|go|swift|make|just|uv|python3?|mix|dotnet|gradle|\.\/gradlew|mvn|xcodebuild|biome)$/;
const CHECK_VERB = /^(test|tests|check|lint|clippy|typecheck|tsc|pytest|build|ci|fmt|vitest|jest)(:\S+)?$/;

/**
 * The checks a command line runs, each as typed: `bun test && bun run lint` is two.
 * A word in a path (`nl src/queue.test.ts`) or an option (`git diff --check`) is not one.
 */
export function checkCommands(command: string): string[] {
  return unwrapShell(command.trim())
    .split(/\s*(?:&&|\|\||;|\n)\s*/)
    .flatMap((part) => {
      const words = part
        .trim()
        .split(/\s+/)
        .filter((w) => !/^\w+=/.test(w));
      const program = words[0]?.split("/").at(-1) ?? "";
      if (CHECK_PROGRAM.test(program)) return [words.join(" ")];
      if (!CHECK_RUNNER.test(program)) return [];
      // The verb comes first, after `run`/`-m`, or after a few options (`cargo +nightly clippy`).
      const verbs = words
        .slice(1, 5)
        .filter((w) => !w.startsWith("-") && !w.startsWith("+") && w !== "run" && w !== "x");
      return verbs[0] && CHECK_VERB.test(verbs[0]) ? [words.join(" ")] : [];
    });
}

/** What the turn ending at `endIndex` changed and which checks it ran, from its tool calls only. */
export function turnCompletion(items: TranscriptItem[], endIndex: number): TurnCompletion | null {
  let start = endIndex - 1;
  while (start >= 0 && items[start]?.kind !== "user") start--;
  const files = new Map<string, { path: string; added: number; removed: number }>();
  const checks = new Map<string, { command: string; outcome: "passed" | "failed" | "unknown" }>();
  for (const item of items.slice(start + 1, endIndex)) {
    if (item.kind !== "tool") continue;
    if (item.status !== "failed") {
      for (const diff of item.diffs) {
        const counts = diffCounts([diff]);
        const f = files.get(diff.path) ?? { path: diff.path, added: 0, removed: 0 };
        f.added += counts.added;
        f.removed += counts.removed;
        files.set(diff.path, f);
      }
    }
    const input =
      item.input && typeof item.input === "object" && !Array.isArray(item.input) ? item.input : {};
    const command =
      typeof input.command === "string"
        ? input.command
        : typeof input.cmd === "string"
          ? input.cmd
          : undefined;
    const failed = item.status === "failed" || (item.exitCode !== undefined && item.exitCode !== 0);
    for (const key of command ? checkCommands(command) : []) {
      // A check run again counts as its last run.
      checks.delete(key);
      checks.set(key, {
        command: key,
        outcome: failed ? "failed" : item.status === "completed" ? "passed" : "unknown",
      });
    }
  }
  if (!files.size && !checks.size) return null;
  const list = [...files.values()];
  return {
    files: list,
    added: list.reduce((n, f) => n + f.added, 0),
    removed: list.reduce((n, f) => n + f.removed, 0),
    checks: [...checks.values()],
  };
}

// ---------------------------------------------------------------- derived values

export function formatDuration(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

/** How long a thread rests untouched before the sidebar settles it on its own. */
export const SETTLE_AFTER_MS = 3 * 86_400_000;

/** Whether a thread last active at `at` has been idle long enough to settle. */
export function idleSettled(at: number | undefined, now = Date.now()): boolean {
  return at !== undefined && now - at >= SETTLE_AFTER_MS;
}

/** A running turn's clock: `0:42`, `12:03`, `1:02:03`. */
export function formatClock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

/** When a message was sent: the time today, then `yesterday at 12:33`, then the date with the time. */
export function messageTime(at: number | undefined, now = Date.now()): string {
  if (!at) return "";
  const date = new Date(at);
  const time = date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  const midnight = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((midnight(new Date(now)) - midnight(date)) / 86_400_000);
  if (days <= 0) return time;
  if (days === 1) return `yesterday at ${time}`;
  const sameYear = date.getFullYear() === new Date(now).getFullYear();
  const day = date.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    ...(sameYear ? {} : { year: "numeric" }),
  });
  return `${day}, ${time}`;
}

/** The sidebar's compact age: `now`, `5m`, `3h`, `2d`, then the date. */
export function shortAge(at: number | undefined, now = Date.now()): string {
  if (!at) return "";
  const minutes = Math.floor((now - at) / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d`;
  return new Date(at).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

export function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`;
  if (n >= 1000) return `${Math.round(n / 1000)}k`;
  return String(n);
}

export function relativeAge(at: number | undefined, now = Date.now()): string {
  if (!at) return "";
  const minutes = Math.floor((now - at) / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes} min. ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days} d ago`;
  return new Date(at).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

/** The `**headings**` of a thought, as the closed row's one-line gist. */
/**
 * The live line under a running turn: what the agent does right now. The
 * newest step of the turn decides: a running tool (as "Running <command>" for
 * a shell), a thought still streaming (its latest bold heading), a reply being written, else "Working".
 */
export function workingLabel(items: TranscriptItem[]): string {
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i]!;
    if (item.kind === "user") break;
    if (item.kind === "tool") {
      // A subagent's own steps show on its row, not here.
      if (item.parent) continue;
      if (item.status !== "in_progress" && item.status !== "pending") return "Working";
      const agent = subagentOf(item);
      if (agent) {
        const working = items.filter(
          (i) => subagentOf(i) && i.kind === "tool" && (i.status === "in_progress" || i.status === "pending"),
        ).length;
        return working > 1 ? `${working} agents working` : `${agent.name} · ${agent.task}`;
      }
      return stepLabel(item);
    }
    if (item.kind === "thought") {
      if (item.done) return "Working";
      const heading = thoughtHeadings(item.text).at(-1);
      return heading ? `Thinking · ${heading}` : "Thinking";
    }
    if (item.kind === "assistant") return "Writing the reply";
  }
  return "Working";
}

/**
 * A tool call that starts a subagent (Claude Code's `Agent`/`Task`, RustCode's
 * `spawn_agent`): who it is and what it was asked.
 */
export function subagentOf(item: TranscriptItem): { name: string; task: string } | undefined {
  if (item.kind !== "tool") return undefined;
  const input = item.input && typeof item.input === "object" && !Array.isArray(item.input) ? item.input : {};
  if (item.title === "spawn_agent") {
    const type = typeof input.agent_type === "string" ? input.agent_type : "";
    const task = typeof input.task === "string" ? input.task.split("\n")[0]! : (item.summary ?? "");
    return { name: type && type !== "default" ? type : "Agent", task };
  }
  if (item.toolName !== "Agent" && item.toolName !== "Task") return undefined;
  const type = typeof input.subagent_type === "string" ? input.subagent_type : "";
  const description = typeof input.description === "string" ? input.description : "";
  return {
    name: type && type !== "general-purpose" ? type : "Agent",
    task: description || item.title,
  };
}

/**
 * The items without the steps subagents made, and those steps by the call that
 * started each subagent. A step whose parent is not in the list stays put.
 */
export function splitSubagentSteps(items: TranscriptItem[]): {
  items: TranscriptItem[];
  steps: ReadonlyMap<string, ToolItem[]>;
} {
  if (!items.some((i) => i.kind === "tool" && i.parent)) return { items, steps: NO_STEPS };
  const ids = new Set(items.flatMap((i) => (i.kind === "tool" ? [i.id] : [])));
  const steps = new Map<string, ToolItem[]>();
  const rest: TranscriptItem[] = [];
  for (const item of items) {
    if (item.kind === "tool" && item.parent && ids.has(item.parent)) {
      const list = steps.get(item.parent);
      if (list) list.push(item);
      else steps.set(item.parent, [item]);
    } else rest.push(item);
  }
  return { items: rest, steps };
}

type ToolItem = Extract<TranscriptItem, { kind: "tool" }>;
const NO_STEPS: ReadonlyMap<string, ToolItem[]> = new Map();

/** A step's one-line label: `Running <command>` for a shell, else its summary or title. */
export function stepLabel(item: ToolItem): string {
  const label = displayTool(item.title, item.input, item.status);
  const summary = usefulSummary(item.summary);
  if (item.toolKind === "execute" || /^(bash|shell)/i.test(item.title)) {
    return `Running ${summary ?? label.summary ?? label.title}`;
  }
  return summary ?? label.summary ?? label.title;
}

export function thoughtHeadings(text: string): string[] {
  return [...text.matchAll(/\*\*([^*\n]+)\*\*/g)].map((m) => m[1]!.trim()).filter(Boolean);
}

export function diffCounts(diffs: FileDiff[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const diff of diffs) {
    for (const line of diffLines(diff)) {
      if (line.kind === "added") added++;
      else if (line.kind === "removed") removed++;
    }
  }
  return { added, removed };
}

export interface DiffLine {
  kind: "context" | "added" | "removed";
  text: string;
  oldNumber?: number;
  newNumber?: number;
}

/** A line diff of a hunk's old and new text (LCS; hunks are small, and big ones fall back to remove+add). */
export function diffLines(diff: FileDiff): DiffLine[] {
  const oldLines = diff.oldText === undefined || diff.oldText === "" ? [] : diff.oldText.split("\n");
  const newLines = diff.newText === "" ? [] : diff.newText.split("\n");
  const n = oldLines.length;
  const m = newLines.length;
  const out: DiffLine[] = [];
  if (n * m > 400_000) {
    oldLines.forEach((text, i) => {
      out.push({ kind: "removed", text, oldNumber: diff.oldStart + i });
    });
    newLines.forEach((text, i) => {
      out.push({ kind: "added", text, newNumber: diff.newStart + i });
    });
    return out;
  }
  const lcs: Uint32Array[] = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i]![j] =
        oldLines[i] === newLines[j] ? lcs[i + 1]![j + 1]! + 1 : Math.max(lcs[i + 1]![j]!, lcs[i]![j + 1]!);
    }
  }
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && oldLines[i] === newLines[j]) {
      out.push({
        kind: "context",
        text: oldLines[i]!,
        oldNumber: diff.oldStart + i,
        newNumber: diff.newStart + j,
      });
      i++;
      j++;
    } else if (i < n && (j >= m || lcs[i + 1]![j]! >= lcs[i]![j + 1]!)) {
      out.push({ kind: "removed", text: oldLines[i]!, oldNumber: diff.oldStart + i });
      i++;
    } else {
      out.push({ kind: "added", text: newLines[j]!, newNumber: diff.newStart + j });
      j++;
    }
  }
  return out;
}
