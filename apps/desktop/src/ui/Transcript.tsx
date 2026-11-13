// The virtualized transcript. Rows are measured as they render; the view
// stays pinned to the bottom while the reply streams unless the reader has
// scrolled up, and a "Latest" button brings them back.

import { measureElement, useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown, ChevronDown, ChevronUp, RotateCw, Search, X } from "lucide-react";
import {
  memo,
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { modelName, type PermissionRequest, type PlanEntry } from "../acp/types";
import { type Checkpoint, checkpointsByTurn } from "../acp/v2";
import { observeSize } from "../lib/resize";
import { flyInFromComposer, takesFlight } from "../lib/sendFlight";
import { hasTimeline } from "../lib/trace";
import { checkpointsByMessage } from "../state/attempts";
import { type Branch, branchesOf } from "../state/branches";
import { COLLAPSED_MESSAGE_HEIGHT, messageCollapses } from "../state/compose";
import {
  collapseNotices,
  collapseRepeats,
  foldEnds,
  foldTurns,
  footerChangeTurns,
  groupTools,
  keepUnchangedRows,
  newestTurnEnd,
  sameShallow,
  type ToolGroup,
  type TurnChangeLine,
  type TurnFold,
  turnChangeLine,
  turnFoldKey,
  turnItemsByEnd,
} from "../state/fold";
import { selectOption } from "../state/harness";
import type { Link } from "../state/orchestration";
import { type ChildRequest, useChildRequests } from "../state/pending";
import { queueView, type SteeringCapabilities, type TurnState } from "../state/queue";
import { type PlanClocks, planCardShown, type SteerNote, steerMarks } from "../state/steer";
import {
  openSideChat,
  RESUME_TEXT,
  rewindCheckpoint,
  shortcutFor,
  spokenTitleOf,
  steeringOf,
  turnState,
  useApp,
} from "../state/store";
import {
  type QueuedPrompt,
  splitSubagentSteps,
  startedLinks,
  subagentOf,
  type ThreadState,
  type TranscriptItem,
  type TurnCompletion,
  turnCompletion,
  workingLabel,
} from "../state/thread";
import { useWaiting, type Waiting, waitingLabel } from "../state/waiting";
import { StartedThreadsRow, SubagentGroupRow, SubagentRow } from "./AgentViews";
import { ApprovalRow, ChildRequestCard } from "./AttentionCards";
import { BranchRow } from "./Branches";
import { EarlierAttempts } from "./EarlierAttempts";
import { PlanCard } from "./PlanCard";
import { cx, noTextHelpers } from "./primitives";
import { LeavingQueuedRow, QueuedMessageRow, ResumeQueueRow } from "./QueuedMessages";
import {
  AssistantRow,
  CompletionCard,
  HandoffRow,
  LiveActivityRow,
  NoticeRow,
  QuestionRow,
  SwitchRow,
  ThoughtRow,
  ToolGroupRow,
  ToolRow,
  TurnEndRow,
  TurnFoldRow,
  UserRow,
  WaitingRow,
  WakeRow,
  WorkingRow,
} from "./Rows";
import { turnFooterHidden } from "./replyActions";
import { SteerMark } from "./SteerMark";
import { plainPreview, type RailNode, TimelineRail, type Turn, turnTitle } from "./TimelineRail";
import { TurnChanges } from "./TurnChanges";
import { TurnTrace } from "./TurnTrace";

export type Row = {
  key: string;
  item?: TranscriptItem;
  /** How many of the same automatic message this row stands for (`collapseRepeats`). */
  repeats?: number;
  /** The running turn's header; on the moment it ends, `done` holds its check. */
  working?: { done?: boolean; elapsedMs?: number };
  /** Two or more subagents in a row, shown as one. */
  agents?: { items: Extract<TranscriptItem, { kind: "tool" }>[]; open: boolean };
  /** The threads a run of `start_session` calls started, shown as one row or card. */
  threads?: { links: Link[]; open: boolean };
  /** A fork or side chat opened from this thread: its lifecycle line (state/branches.ts). */
  branch?: Branch;
  /** The running turn's foot: what it does now, when no row of its own shows it. */
  activity?: string;
  /** The turn ended while the foot was the live line: it settles on a check. */
  activityDone?: boolean;
  /** The thread's background work, when no turn of its own runs (state/waiting.ts). */
  waiting?: Waiting;
  /** A request from a thread below this one, at the foot (state/pending.ts). */
  childRequest?: ChildRequest;
  /** Whether this child card is the first on screen, and so takes the keyboard. */
  childKeys?: boolean;
  fold?: TurnFold;
  group?: ToolGroup;
  inFold?: string;
  /** An approval the thread is waiting on, shown at the call it is about. */
  approval?: PermissionRequest;
  /** Which of the thread's waiting approvals it is, from 1: `2 of 3`. */
  approvalNumber?: number;
  /** The plan a thread keeps to, at the foot: the one a turn works through, and the one it waits on. */
  plan?: {
    threadId: string;
    entries: PlanEntry[];
    running: boolean;
    steers: SteerNote[];
    clocks: PlanClocks;
  };
  /** A message waiting for the thread, at the end of it. */
  queued?: {
    item: QueuedPrompt;
    index: number;
    count: number;
    held: boolean;
    /** The harness's steering record and the turn's state: the rest of the send rule. */
    steering: SteeringCapabilities;
    turn: TurnState;
    from?: string;
  };
  /** The queue's wake turn, as the one quiet line it waits under (its first line, and the whole text). */
  queueWake?: { line: string; text: string };
  /** A steering note the plan cannot hold, drawn under the message it was sent as. */
  steerNote?: SteerNote;
  /** A queued row a steer just took: it is gliding out of the stack on its way to the transcript. */
  leaving?: QueuedPrompt;
  /** Resume, under a queue a stopped or failed turn held. */
  resume?: { threadId: string; count: number };
};

/** The turns the reader opened, by thread: kept while the app runs, so going back keeps them. */
const openFolds = new Map<string, Set<string>>();
const NO_STEPS: Extract<TranscriptItem, { kind: "tool" }>[] = [];
const NO_FOLDS: ReadonlySet<string> = new Set();
const NO_CHECKPOINTS: Record<string, Checkpoint> = {};
const ALL_OPEN: ReadonlySet<string> = { has: () => true } as unknown as ReadonlySet<string>;

// Measured row heights by item id, kept across mounts and threads: reopening a
// thread or scrolling back estimates each row exactly, so the virtualizer does
// not chase estimate errors (each one a re-layout) while pinning to the bottom.
const rowSizes = new Map<string, number>();
const ROW_SIZES_MAX = 20_000;
function rememberSize(key: string, size: number) {
  if (!rowSizes.has(key) && rowSizes.size >= ROW_SIZES_MAX)
    rowSizes.delete(rowSizes.keys().next().value as string);
  rowSizes.set(key, size);
}

// A finished turn's summary never changes: computed once per footer row, not per streamed chunk.
const completionCache = new WeakMap<TranscriptItem, TurnCompletion | null>();

function useNow(active: boolean) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!active) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [active]);
  return now;
}

/** How long the finished turn's live header holds its check before the fold replaces it. */
const TURN_SETTLE_MS = 400;

/**
 * The turn that just completed: its footer's id, for a moment after `running` falls and a new
 * footer appears, so its live header can settle on a check instead of vanishing. A thread switch,
 * the next turn, or a stop that added no footer clears it.
 */
function useTurnSettle(threadId: string, running: boolean, endId: string | undefined): string | undefined {
  const [settling, setSettling] = useState<string | undefined>(undefined);
  const before = useRef({ threadId, running, endId });
  useLayoutEffect(() => {
    const previous = before.current;
    before.current = { threadId, running, endId };
    if (previous.threadId !== threadId || running) {
      setSettling(undefined);
      return;
    }
    if (!previous.running || !endId || endId === previous.endId) return;
    setSettling(endId);
    const timer = setTimeout(() => setSettling(undefined), TURN_SETTLE_MS);
    return () => clearTimeout(timer);
  }, [threadId, running, endId]);
  return settling;
}

/**
 * The height a row starts at, before it is measured. `hidden` rows (a turn footer that renders
 * nothing) take no room: nothing mounts to measure them, so an estimate above zero would push every
 * row below it down and leave a blank strip at the end of the scroll.
 */
/** Whether the reader has text marked: following the stream would scroll it away. */
function selecting(): boolean {
  const s = window.getSelection();
  return !!s && s.rangeCount > 0 && !s.isCollapsed;
}

export function estimatedRowHeight(row: Row | undefined, hidden = false): number {
  if (hidden) return 0;
  if (!row) return 24;
  if (row.approval) return 96;
  if (row.childRequest) return row.childRequest.request.question ? 240 : 52;
  if (row.plan) return 260;
  if (row.fold || row.group) return 26;
  if (row.queued) return 96;
  if (row.resume) return 30;
  if (row.branch) return 24;
  if (row.queueWake) return 26;
  // The turn's own lines: `Working for 12s` is `h-[34px]` over its hairline, and the live activity
  // is `min-h-6`. Left at a generic 24 they measured 39px taller a frame after mounting, and the
  // pinned view took that back in one step — the message just sent jumped with it.
  if (row.working) return 35;
  if (row.activity) return 24;
  const item = row.item;
  if (!item) return 24;
  if (item.kind === "tool" || item.kind === "turnEnd") return 26;
  // A live thought shows its last lines; a finished one is its one line.
  if (item.kind === "thought")
    return item.done ? 17 : 25 + Math.min(THOUGHT_TAIL_LINES, lines(item.text, 55)) * 17;
  // A sent bubble: its pictures and files above it, 12px of padding each way, its text, and the
  // 22px line under it that holds the time (shown on hover, laid out always). A long one folds at
  // `COLLAPSED_MESSAGE_HEIGHT` and keeps its "Show full message" pill, so its estimate never
  // reserves a screenful the row will not use.
  if (item.kind === "user") {
    const body = lines(item.text, 60) * BODY_LINE;
    const text = messageCollapses(body) ? COLLAPSED_MESSAGE_HEIGHT + SHOW_FULL_PILL : body;
    return (item.attachments?.length ? ATTACHMENT_CHIPS : 0) + BUBBLE_PADDING * 2 + text + META_LINE;
  }
  return Math.min(600, 40 + ("text" in item ? item.text.length / 4 : 0));
}

/** The lines a string wraps to in the lane, near enough for an estimate. */
function lines(text: string, perLine: number): number {
  return Math.max(1, Math.ceil(text.length / perLine));
}

/**
 * The air above a row, from the transcript's own two values: a turn starts and a footer ends on the
 * wide one, everything inside a turn sits on the narrow one, and the first line under a divider
 * takes the narrow one. The rows draw it and so does `estimateSize`, because a row that mounts with
 * a size that leaves this out measures a gap taller a frame later — and the pinned transcript takes
 * that difference back in one step, moving every row above it, the message just sent included.
 */
export function rowGap(rows: readonly Row[], index: number): number {
  if (index <= 0) return 0;
  const item = rows[index]?.item;
  const prev = rows[index - 1]?.item;
  if (
    (prev?.kind === "notice" && prev.tone === "info") ||
    (prev?.kind === "user" && !!(prev.wake || prev.continued))
  )
    return ITEM_GAP;
  return item?.kind === "user" || prev?.kind === "user" ? TURN_GAP : ITEM_GAP;
}

function searchable(item: TranscriptItem): string {
  switch (item.kind) {
    case "user":
    case "assistant":
    case "thought":
    case "notice":
      return item.text;
    case "question":
      return `${item.header ?? ""} ${item.text} ${item.answer}`;
    case "tool":
      return `${item.title} ${item.summary ?? ""} ${item.detail ?? ""} ${item.output ?? ""}`;
    default:
      return "";
  }
}

/** Marks every visible occurrence of `query` with the CSS highlight `rc-find` (current one: `rc-find-current`). */
function paintMatches(root: HTMLElement | null, query: string, current: HTMLElement | null) {
  const highlights = (globalThis.CSS as unknown as { highlights?: Map<string, unknown> })?.highlights;
  const HighlightCtor = (globalThis as unknown as { Highlight?: new (...r: Range[]) => unknown }).Highlight;
  if (!highlights || !HighlightCtor) return;
  highlights.delete("rc-find");
  highlights.delete("rc-find-current");
  if (!root || !query) return;
  const all: Range[] = [];
  const mine: Range[] = [];
  const needle = query.toLowerCase();
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const text = node.nodeValue?.toLowerCase() ?? "";
    let at = text.indexOf(needle);
    while (at >= 0) {
      const range = document.createRange();
      range.setStart(node, at);
      range.setEnd(node, at + needle.length);
      (current?.contains(node) ? mine : all).push(range);
      at = text.indexOf(needle, at + needle.length);
    }
  }
  highlights.set("rc-find", new HighlightCtor(...all));
  highlights.set("rc-find-current", new HighlightCtor(...mine));
}

/**
 * Rows ride left with the composer when the details card needs the lane's end.
 * The `translate` property, not `transform`: a row's own rise-in animation
 * animates `transform`, and the two must not overwrite each other.
 */
const LANE_MOTION = "transition-[translate] duration-200 ease-snappy";

/**
 * The transcript's vertical rhythm, in two values: 14px between the items of a
 * turn (narration, a tool row, a group, the answer) and 28px between turns, which
 * is where a message starts and a footer ends. A row draws no air of its own, so
 * these two decide every gap in the lane and nothing reserves room for a hover.
 */
const ITEM_GAP = 14;
const TURN_GAP = 28;

/**
 * The pieces an estimate is made of, in the sizes the rows are drawn at: a sent bubble's padding and
 * the line it sits on, the time line under it, the chips above it when it carries pictures or files,
 * and how much of a live thought's tail shows (`ThoughtRow`'s `line-clamp-5`).
 */
const BUBBLE_PADDING = 12;
const BODY_LINE = 23;
const META_LINE = 24;
const ATTACHMENT_CHIPS = 32;
const THOUGHT_TAIL_LINES = 5;
/** The "Show full message" pill a folded bubble ends with, and its air above it. */
const SHOW_FULL_PILL = 30;

/** The air above the transcript's first row, under the glass header. */
export const FIRST_ROW_PAD = 20;

/**
 * How far past the live end the view rests when an earlier attempt sits below it: enough for that
 * attempt's divider and the first lines of the turn it kept to read above the composer.
 */
const ATTEMPT_PEEK = 100;

/** A turn's change line, wherever it draws: the completion card, else the checkpoint chip. */
function ChangeLine({ threadId, line }: { threadId: string; line: TurnChangeLine }) {
  return line.kind === "completion" ? (
    <CompletionCard completion={line.completion} />
  ) : (
    <TurnChanges threadId={threadId} checkpoint={line.checkpoint} />
  );
}

/**
 * A finished turn's footer. Memoised, and handed only what stays the same while another turn
 * streams (its item, its slice of the thread, its change line), so a chunk does not redraw it.
 */
const TurnFooter = memo(function TurnFooter({
  item,
  folded,
  timeInFold,
  threadId,
  reply,
  completion,
  checkpoint,
  items,
  model,
}: {
  item: Extract<TranscriptItem, { kind: "turnEnd" }>;
  folded: boolean;
  timeInFold: boolean;
  threadId: string;
  reply?: string;
  completion?: TurnCompletion;
  checkpoint?: Checkpoint;
  items?: TranscriptItem[];
  model?: string;
}) {
  return (
    <TurnEndRow
      item={item}
      folded={folded}
      timeInFold={timeInFold}
      threadId={threadId}
      reply={reply}
      completion={completion}
      changes={checkpoint ? <TurnChanges threadId={threadId} checkpoint={checkpoint} /> : undefined}
      trace={hasTimeline(item) ? <TurnTrace end={item} items={items} model={model} /> : undefined}
    />
  );
});

/** Offer an action only for a selection contained in this message. */
function SelectableMessage({ threadId, children }: { threadId: string; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const [selection, setSelection] = useState("");
  const supported = useApp((s) => !!s.agent?.lineage);
  const capture = useCallback(() => {
    const selected = window.getSelection();
    const element = ref.current;
    setSelection(
      selected &&
        element &&
        selected.anchorNode &&
        selected.focusNode &&
        element.contains(selected.anchorNode) &&
        element.contains(selected.focusNode)
        ? selected.toString()
        : "",
    );
  }, []);
  useEffect(() => {
    if (!selection) return;
    document.addEventListener("selectionchange", capture);
    return () => document.removeEventListener("selectionchange", capture);
  }, [selection, capture]);
  return (
    <div ref={ref} onMouseUp={capture} onKeyUp={capture}>
      {children}
      {supported && selection.trim() && (
        <button
          type="button"
          className="rounded-md px-2 py-1 text-caption text-fg-muted hover:bg-fill hover:text-fg"
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => {
            void openSideChat(threadId, selection);
            setSelection("");
          }}
        >
          Ask in a side chat
        </button>
      )}
    </div>
  );
}

export function Transcript({
  thread,
  bottomInset,
  topInset = 0,
  peek = false,
}: {
  thread: ThreadState;
  /** Room the floating composer stack takes at the foot. */
  bottomInset: number;
  /** Room the glass header takes at the top: the rows scroll under it. */
  topInset?: number;
  /** Shown beside another thread (ui/ThreadPeek.tsx): it reads, and leaves ⌘F and editing to the open thread. */
  peek?: boolean;
}) {
  const [find, setFind] = useState<{ query: string; index: number } | null>(null);
  const findField = useRef<HTMLInputElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const innerRef = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  const [showLatest, setShowLatest] = useState(false);
  const now = useNow(thread.running);
  const latestEndId = thread.items.findLast((item) => item.kind === "turnEnd")?.id;
  const settlingId = useTurnSettle(thread.id, thread.running, latestEndId);

  // Finished turns fold into one summary row each (state/fold.ts); find opens them all so it can reach every step.
  const [open, setOpen] = useState<ReadonlySet<string>>(() => openFolds.get(thread.id) ?? NO_FOLDS);
  const toggleFold = useCallback(
    (key: string) =>
      setOpen((prev) => {
        const next = new Set(prev);
        if (!next.delete(key)) next.add(key);
        openFolds.set(thread.id, next);
        return next;
      }),
    [thread.id],
  );
  const searching = !!find?.query;
  // The turn that just ended, while its finished header can still hold its check: the latest end
  // is a completed turn and its fold is closed (an open fold already shows the steps).
  const settleEnd = useMemo(() => {
    if (!settlingId || thread.running) return undefined;
    const end = thread.items.find((item) => item.id === settlingId);
    if (end?.kind !== "turnEnd" || end.outcome !== "completed") return undefined;
    return (searching ? ALL_OPEN : open).has(turnFoldKey(end.id)) ? undefined : end;
  }, [settlingId, thread.running, thread.items, searching, open]);
  // A subagent's steps fold into its own row instead of filling the thread.
  const { items: visible, steps: subagentSteps } = useMemo(
    () => splitSubagentSteps(thread.items),
    [thread.items],
  );
  // The threads this one started, by the call that started them (its book of links).
  const links = useApp((s) => s.links.links);
  // No turn runs, but the threads it started or messaged still work: not finished, not working.
  const wait = useWaiting(thread.id);
  // What the threads below this one wait on: answered here, in the same cards their own chat shows.
  const childRequests = useChildRequests(thread.id);
  // The branches as a string, so a status change elsewhere in the list does not rebuild the rows.
  const branchKey = useApp((s) => JSON.stringify(branchesOf(s.sessions, thread.id)));
  const spokenTitle = useApp((s) => spokenTitleOf(s, thread.id));
  const branches = useMemo<Branch[]>(() => JSON.parse(branchKey), [branchKey]);
  const started = useMemo(
    () =>
      startedLinks(
        visible,
        links.filter((l) => l.orchestrator === thread.id),
      ),
    [visible, links, thread.id],
  );
  // The rows the last render drew: an unchanged row keeps its object, so its memoised component
  // skips a streamed chunk (state/fold.ts keepUnchangedRows).
  const drawnRows = useRef<Row[]>([]);
  const rows = useMemo<Row[]>(() => {
    // Finished turns fold, then runs of finished tool calls inside what shows fold into one row each,
    // and the same quiet notice said back to back reads as one row with its count.
    const shown = searching ? ALL_OPEN : open;
    const grouped: Row[] = collapseRepeats(
      collapseNotices(groupTools(foldTurns(visible, thread.running, shown), shown)),
      (item) => item.kind === "user" && item.text === RESUME_TEXT,
    ).filter(
      // The turn that just finished keeps its live header for a moment instead of its fold
      // divider: the header settles on a check and then becomes this divider.
      (row) => !(settleEnd && "fold" in row && row.fold?.key === turnFoldKey(settleEnd.id)),
    );
    // Subagents started together read as one row (`3 agents · 2 working`), and so do the
    // threads a run of `start_session` calls started (`2 threads · done`).
    const kindOf = (r?: Row): "agents" | "threads" | undefined => {
      const item = r?.item;
      if (item?.kind !== "tool") return undefined;
      if (subagentOf(item) || subagentSteps.has(item.id)) return "agents";
      return started.has(item.id) ? "threads" : undefined;
    };
    const out: Row[] = [];
    for (let i = 0; i < grouped.length; ) {
      const kind = kindOf(grouped[i]);
      let j = i;
      if (kind) {
        while (j < grouped.length && kindOf(grouped[j]) === kind && grouped[j]!.inFold === grouped[i]!.inFold)
          j++;
      }
      if (kind && j - i >= 2) {
        const key = `${kind}-${grouped[i]!.key}`;
        const items = grouped.slice(i, j).map((r) => r.item as Extract<TranscriptItem, { kind: "tool" }>);
        out.push({
          key,
          inFold: grouped[i]!.inFold,
          ...(kind === "agents"
            ? { agents: { items, open: shown.has(key) } }
            : {
                threads: { links: items.flatMap((item) => started.get(item.id) ?? []), open: shown.has(key) },
              }),
        });
        i = j;
        continue;
      }
      const single = kind === "threads" ? started.get(grouped[i]!.item!.id) : undefined;
      if (single) {
        // One started thread is a row of its own, and stays hidden until its thread settles.
        out.push({
          key: grouped[i]!.key,
          inFold: grouped[i]!.inFold,
          threads: { links: [single], open: false },
        });
      } else {
        out.push(grouped[i]!);
      }
      i++;
    }
    // The forks and side chats opened from here, after the turns that were done when they were
    // opened. The history stores no row for them: they come from the threads' recorded parents.
    if (branches.length) {
      const lastEnd = out.findLastIndex((r) => r.item?.kind === "turnEnd");
      const rowsFor = branches.map((branch): Row => ({ key: `branch-${branch.id}`, branch }));
      out.splice(lastEnd < 0 ? out.length : lastEnd + 1, 0, ...rowsFor);
    }
    // A pending approval sits at the call it is about, so what it wants is read where the work
    // stopped; one whose call is not on screen (yet, or folded away) goes to the foot, where it
    // can still be answered.
    const loose: Row[] = [];
    let waiting = 0;
    for (const request of thread.permissions) {
      // A question keeps its form over the composer (its answer is a choice); it leaves a row
      // only once it is answered.
      if (request.question) continue;
      waiting++;
      const at = out.findIndex((r) => r.item?.kind === "tool" && r.item.id === request.toolCallId);
      const row: Row = { key: `approval-${request.rpcId}`, approval: request, approvalNumber: waiting };
      if (at >= 0) out.splice(at + 1, 0, row);
      else loose.push(row);
    }
    out.push(...loose);
    // A quiet turn is the app's own (an automatic compaction): it draws no working line, so the
    // transcript the reader is on does not move for it, and the notice it leaves is the record.
    if ((thread.running && !thread.quiet) || settleEnd) {
      // `Working for 12s` heads the turn, under the message that started it; when the turn just
      // ended, the same row holds a green check for a moment instead of vanishing.
      let at = out.length;
      for (let i = out.length - 1; i >= 0; i--) {
        if (out[i]!.item?.kind === "user") {
          at = i + 1;
          break;
        }
      }
      out.splice(at, 0, {
        key: "working",
        working: { done: !thread.running, elapsedMs: settleEnd?.elapsedMs },
      });
      // The foot says what it does now unless the last row already does (a call
      // or thought still running, a reply streaming).
      const last = out.filter((row) => row.item?.kind !== "turnEnd").at(-1)?.item;
      const shows =
        (last?.kind === "tool" && (last.status === "in_progress" || last.status === "pending")) ||
        (last?.kind === "thought" && !last.done) ||
        last?.kind === "assistant";
      // With an approval waiting, the turn is not working: the row that asks is the foot.
      const activity = workingLabel(visible);
      if (!shows && !waiting && !(settleEnd && activity === "Working"))
        out.push({
          key: "activity",
          activity,
          activityDone: !!settleEnd,
        });
    } else if (wait) out.push({ key: "waiting", waiting: wait });
    // What a thread below this one asks sits at the foot with the waiting line, oldest first; the
    // first one that can take the keyboard does, so a key never answers another thread's card too.
    const ownKeys = !thread.permissions.length;
    childRequests.forEach((childRequest, i) => {
      out.push({
        key: `child-${childRequest.thread}-${childRequest.request.rpcId}`,
        childRequest,
        childKeys: ownKeys && i === 0,
      });
    });
    // The plan a thread works to, at the foot: while a turn runs it is live (the step it is on, and
    // the notes it was handed there), and once the thread waits on you it keeps what is left and
    // Implement. A plan with nothing left still shows where a steering note landed.
    if (planCardShown(thread.plan, thread.steers))
      out.push({
        key: "plan",
        plan: {
          threadId: thread.id,
          entries: thread.plan,
          running: thread.running,
          steers: thread.steers,
          clocks: thread.planClocks,
        },
      });
    // What waits for the thread comes after all of it: its queued messages, the one quiet line a
    // wake turn waits under, and Resume when a stopped or failed turn held them. They scroll with
    // the transcript, and the pinned view follows them like any other row.
    const queue = queueView(thread);
    const waking = thread.queue.find((q) => q.wake);
    if (queue.wake && waking)
      out.push({ key: "queue-wake", queueWake: { line: queue.wake, text: waking.text } });
    const steering = steeringOf(thread);
    const turn = turnState(thread);
    queue.messages.forEach((q, i) => {
      out.push({
        key: `queued-${q.id}`,
        queued: {
          item: q,
          index: i,
          count: queue.messages.length,
          held: queue.held,
          steering,
          turn,
          from: q.link ? links.find((l) => l.id === q.link)?.orchestratorTitle : undefined,
        },
      });
    });
    if (queue.resumable)
      out.push({ key: "queue-resume", resume: { threadId: thread.id, count: queue.messages.length } });
    // Rows a steer just took keep the stack's place while they glide up toward the transcript.
    for (const item of thread.departing ?? []) out.push({ key: `leaving-${item.id}`, leaving: item });
    // A steering note the plan does not hold says where it took effect, under the message it was
    // sent as. It is its own row, outside the turn's fold, so it stays readable once that folds.
    for (const note of steerMarks(thread.steers, thread.plan).inline) {
      const at = out.findIndex((r) => r.item?.id === note.id);
      if (at >= 0) out.splice(at + 1, 0, { key: `steer-${note.id}`, steerNote: note });
      else out.push({ key: `steer-${note.id}`, steerNote: note });
    }
    drawnRows.current = keepUnchangedRows(drawnRows.current, out);
    return drawnRows.current;
  }, [
    visible,
    subagentSteps,
    started,
    links,
    thread,
    open,
    searching,
    wait,
    childRequests,
    settleEnd,
    branches,
  ]);
  // Footers of folded turns: their time is in the summary row, not repeated. The turn that just
  // finished still counts as folded while its live header holds the place of that summary row.
  const foldedEnds = useMemo(() => {
    const set = new Set<string>();
    for (const row of rows) if (row.fold) set.add(row.fold.key.slice("fold-".length));
    if (settleEnd) set.add(settleEnd.id);
    return set;
  }, [rows, settleEnd]);

  // Folded turns that wrote no answer: their summary row says the time, so the footer leaves it out.
  // Only when the header itself carries a time (`TurnFoldRow`): a `turnEnd` without one would
  // otherwise lose it altogether.
  const timeInFold = useMemo(() => {
    const set = new Set<string>();
    for (const row of rows)
      if (row.fold && !row.fold.answered && row.fold.time !== undefined)
        set.add(row.fold.key.slice("fold-".length));
    return set;
  }, [rows]);

  // The reply each turn footer copies and forks: the last reply before it. Nothing under that
  // reply draws actions, so a finished turn shows one action row, at its foot.
  const turnReply = useMemo(() => {
    const byEnd: Record<string, { id: string; text: string }> = {};
    let last: { id: string; text: string } | undefined;
    for (const item of thread.items) {
      if (item.kind === "assistant") last = { id: item.id, text: item.text };
      else if (item.kind === "user") last = undefined;
      else if (item.kind === "turnEnd" && last) byEnd[item.id] = last;
    }
    return { byEnd };
  }, [thread.items]);

  const completions = useMemo(() => {
    const map: Record<string, TurnCompletion | null> = {};
    thread.items.forEach((item, i) => {
      if (item.kind !== "turnEnd") return;
      let done = completionCache.get(item);
      if (done === undefined) {
        done = turnCompletion(thread.items, i);
        completionCache.set(item, done);
      }
      map[item.id] = done;
    });
    return map;
  }, [thread.items]);
  // How many approvals wait: the rows and the bar both say "1 of N" from the same count.
  const approvals = thread.permissions.filter((p) => !p.question).length;
  const rewind = useApp((s) => !!s.agent?.rewind) && (thread.harness?.capabilities.rewind ?? true);
  const checkpoints = useApp((s) => s.checkpoints[thread.id]);
  // Each finished turn's checkpoint, for its Changes chip.
  const turnCheckpoints = useMemo(() => {
    if (!checkpoints?.length) return NO_CHECKPOINTS;
    const turns: { id: string; prompt: string; at?: number }[] = [];
    let prompt = "";
    for (const item of thread.items) {
      if (item.kind === "user") prompt = item.text;
      else if (item.kind === "turnEnd") turns.push({ id: item.id, prompt, at: item.at });
    }
    return checkpointsByTurn(turns, checkpoints);
  }, [thread.items, checkpoints]);

  // The turns an earlier rewind abandoned, faded under their divider after the live transcript
  // (ui/EarlierAttempts.tsx): a record of the attempt this thread left, not part of it.
  const attempts = useApp((s) => s.attempts[thread.id]);

  // The newest finished turn keeps its actions and its change line in view; the older ones wait
  // for a hover, and their changes fold away with their steps (state/fold.ts, turnChangeLine).
  const lastEnd = useMemo(() => newestTurnEnd(rows), [rows]);

  // Each finished turn's one change line: the completion card where the harness reported one,
  // else the checkpoint chip. Never both, so a turn shows one set of numbers.
  const changeLines = useMemo(() => {
    const map: Record<string, TurnChangeLine> = {};
    for (const item of thread.items) {
      if (item.kind !== "turnEnd") continue;
      const line = turnChangeLine({
        completion: completions[item.id],
        checkpoint: turnCheckpoints[item.id],
      });
      if (line) map[item.id] = line;
    }
    return map;
  }, [thread.items, completions, turnCheckpoints]);

  // Which turns keep their change line in their own footer: the newest, and one with nothing
  // folded to hold it (state/fold.ts, footerChangeTurns).
  const footerLines = useMemo(() => {
    const ends = thread.items.flatMap((item) => (item.kind === "turnEnd" ? [item.id] : []));
    // A reply-only turn's divider folds nothing away, so its footer keeps the change line.
    const folded = new Set(foldedEnds);
    for (const row of rows)
      if (row.fold && !row.fold.steps) folded.delete(row.fold.key.slice("fold-".length));
    return new Set(footerChangeTurns(ends, { last: lastEnd, folded }));
  }, [thread.items, lastEnd, foldedEnds, rows]);

  // An older turn with steps draws its change line as the last row inside its own fold, so it
  // appears only when that turn is opened.
  const foldChanges = useMemo(() => {
    const map = new Map<number, ReactNode>();
    const ends = foldEnds(rows);
    for (const item of thread.items) {
      if (item.kind !== "turnEnd" || footerLines.has(item.id)) continue;
      const at = ends[turnFoldKey(item.id)];
      const line = changeLines[item.id];
      if (at === undefined || !line) continue;
      map.set(at, <ChangeLine threadId={thread.id} line={line} />);
    }
    return map;
  }, [rows, thread.items, changeLines, footerLines, thread.id]);

  // Those turns' lines as their footer's props: the completion card, else the checkpoint chip.
  const footerLineOf = (id: string): { completion?: TurnCompletion; checkpoint?: Checkpoint } => {
    const line = footerLines.has(id) ? changeLines[id] : undefined;
    if (!line) return {};
    return line.kind === "completion" ? { completion: line.completion } : { checkpoint: line.checkpoint };
  };

  // The thread's model names its Trace's model calls (state/thread.ts TurnUsage).
  const model = useMemo(
    () => modelName(selectOption(thread.configOptions, "model")?.current),
    [thread.configOptions],
  );

  // Each finished turn's own rows, for its Trace chip: everything since the
  // turn before it, so a wake turn (which has no message of its own) comes out
  // whole. The trace keeps only what falls between its start and its end.
  // A finished turn keeps its array, so its memoised footer skips a streamed chunk.
  const drawnTurnItems = useRef<Record<string, TranscriptItem[]>>({});
  const turnItems = useMemo(() => {
    drawnTurnItems.current = turnItemsByEnd(thread.items, drawnTurnItems.current);
    return drawnTurnItems.current;
  }, [thread.items]);

  // Turn footers that render nothing at all. Nothing measures a row that never mounts, so the
  // virtualizer has to estimate no height for it, or every row below drifts by a footer.
  const footerless = useMemo(() => {
    const set = new Set<string>();
    for (const row of rows) {
      const item = row.item;
      if (item?.kind !== "turnEnd") continue;
      // A turn with a footered change line keeps its footer; an older folded one has none.
      const line = footerLines.has(item.id) ? changeLines[item.id] : undefined;
      const hidden = turnFooterHidden({
        folded: foldedEnds.has(item.id),
        reply: turnReply.byEnd[item.id]?.text,
        completion: line?.kind === "completion",
        changes: line?.kind === "checkpoint",
        trace: hasTimeline(item),
        outcome: item.outcome,
      });
      if (hidden) set.add(item.id);
    }
    return set;
  }, [rows, foldedEnds, turnReply, footerLines, changeLines]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => {
      const row = rows[i];
      // A footer nothing renders never mounts, so no measurement follows to correct an estimate,
      // and one measured while it still drew something must not keep that height.
      const hidden = row?.item?.kind === "turnEnd" && footerless.has(row.item.id);
      if (hidden) return 0;
      const known = rowSizes.get(row?.key ?? "");
      if (known !== undefined) return known;
      // A measured row keeps the gap it was drawn with, so an estimate must carry its own or every
      // row would come up short by one and be corrected a frame later.
      return rowGap(rows, i) + estimatedRowHeight(row, false);
    },
    getItemKey: (i) => rows[i]?.key ?? i,
    // Read lazily, after `attachScroll` put the view at the end (scroll events come later).
    initialOffset: () => scrollRef.current?.scrollTop ?? 0,
    measureElement: (element, entry, instance) => {
      const index = instance.indexFromElement(element);
      const key = rows[index]?.key;
      // A row that just mounted keeps its estimate (exact when seen before); its ResizeObserver
      // entry follows after layout with every other row's. Reading offsetHeight here instead
      // forced a layout per mounted row, each after the re-render the previous one caused.
      if (!entry) return (key && rowSizes.get(key)) || instance.options.estimateSize(index);
      const size = measureElement(element, entry, instance);
      if (key && key !== "working") rememberSize(key, size);
      return size;
    },
    overscan: 8,
    // Row measurements land on the next frame, not inside the ResizeObserver's own delivery.
    useAnimationFrameWithResizeObserver: true,
    paddingStart: FIRST_ROW_PAD + topInset,
    paddingEnd: bottomInset,
  });

  // Starts at the end before the virtualizer first reads the offset (refs attach before layout
  // effects), so it renders the last rows at once instead of the first ones and then scrolling.
  const attachScroll = useCallback((el: HTMLDivElement | null) => {
    scrollRef.current = el;
    if (el) el.scrollTop = el.scrollHeight;
  }, []);

  // The virtualizer's own ref also runs on detach, and each detach walks every element it has
  // seen (O(rows²) when a thread closes); stale elements are dropped once per commit instead.
  const measureRef = useCallback(
    (el: HTMLDivElement | null) => {
      if (el) virtualizer.measureElement(el);
    },
    [virtualizer],
  );
  useEffect(() => {
    virtualizer.measureElement(null);
  });

  const [ready, setReady] = useState(false);
  useLayoutEffect(() => setReady(true), []);

  // An approval (or the plan card) appearing or resolving changes how tall the rows around it
  // are, and the virtualizer's cached measurements are stale until each mounted row reports
  // again: ask it to measure the ones on screen.
  const approvalSig = thread.permissions.map((p) => p.rpcId).join(" ");
  const planSig = thread.plan.map((e) => e.status).join("");
  const childSig = childRequests.map((entry) => `${entry.thread}:${entry.request.rpcId}`).join(" ");
  // biome-ignore lint/correctness/useExhaustiveDependencies: the signature is what changed
  useEffect(() => {
    virtualizer.measure();
  }, [approvalSig, planSig, childSig]);

  // Where the thread's own rows end: the last live row just above the composer, past the earlier
  // attempts a rewind keeps below it, so a new turn is never scrolled into a record the reader is
  // not writing to.
  const liveEnd = useCallback((el: HTMLDivElement) => {
    const inner = innerRef.current;
    return Math.max(0, (inner?.offsetHeight ?? el.scrollHeight) - el.clientHeight);
  }, []);
  // Where the view rests at the thread's end, and what counts as being there. With an earlier
  // attempt below the live end it rests a little past it, far enough that the divider and the
  // first lines of the turn it kept read above the composer rather than behind it.
  const attemptCount = attempts?.length ?? 0;
  const threadEnd = useCallback(
    (el: HTMLDivElement) => liveEnd(el) + (attemptCount ? ATTEMPT_PEEK : 0),
    [attemptCount, liveEnd],
  );
  const toBottom = useCallback(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = threadEnd(el);
  }, [threadEnd]);

  // Opening a thread starts at its end.
  // biome-ignore lint/correctness/useExhaustiveDependencies: runs when the thread changes
  useLayoutEffect(() => {
    pinned.current = true;
    setShowLatest(false);
    virtualizer.scrollToIndex(Math.max(0, rows.length - 1), { align: "end" });
    requestAnimationFrame(toBottom);
  }, [thread.id]);

  // Whatever grows (a streamed chunk, a measured row, the composer) keeps a pinned view at the bottom,
  // unless the reader is marking text: a selection must not scroll out from under them mid-stream.
  useEffect(() => {
    const inner = innerRef.current;
    if (!inner) return;
    const observer = new ResizeObserver(() => {
      if (pinned.current && !selecting()) toBottom();
    });
    observer.observe(inner);
    return () => observer.disconnect();
  }, [toBottom]);

  // A new earlier attempt re-pins through `toBottom`'s own deps; rows and the composer's room have
  // none to read, only a moment to act on.
  // biome-ignore lint/correctness/useExhaustiveDependencies: new rows and a taller composer re-pin
  useLayoutEffect(() => {
    if (pinned.current && !selecting()) toBottom();
  }, [rows, toBottom, bottomInset]);

  // The timeline's turns: one per user message. Kept as the same array while they do not change,
  // so a streamed chunk does not redraw the rail.
  const turnsKey = useRef<{ sig: string; turns: Turn[] }>({ sig: "", turns: [] });
  let sig = "";
  for (let i = 0; i < rows.length; i++) if (rows[i]!.item?.kind === "user") sig += `${i},`;
  if (sig !== turnsKey.current.sig) {
    turnsKey.current = {
      sig,
      turns: rows.flatMap((row, i) =>
        row.item?.kind === "user" ? [{ row: i, id: row.item.id, title: turnTitle(row.item.text) }] : [],
      ),
    };
  }
  const turns = turnsKey.current.turns;
  // Each message you sent that started a turn with a checkpoint: named in that turn's hover card.
  // The same array while the nodes are the same, so a streamed chunk does not redraw the rail.
  const drawnNodes = useRef<RailNode[]>([]);
  const checkpointNodes = useMemo(() => {
    const byMessage = checkpointsByMessage(thread.items, turnCheckpoints);
    const nodes: RailNode[] = [];
    turns.forEach((turn, index) => {
      const checkpoint = byMessage[turn.id];
      if (checkpoint) nodes.push({ at: index, checkpoint });
    });
    if (!sameShallow(drawnNodes.current, nodes, 2)) drawnNodes.current = nodes;
    return drawnNodes.current;
  }, [thread.items, turnCheckpoints, turns]);
  // The rail's card offers the rewind the turn's Changes chip does, when the harness can go back.
  const railRewind = useCallback(
    (checkpoint: Checkpoint) => void rewindCheckpoint(thread.id, checkpoint),
    [thread.id],
  );
  // An engine that keeps what a rewind abandons goes back without a confirm, so the card says so.
  const keepsAttempts = useApp((s) => !!s.agent?.checkpointAttempts);

  // Sending a message follows it down, even from a view scrolled up to read.
  const sentTurns = useRef({ thread: thread.id, count: turns.length });
  useLayoutEffect(() => {
    const before = sentTurns.current;
    sentTurns.current = { thread: thread.id, count: turns.length };
    if (before.thread !== thread.id || turns.length <= before.count) return;
    pinned.current = true;
    setShowLatest(false);
    virtualizer.scrollToIndex(Math.max(0, rows.length - 1), { align: "end" });
    requestAnimationFrame(toBottom);
  }, [turns.length, thread.id, rows.length, toBottom, virtualizer]);
  const [readTurn, setReadTurn] = useState(0);
  const [lastOnScreen, setLastOnScreen] = useState(0);
  // How a turn was answered, read when the minimap asks: the rail does not redraw as a reply streams.
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  const replyOf = useCallback((row: number) => {
    let reply: string | undefined;
    for (let i = row + 1; i < rowsRef.current.length; i++) {
      const item = rowsRef.current[i]!.item;
      if (item?.kind === "user") break;
      if (item?.kind === "assistant") reply = item.text;
    }
    return reply && plainPreview(reply);
  }, []);
  const [size, setSize] = useState({ width: 0, height: 0 });
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    setSize({ width: el.clientWidth, height: el.clientHeight });
    return observeSize(el, () => setSize({ width: el.clientWidth, height: el.clientHeight }));
  }, []);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    // Reading an earlier attempt is below the thread's end: the pill shows there too.
    const end = threadEnd(el);
    const atBottom = Math.abs(el.scrollTop - end) < 48;
    pinned.current = atBottom;
    setShowLatest(!atBottom);
    // The turn being read: the last one whose first line is above 30% of the visible part.
    const reading = el.scrollTop + topInset + (el.clientHeight - bottomInset - topInset) * 0.3;
    const starts = virtualizer.measurementsCache;
    const bottom = el.scrollTop + el.clientHeight - bottomInset;
    let at = 0;
    let onScreen = 0;
    for (let t = 0; t < turns.length; t++) {
      const start = starts[turns[t]!.row]?.start;
      if (start === undefined) continue;
      if (start + 16 <= reading) at = t;
      if (start >= bottom) break;
      onScreen = t;
    }
    setReadTurn(at);
    setLastOnScreen(onScreen);
  };

  const jumpToTurn = useCallback(
    (row: number) => {
      pinned.current = false;
      setShowLatest(true);
      virtualizer.scrollToIndex(row, { align: "start" });
    },
    [virtualizer],
  );

  const railCurrent = showLatest ? Math.min(readTurn, turns.length - 1) : turns.length - 1;

  // What waits at the end while the reader is scrolled up, after the pill's `·`: an approval it
  // needs, what the turn does now, or the agents that finished since. Nothing new is asked of the
  // store, only what the work already said.
  const waits = useMemo(() => {
    if (!showLatest) return undefined;
    const approvals = thread.permissions.filter((p) => !p.question).length;
    if (approvals) return approvals === 1 ? "Approval" : `${approvals} approvals`;
    if (thread.running) return workingLabel(visible);
    if (wait) return waitingLabel(wait);
    // The last turn's subagents, once they have all stopped: what the reader scrolled away from.
    let agents = 0;
    let finished = 0;
    let ended = false;
    for (let i = visible.length - 1; i >= 0; i--) {
      const item = visible[i]!;
      if (item.kind === "user") break;
      if (item.kind === "turnEnd") {
        ended = true;
        continue;
      }
      const agent = item.kind === "tool" && !item.parent ? subagentOf(item) : undefined;
      if (!agent) continue;
      agents++;
      if (item.kind === "tool" && item.status !== "pending" && item.status !== "in_progress") finished++;
    }
    if (!ended || !agents || finished !== agents) return undefined;
    return `${agents} ${agents === 1 ? "agent" : "agents"} finished`;
  }, [thread.permissions, thread.running, showLatest, visible, wait]);

  const onWheel = (e: React.WheelEvent) => {
    // Scrolling up during a stream unpins at once, before the next chunk pulls the view down.
    if (e.deltaY < 0) pinned.current = false;
  };

  const matches = useMemo(() => {
    const q = find?.query.trim().toLowerCase();
    if (!q) return [];
    return rows.flatMap((row, i) => (row.item && searchable(row.item).toLowerCase().includes(q) ? [i] : []));
  }, [rows, find?.query]);
  const currentRow = find && matches.length ? matches[Math.min(find.index, matches.length - 1)] : undefined;

  // Find in this thread, on the key Settings › Keyboard binds to it; Esc closes it.
  useEffect(() => {
    if (peek) return;
    const onKey = (e: KeyboardEvent) => {
      if (shortcutFor(e) === "find-in-thread") {
        e.preventDefault();
        setFind((f) => f ?? { query: "", index: 0 });
        requestAnimationFrame(() => findField.current?.select());
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [peek]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: scroll when the current match changes
  useEffect(() => {
    if (currentRow === undefined) return;
    pinned.current = false;
    virtualizer.scrollToIndex(currentRow, { align: "center" });
  }, [currentRow]);

  // Paint after each render; rows mount as they scroll into view.
  useEffect(() => {
    const current = innerRef.current?.querySelector<HTMLElement>(`[data-index="${currentRow}"]`) ?? null;
    paintMatches(find ? innerRef.current : null, find?.query.trim() ?? "", current);
  });
  useEffect(() => () => paintMatches(null, "", null), []);

  const step = (delta: number) =>
    setFind((f) =>
      f && matches.length ? { ...f, index: (f.index + delta + matches.length) % matches.length } : f,
    );

  // The last row that holds a transcript item: the streaming reply is that one, whatever follows
  // it (the live foot, an approval, the plan).
  const lastIndex = rows.findLastIndex((r) => !!r.item);
  // The first render draws no rows: the virtualizer has no scroll element yet and would pick the
  // thread's first rows. `attachScroll` then puts the view at the end, and the second render
  // (still before the first paint) draws the last rows straight away.
  const items = ready ? virtualizer.getVirtualItems() : [];
  // New rows (a sent message, a reply, a tool call) rise in once. Rows already there when the
  // thread opened do not, and neither does a row the virtualizer remounts on scroll: each key's
  // first sighting is kept, and only a sighting under 300 ms old animates.
  const sightings = useRef<{ thread: string; at: Map<string, number> }>({ thread: "", at: new Map() });
  // A thread whose history arrives after it opens counts that first batch as already there.
  if (sightings.current.thread !== thread.id || (sightings.current.at.size <= 1 && rows.length > 1)) {
    sightings.current = { thread: thread.id, at: new Map(rows.map((r) => [r.key, 0])) };
  }
  const nowMs = performance.now();
  const fresh = (key: string) => {
    const at = sightings.current.at;
    if (!at.has(key)) at.set(key, nowMs);
    return nowMs - at.get(key)! < 300;
  };
  // The message the composer just sent: it rises out of the field rather than only rising in
  // (lib/sendFlight.ts). The item says so itself (thread.ts `beginTurn`), so a thread's history, a
  // wake, a resume, a message from another thread or a view keep the plain row rise, and the flight
  // log keeps one flight per message when the row is redrawn or the thread is opened again.
  const sentRow = rows.findLast((r) => r.item?.kind === "user" && r.item.sent);
  // biome-ignore lint/correctness/useExhaustiveDependencies: the row's key is its item's id, and `rows.length` retries a flight whose row the virtualizer had not drawn yet
  useLayoutEffect(() => {
    const key = sentRow?.key;
    const el = key
      ? (innerRef.current?.querySelector<HTMLElement>(`[data-rc-row="${CSS.escape(key)}"]`) ?? null)
      : null;
    if (!el || !takesFlight(sentRow?.item?.id)) return;
    flyInFromComposer(el);
  }, [sentRow?.key, rows.length]);

  return (
    <div className="relative min-h-0 flex-1">
      {find && (
        <div
          style={{ top: topInset + 8 }}
          className="elevated absolute right-7 z-20 flex h-[32px] items-center gap-1 rounded-full pr-1 pl-3 rc-in"
        >
          <Search size={12} className="text-fg-muted" />
          <input
            ref={findField}
            autoFocus
            value={find.query}
            onChange={(e) => setFind({ query: e.target.value, index: 0 })}
            onKeyDown={(e) => {
              if (e.key === "Enter") step(e.shiftKey ? -1 : 1);
              if (e.key === "Escape") setFind(null);
            }}
            placeholder="Find in thread"
            aria-label="Find in thread"
            {...noTextHelpers}
            className="w-[180px] bg-transparent text-callout outline-none placeholder:text-fg-faint"
          />
          <span className="min-w-[52px] text-right text-caption text-fg-muted tabular" aria-live="polite">
            {find.query.trim()
              ? matches.length
                ? `${Math.min(find.index, matches.length - 1) + 1} of ${matches.length}`
                : "None"
              : ""}
          </span>
          <button
            type="button"
            aria-label="Previous match"
            onClick={() => step(-1)}
            className="flex size-6 items-center justify-center rounded-full text-fg-muted hover:bg-fill"
          >
            <ChevronUp size={13} />
          </button>
          <button
            type="button"
            aria-label="Next match"
            onClick={() => step(1)}
            className="flex size-6 items-center justify-center rounded-full text-fg-muted hover:bg-fill"
          >
            <ChevronDown size={13} />
          </button>
          <button
            type="button"
            aria-label="Close find"
            onClick={() => setFind(null)}
            className="flex size-6 items-center justify-center rounded-full text-fg-muted hover:bg-fill"
          >
            <X size={12} />
          </button>
        </div>
      )}
      <div
        ref={attachScroll}
        onScroll={onScroll}
        onWheel={onWheel}
        className="h-full overflow-y-auto overscroll-contain px-3 [overflow-anchor:none] [scrollbar-gutter:stable_both-edges] sm:px-12"
        role="log"
        // Named for its thread, and silent as it streams: the turn's moments are announced once
        // each (`state/announceThreads.ts`), never every token or tool call a log would read out.
        aria-label={spokenTitle}
        aria-live="off"
        aria-busy={thread.running}
      >
        <div ref={innerRef} className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
          {/* The rendered rows sit in normal flow under one offset: a row that grows pushes the
              ones after it in the same frame. Each row placed at its own measured offset left
              them overlapping it until the measurement landed a frame later (rows jumped). */}
          <div
            className="absolute top-0 left-0 w-full"
            style={{ transform: `translateY(${items[0]?.start ?? 0}px)` }}
          >
            {items.map((v) => {
              const row = rows[v.index]!;
              const item = row.item;
              const prev = rows[v.index - 1];
              // A divider (a wake, a notice) already draws the line a fold header would draw again.
              const afterDivider =
                (prev?.item?.kind === "notice" && prev.item.tone === "info") ||
                (prev?.item?.kind === "user" && !!prev.item.wake) ||
                (prev?.item?.kind === "user" && !!prev.item.continued);
              // The air above the row, from the same answer its estimate reads (`rowGap`), so a row
              // mounts at the size the transcript already reserved for it.
              const turnGap = rowGap(rows, v.index);
              if (row.fold) {
                return (
                  <div key={v.key} data-index={v.index} ref={measureRef} className="w-full">
                    <div
                      className={cx("mx-auto w-full max-w-[var(--rc-chat-content-max-width)]", LANE_MOTION)}
                      style={{ paddingTop: turnGap }}
                    >
                      <TurnFoldRow fold={row.fold} onToggle={toggleFold} rule={!afterDivider} />
                    </div>
                  </div>
                );
              }
              // A turn whose footer has nothing to add leaves no row, and no gap for one.
              if (item?.kind === "turnEnd" && footerless.has(item.id)) return null;
              return (
                <div key={v.key} data-index={v.index} ref={measureRef} className="w-full">
                  <div
                    data-rc-row={row.key === sentRow?.key ? row.key : undefined}
                    className={cx(
                      "mx-auto w-full max-w-[var(--rc-chat-content-max-width)]",
                      LANE_MOTION,
                      row.inFold ? "rc-expand" : fresh(row.key) && row.key !== sentRow?.key && "rc-row-in",
                    )}
                    style={{ paddingTop: turnGap }}
                  >
                    {row.approval ? (
                      <ApprovalRow
                        request={row.approval}
                        pending={approvals}
                        number={row.approvalNumber}
                        project={thread.cwd}
                      />
                    ) : row.childRequest ? (
                      <ChildRequestCard
                        request={row.childRequest.request}
                        from={row.childRequest.title}
                        pending={row.childRequest.pending}
                        number={row.childRequest.number}
                        keys={row.childKeys}
                        project={thread.cwd}
                      />
                    ) : row.plan ? (
                      <PlanCard
                        threadId={row.plan.threadId}
                        entries={row.plan.entries}
                        running={row.plan.running}
                        steers={row.plan.steers}
                        clocks={row.plan.clocks}
                      />
                    ) : row.steerNote ? (
                      <SteerMark note={row.steerNote} />
                    ) : row.leaving ? (
                      <LeavingQueuedRow item={row.leaving} />
                    ) : row.agents ? (
                      <SubagentGroupRow
                        items={row.agents.items}
                        stepsOf={(id) => subagentSteps.get(id) ?? NO_STEPS}
                        threadId={thread.id}
                        open={row.agents.open}
                        onToggle={() => toggleFold(row.key)}
                      />
                    ) : row.threads ? (
                      <StartedThreadsRow
                        links={row.threads.links}
                        open={row.threads.open}
                        onToggle={() => toggleFold(row.key)}
                      />
                    ) : row.group ? (
                      <ToolGroupRow group={row.group} onToggle={toggleFold} />
                    ) : row.branch ? (
                      <BranchRow branch={row.branch} />
                    ) : row.waiting ? (
                      <WaitingRow wait={row.waiting} />
                    ) : row.queued ? (
                      <QueuedMessageRow threadId={thread.id} {...row.queued} />
                    ) : row.queueWake ? (
                      // The same quiet line its turn leaves once it runs: the delegates, their
                      // harnesses and time, the whole message in the title. Never a bubble that
                      // reads as the user's own.
                      <WakeRow text={row.queueWake.text} flat />
                    ) : row.resume ? (
                      <ResumeQueueRow threadId={row.resume.threadId} count={row.resume.count} />
                    ) : row.working ? (
                      <WorkingRow
                        startedAt={thread.turnStartedAt}
                        now={now}
                        done={row.working.done}
                        elapsedMs={row.working.elapsedMs}
                      />
                    ) : row.activity ? (
                      <LiveActivityRow label={row.activity} done={row.activityDone} />
                    ) : item?.kind === "user" && item.text === RESUME_TEXT ? (
                      <div className="flex items-center gap-2 text-caption text-fg-faint">
                        <span className="h-px flex-1 bg-border" />
                        <RotateCw size={11} />
                        Continued after a restart
                        {(row.repeats ?? 1) > 1 && <span className="tabular">×{row.repeats}</span>}
                        <span className="h-px flex-1 bg-border" />
                      </div>
                    ) : item?.kind === "user" ? (
                      <SelectableMessage threadId={thread.id}>
                        <UserRow
                          item={item}
                          threadId={thread.running || peek ? undefined : thread.id}
                          rewind={rewind}
                          // Consecutive messages from the same side soften the corners they share.
                          after={rows[v.index - 1]?.item?.kind === "user"}
                          before={rows[v.index + 1]?.item?.kind === "user"}
                        />
                      </SelectableMessage>
                    ) : item?.kind === "assistant" ? (
                      <SelectableMessage threadId={thread.id}>
                        <AssistantRow
                          item={item}
                          threadId={thread.id}
                          streaming={thread.running && v.index === lastIndex}
                        />
                      </SelectableMessage>
                    ) : item?.kind === "thought" ? (
                      <ThoughtRow item={item} live={thread.running && !item.done && v.index === lastIndex} />
                    ) : item?.kind === "tool" && (subagentOf(item) || subagentSteps.has(item.id)) ? (
                      <SubagentRow
                        item={item}
                        steps={subagentSteps.get(item.id) ?? NO_STEPS}
                        threadId={thread.id}
                      />
                    ) : item?.kind === "tool" ? (
                      <ToolRow item={item} cwd={thread.cwd} />
                    ) : item?.kind === "notice" ? (
                      <NoticeRow item={item} />
                    ) : item?.kind === "handoff" ? (
                      <HandoffRow item={item} />
                    ) : item?.kind === "switch" ? (
                      <SwitchRow item={item} />
                    ) : item?.kind === "question" ? (
                      <QuestionRow item={item} />
                    ) : item?.kind === "turnEnd" ? (
                      <TurnFooter
                        item={item}
                        folded={foldedEnds.has(item.id)}
                        timeInFold={timeInFold.has(item.id)}
                        threadId={thread.id}
                        reply={turnReply.byEnd[item.id]?.text}
                        {...footerLineOf(item.id)}
                        items={turnItems[item.id]}
                        model={model}
                      />
                    ) : null}
                  </div>
                  {/* An older turn's change line rides at the end of its own fold, so it shows
                      only once that turn's `Worked for …` is opened. */}
                  {foldChanges.has(v.index) && (
                    <div
                      className={cx(
                        "mx-auto w-full max-w-[var(--rc-chat-content-max-width)]",
                        LANE_MOTION,
                        "rc-expand",
                      )}
                    >
                      {foldChanges.get(v.index)}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
        {attempts?.length ? (
          <EarlierAttempts
            threadId={thread.id}
            cwd={thread.cwd}
            attempts={attempts}
            bottomInset={bottomInset}
          />
        ) : null}
      </div>
      <TimelineRail
        turns={turns}
        current={railCurrent}
        last={showLatest ? Math.max(railCurrent, Math.min(lastOnScreen, turns.length - 1)) : turns.length - 1}
        width={size.width}
        visible={size.height - bottomInset + 20 - topInset}
        offset={topInset}
        onJump={jumpToTurn}
        replyOf={replyOf}
        nodes={checkpointNodes}
        onRewind={rewind ? railRewind : undefined}
        keepsAttempts={keepsAttempts}
        busy={thread.running}
      />
      <button
        type="button"
        onClick={() => {
          pinned.current = true;
          setShowLatest(false);
          const el = scrollRef.current;
          el?.scrollTo({ top: threadEnd(el), behavior: "smooth" });
        }}
        aria-hidden={!showLatest}
        tabIndex={showLatest ? 0 : -1}
        className={cx(
          // The strip of threads above the composer and this pill are one surface (index.css
          // `.rc-glass-overlay`), so what floats over the transcript reads as one glass. It sits a
          // gap above everything the stack holds (`bottomInset`), and above the composer's drawer
          // when that opens up over the transcript (`--rc-composer-float`, set by ui/Composer.tsx).
          "rc-glass-overlay absolute z-30 flex h-[30px] max-w-[min(360px,calc(100%-32px))] -translate-x-1/2 items-center gap-1.5 rounded-full px-3 font-medium text-callout text-fg transition-[opacity,translate,left,bottom] duration-200 ease-snappy",
          showLatest ? "translate-y-0 opacity-100" : "pointer-events-none translate-y-1 opacity-0",
        )}
        style={{
          bottom: `max(${bottomInset - 8}px, calc(var(--rc-composer-float, 0px) + 8px))`,
          left: "50%",
        }}
      >
        <ArrowDown size={13} />
        Latest
        {waits && (
          <>
            <span aria-hidden className="text-fg-faint">
              ·
            </span>
            <span className="min-w-0 truncate font-normal text-fg-muted">{waits}</span>
          </>
        )}
      </button>
    </div>
  );
}
