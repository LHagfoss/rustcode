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
import type { ComposeExtras, SentAttachment } from "./compose";

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
    }
  | { kind: "notice"; id: string; text: string; tone: "warning" | "error" | "info"; resumable?: boolean }
  | {
      kind: "turnEnd";
      id: string;
      elapsedMs?: number;
      at?: number;
      outcome: "completed" | "cancelled" | "failed";
    };

export interface QueuedPrompt {
  id: string;
  text: string;
  extras?: ComposeExtras;
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
  /** The replay of `session/load` is still arriving. */
  loading: boolean;
  loaded: boolean;
  harness?: { id: string; capabilities: HarnessCapabilities };
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

/** Marks every open thought as done, so its clock stops. */
function finishThoughts(items: TranscriptItem[], now: number): TranscriptItem[] {
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

function replaceLast(items: TranscriptItem[], item: TranscriptItem): TranscriptItem[] {
  const next = items.slice();
  next[next.length - 1] = item;
  return next;
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
        items: [...items, { kind: "user", id: localId("user"), text: update.text, at: update.at }],
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
        items: [
          ...items,
          {
            kind: "assistant",
            id: localId("reply"),
            text: update.text.replace(/^\n+/, ""),
            at: update.at ?? now,
          },
        ],
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
        items: [
          ...state.items,
          {
            kind: "thought",
            id: localId("thought"),
            text: update.text,
            summary: update.summary,
            startedAt: now,
            durationMs: update.durationMs,
            // A replayed thought carries its duration and is already over.
            done: update.durationMs !== undefined && !state.running,
          },
        ],
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
      };
      if (existing !== undefined && items[existing]?.kind === "tool") {
        const next = items.slice();
        next[existing] = { ...(items[existing] as Extract<TranscriptItem, { kind: "tool" }>), ...tool };
        return { ...state, items: next };
      }
      return {
        ...state,
        items: [...items, tool],
        toolIndex: { ...state.toolIndex, [update.id]: items.length },
      };
    }
    case "toolCallUpdate": {
      const index = state.toolIndex[update.id];
      const item = index === undefined ? undefined : state.items[index];
      if (index === undefined || item?.kind !== "tool") return state;
      const next = state.items.slice();
      next[index] = {
        ...item,
        title: update.title ?? item.title,
        status: update.status ?? (item.status === "pending" ? "in_progress" : item.status),
        output:
          update.output ??
          (update.outputDelta !== undefined ? (item.output ?? "") + update.outputDelta : item.output),
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

/** A turn ended: thoughts stop, running tools are marked, and a footer row is added. */
export function finishTurn(
  state: ThreadState,
  outcome: "completed" | "cancelled" | "failed",
  now = Date.now(),
  error?: string,
): ThreadState {
  const items = finishThoughts(state.items, now).map((item) =>
    item.kind === "tool" && (item.status === "pending" || item.status === "in_progress")
      ? { ...item, status: (outcome === "completed" ? "completed" : "failed") as ToolStatus }
      : item,
  );
  const elapsedMs = state.turnStartedAt !== undefined ? now - state.turnStartedAt : undefined;
  return {
    ...state,
    running: false,
    turnStartedAt: undefined,
    pendingTurn: undefined,
    permissions: [],
    error,
    items: [...items, { kind: "turnEnd", id: localId("turn"), elapsedMs, at: now, outcome }],
  };
}

/** The replay of `session/load` finished. */
export function finishLoad(state: ThreadState): ThreadState {
  const closed = closePendingTurn(state);
  return { ...closed, loading: false, loaded: true, items: finishThoughts(closed.items, Date.now()) };
}

/** Add a steered message where the running turn will read it. */
export function appendSteer(state: ThreadState, text: string, now = Date.now()): ThreadState {
  return {
    ...state,
    items: [...state.items, { kind: "user", id: localId("user"), text, at: now, steered: true }],
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

// ---------------------------------------------------------------- derived values

export function formatDuration(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
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
