// The virtualized transcript. Rows are measured as they render; the view
// stays pinned to the bottom while the reply streams unless the reader has
// scrolled up, and a "Latest" button brings them back.

import { measureElement, useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown, ChevronDown, ChevronUp, RotateCw, Search, X } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { PermissionRequest, PlanEntry } from "../acp/types";
import { checkpointsByTurn } from "../acp/v2";
import { observeSize } from "../lib/resize";
import {
  foldEnds,
  foldTurns,
  groupTools,
  newestTurnEnd,
  type ToolGroup,
  type TurnChangeLine,
  type TurnFold,
  turnChangeLine,
  turnFoldKey,
} from "../state/fold";
import type { Link } from "../state/orchestration";
import { type ChildRequest, useChildRequests } from "../state/pending";
import { queueView } from "../state/queue";
import { RESUME_TEXT, shortcutFor, steerConditions, useApp } from "../state/store";
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
import { PlanCard } from "./PlanCard";
import { cx } from "./primitives";
import { QueuedMessageRow, ResumeQueueRow } from "./QueuedMessages";
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
  WorkingRow,
} from "./Rows";
import { turnFooterHidden } from "./replyActions";
import { plainPreview, TimelineRail, type Turn, turnTitle } from "./TimelineRail";
import { TurnChanges } from "./TurnChanges";

export type Row = {
  key: string;
  item?: TranscriptItem;
  working?: true;
  /** Two or more subagents in a row, shown as one. */
  agents?: { items: Extract<TranscriptItem, { kind: "tool" }>[]; open: boolean };
  /** The threads a run of `start_session` calls started, shown as one row or card. */
  threads?: { links: Link[]; open: boolean };
  /** The running turn's foot: what it does now, when no row of its own shows it. */
  activity?: string;
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
  /** The plan a paused thread keeps to, at the foot. */
  plan?: { threadId: string; entries: PlanEntry[] };
  /** A message waiting for the thread, at the end of it. */
  queued?: {
    item: QueuedPrompt;
    index: number;
    count: number;
    held: boolean;
    steerable: boolean;
    /** The same thread's `acceptsImages` and built-in flag: the rest of the steer rule. */
    images: boolean;
    builtin: boolean;
    from?: string;
  };
  /** The queue's wake turn, as the one quiet line it waits under (its first line, and the whole text). */
  queueWake?: { line: string; text: string };
  /** Resume, under a queue a stopped or failed turn held. */
  resume?: { threadId: string; count: number };
};

/** The turns the reader opened, by thread: kept while the app runs, so going back keeps them. */
const openFolds = new Map<string, Set<string>>();
const NO_STEPS: Extract<TranscriptItem, { kind: "tool" }>[] = [];
const NO_FOLDS: ReadonlySet<string> = new Set();
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

/**
 * The height a row starts at, before it is measured. `hidden` rows (a turn footer that renders
 * nothing) take no room: nothing mounts to measure them, so an estimate above zero would push every
 * row below it down and leave a blank strip at the end of the scroll.
 */
export function estimatedRowHeight(row: Row | undefined, hidden = false): number {
  if (hidden) return 0;
  if (!row) return 24;
  if (row.approval) return 96;
  if (row.childRequest) return row.childRequest.request.question ? 240 : 52;
  if (row.plan) return 260;
  if (row.fold || row.group) return 26;
  if (row.queued) return 96;
  if (row.resume) return 30;
  if (row.queueWake) return 26;
  const item = row.item;
  if (!item) return 24;
  if (item.kind === "tool" || item.kind === "thought" || item.kind === "turnEnd") return 26;
  return Math.min(600, 40 + ("text" in item ? item.text.length / 4 : 0));
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
const LANE_SHIFT = { translate: "calc(-1 * var(--rc-lane-inset-end, 0px))" } as const;
const LANE_MOTION = "transition-[translate] duration-200 ease-snappy";

/** A turn's change line, wherever it draws: the completion card, else the checkpoint chip. */
function ChangeLine({ threadId, line }: { threadId: string; line: TurnChangeLine }) {
  return line.kind === "completion" ? (
    <CompletionCard completion={line.completion} />
  ) : (
    <TurnChanges threadId={threadId} checkpoint={line.checkpoint} />
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
  const started = useMemo(
    () =>
      startedLinks(
        visible,
        links.filter((l) => l.orchestrator === thread.id),
      ),
    [visible, links, thread.id],
  );
  const rows = useMemo<Row[]>(() => {
    // Finished turns fold, then runs of finished tool calls inside what shows fold into one row each.
    const shown = searching ? ALL_OPEN : open;
    const grouped: Row[] = groupTools(foldTurns(visible, thread.running, shown), shown);
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
    if (thread.running) {
      // `Working for 12s` heads the turn, under the message that started it.
      let at = out.length;
      for (let i = out.length - 1; i >= 0; i--) {
        if (out[i]!.item?.kind === "user") {
          at = i + 1;
          break;
        }
      }
      out.splice(at, 0, { key: "working", working: true });
      // The foot says what it does now unless the last row already does (a call
      // or thought still running, a reply streaming).
      const last = out.at(-1)?.item;
      const shows =
        (last?.kind === "tool" && (last.status === "in_progress" || last.status === "pending")) ||
        (last?.kind === "thought" && !last.done) ||
        last?.kind === "assistant";
      // With an approval waiting, the turn is not working: the row that asks is the foot.
      if (!shows && !waiting) out.push({ key: "activity", activity: workingLabel(visible) });
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
    // The plan a paused thread still has work on, at the foot: what is left, and Implement. A
    // finished plan, or one a running turn is working through, is not repeated here.
    if (!thread.running && thread.plan.some((e) => e.status !== "completed"))
      out.push({ key: "plan", plan: { threadId: thread.id, entries: thread.plan } });
    // What waits for the thread comes after all of it: its queued messages, the one quiet line a
    // wake turn waits under, and Resume when a stopped or failed turn held them. They scroll with
    // the transcript, and the pinned view follows them like any other row.
    const queue = queueView(thread);
    const waking = thread.queue.find((q) => q.wake);
    if (queue.wake && waking)
      out.push({ key: "queue-wake", queueWake: { line: queue.wake, text: waking.text } });
    const steer = steerConditions(thread);
    queue.messages.forEach((q, i) => {
      out.push({
        key: `queued-${q.id}`,
        queued: {
          item: q,
          index: i,
          count: queue.messages.length,
          held: queue.held,
          steerable: steer.live,
          images: steer.images,
          builtin: steer.builtin,
          from: q.link ? links.find((l) => l.id === q.link)?.orchestratorTitle : undefined,
        },
      });
    });
    if (queue.resumable)
      out.push({ key: "queue-resume", resume: { threadId: thread.id, count: queue.messages.length } });
    return out;
  }, [visible, subagentSteps, started, links, thread, open, searching, wait, childRequests]);
  // Footers of folded turns: their time is in the summary row, not repeated.
  const foldedEnds = useMemo(() => {
    const set = new Set<string>();
    for (const row of rows) if (row.fold) set.add(row.fold.key.slice("fold-".length));
    return set;
  }, [rows]);

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

  // The reply each turn footer takes over: the last reply before it, whose own row then
  // stays clean, so a finished turn shows one action row under its last reply, not two.
  const turnReply = useMemo(() => {
    const byEnd: Record<string, { id: string; text: string }> = {};
    const terminal = new Set<string>();
    let last: { id: string; text: string } | undefined;
    for (const item of thread.items) {
      if (item.kind === "assistant") last = { id: item.id, text: item.text };
      else if (item.kind === "user") last = undefined;
      else if (item.kind === "turnEnd" && last) {
        byEnd[item.id] = last;
        terminal.add(last.id);
      }
    }
    return { byEnd, terminal };
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
    if (!checkpoints?.length) return {};
    const turns: { id: string; prompt: string; at?: number }[] = [];
    let prompt = "";
    for (const item of thread.items) {
      if (item.kind === "user") prompt = item.text;
      else if (item.kind === "turnEnd") turns.push({ id: item.id, prompt, at: item.at });
    }
    return checkpointsByTurn(turns, checkpoints);
  }, [thread.items, checkpoints]);

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

  // The newest finished turn keeps its change line in view under its footer; an older turn draws
  // it as the last row inside its own fold, so it appears only when that turn is opened.
  const foldChanges = useMemo(() => {
    const map = new Map<number, ReactNode>();
    const ends = foldEnds(rows);
    for (const item of thread.items) {
      if (item.kind !== "turnEnd" || item.id === lastEnd) continue;
      const at = ends[turnFoldKey(item.id)];
      const line = changeLines[item.id];
      if (at === undefined || !line) continue;
      map.set(at, <ChangeLine threadId={thread.id} line={line} />);
    }
    return map;
  }, [rows, thread.items, changeLines, lastEnd, thread.id]);

  // The newest turn's line as the footer's props: the completion card, else the checkpoint chip.
  const footerChanges = useMemo(() => {
    const map: Record<string, { completion?: TurnCompletion; changes?: ReactNode }> = {};
    const line = lastEnd ? changeLines[lastEnd] : undefined;
    if (line?.kind === "completion") map[lastEnd!] = { completion: line.completion };
    else if (line?.kind === "checkpoint")
      map[lastEnd!] = { changes: <ChangeLine threadId={thread.id} line={line} /> };
    return map;
  }, [changeLines, lastEnd, thread.id]);

  // Turn footers that render nothing at all. Nothing measures a row that never mounts, so the
  // virtualizer has to estimate no height for it, or every row below drifts by a footer.
  const footerless = useMemo(() => {
    const set = new Set<string>();
    for (const row of rows) {
      const item = row.item;
      if (item?.kind !== "turnEnd") continue;
      // Only the newest turn keeps its change line in the footer; an older one folds it.
      const line = footerChanges[item.id];
      const hidden = turnFooterHidden({
        folded: foldedEnds.has(item.id),
        reply: turnReply.byEnd[item.id]?.text,
        completion: !!line?.completion,
        changes: !!line?.changes,
        outcome: item.outcome,
      });
      if (hidden) set.add(item.id);
    }
    return set;
  }, [rows, foldedEnds, turnReply, footerChanges]);

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
      return estimatedRowHeight(row, false);
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
    paddingStart: 20 + topInset,
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

  const toBottom = useCallback(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, []);

  // Opening a thread starts at its end.
  // biome-ignore lint/correctness/useExhaustiveDependencies: runs when the thread changes
  useLayoutEffect(() => {
    pinned.current = true;
    setShowLatest(false);
    virtualizer.scrollToIndex(Math.max(0, rows.length - 1), { align: "end" });
    requestAnimationFrame(toBottom);
  }, [thread.id]);

  // Whatever grows (a streamed chunk, a measured row, the composer) keeps a pinned view at the bottom.
  useEffect(() => {
    const inner = innerRef.current;
    if (!inner) return;
    const observer = new ResizeObserver(() => {
      if (pinned.current) toBottom();
    });
    observer.observe(inner);
    return () => observer.disconnect();
  }, [toBottom]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: new rows and new text re-pin
  useLayoutEffect(() => {
    if (pinned.current) toBottom();
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
        row.item?.kind === "user" ? [{ row: i, title: turnTitle(row.item.text) }] : [],
      ),
    };
  }
  const turns = turnsKey.current.turns;
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
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
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
        className="h-full overflow-y-auto overscroll-contain [overflow-anchor:none]"
        role="log"
        aria-label="Transcript"
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
                (prev?.item?.kind === "user" && !!prev.item.wake);
              // A wake divider is the turn's own first line: the work under it sits on it, and the
              // next turn starts a full gap below, the same one a message gets.
              const dividerGap = afterDivider ? 10 : 16;
              if (row.fold) {
                return (
                  <div key={v.key} data-index={v.index} ref={measureRef} className="w-full">
                    <div
                      className={cx(
                        "mx-auto w-full max-w-[var(--rc-chat-content-max-width)] px-3 sm:px-5",
                        LANE_MOTION,
                      )}
                      style={{ paddingTop: v.index === 0 ? 0 : dividerGap, ...LANE_SHIFT }}
                    >
                      <TurnFoldRow fold={row.fold} onToggle={toggleFold} rule={!afterDivider} />
                    </div>
                  </div>
                );
              }
              // 16 between messages, 8 around work, steps of one run tight.
              const workLike = (r?: Row) =>
                !!r?.group ||
                !!r?.agents ||
                !!r?.threads ||
                !!r?.activity ||
                !!r?.approval ||
                r?.item?.kind === "tool" ||
                r?.item?.kind === "thought";
              const gap =
                v.index === 0
                  ? 0
                  : workLike(row) && workLike(prev)
                    ? 2
                    : prev?.working || prev?.fold || workLike(row) || workLike(prev)
                      ? 10
                      : 16;
              // A wake divider is the turn's own first line: the work under it sits on it, and the
              // next turn starts a full gap below, the same one a message gets.
              const turnGap = afterDivider ? 10 : gap;
              // A turn whose footer has nothing to add leaves no row, and no gap for one.
              if (item?.kind === "turnEnd" && footerless.has(item.id)) return null;
              return (
                <div key={v.key} data-index={v.index} ref={measureRef} className="w-full">
                  <div
                    className={cx(
                      "mx-auto w-full max-w-[var(--rc-chat-content-max-width)] px-3 sm:px-5",
                      LANE_MOTION,
                      row.inFold ? "rc-expand" : fresh(row.key) && "rc-row-in",
                    )}
                    style={{ paddingTop: turnGap, ...LANE_SHIFT }}
                  >
                    {row.approval ? (
                      <ApprovalRow request={row.approval} pending={approvals} number={row.approvalNumber} />
                    ) : row.childRequest ? (
                      <ChildRequestCard
                        request={row.childRequest.request}
                        from={row.childRequest.title}
                        pending={row.childRequest.pending}
                        number={row.childRequest.number}
                        keys={row.childKeys}
                      />
                    ) : row.plan ? (
                      <PlanCard threadId={row.plan.threadId} entries={row.plan.entries} />
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
                    ) : row.waiting ? (
                      <WaitingRow wait={row.waiting} />
                    ) : row.queued ? (
                      <QueuedMessageRow threadId={thread.id} {...row.queued} />
                    ) : row.queueWake ? (
                      // The same quiet line its turn leaves once it runs: the wake's first line, the
                      // whole message in the title. Never a bubble that reads as the user's own.
                      <div title={row.queueWake.text}>
                        <NoticeRow
                          flat
                          item={{ kind: "notice", id: "queue-wake", text: row.queueWake.line, tone: "info" }}
                        />
                      </div>
                    ) : row.resume ? (
                      <ResumeQueueRow threadId={row.resume.threadId} count={row.resume.count} />
                    ) : row.working ? (
                      <WorkingRow startedAt={thread.turnStartedAt} now={now} />
                    ) : row.activity ? (
                      <LiveActivityRow label={row.activity} />
                    ) : item?.kind === "user" && item.text === RESUME_TEXT ? (
                      <div className="flex items-center gap-2 text-caption text-fg-faint">
                        <span className="h-px flex-1 bg-border" />
                        <RotateCw size={11} />
                        Continued after a restart
                        <span className="h-px flex-1 bg-border" />
                      </div>
                    ) : item?.kind === "user" ? (
                      <UserRow
                        item={item}
                        threadId={thread.running || peek ? undefined : thread.id}
                        rewind={rewind}
                        // Consecutive messages from the same side soften the corners they share.
                        after={rows[v.index - 1]?.item?.kind === "user"}
                        before={rows[v.index + 1]?.item?.kind === "user"}
                      />
                    ) : item?.kind === "assistant" ? (
                      <AssistantRow
                        item={item}
                        threadId={thread.id}
                        streaming={thread.running && v.index === lastIndex}
                        inTurnFoot={turnReply.terminal.has(item.id)}
                      />
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
                      <TurnEndRow
                        item={item}
                        folded={foldedEnds.has(item.id)}
                        timeInFold={timeInFold.has(item.id)}
                        lastTurn={item.id === lastEnd}
                        threadId={thread.id}
                        reply={turnReply.byEnd[item.id]?.text}
                        completion={footerChanges[item.id]?.completion}
                        changes={footerChanges[item.id]?.changes}
                      />
                    ) : null}
                  </div>
                  {/* An older turn's change line rides at the end of its own fold, so it shows
                      only once that turn's `Worked for …` is opened. */}
                  {foldChanges.has(v.index) && (
                    <div
                      className={cx(
                        "mx-auto w-full max-w-[var(--rc-chat-content-max-width)] px-3 sm:px-5",
                        LANE_MOTION,
                        "rc-expand",
                      )}
                      style={LANE_SHIFT}
                    >
                      {foldChanges.get(v.index)}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
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
      />
      <button
        type="button"
        onClick={() => {
          pinned.current = true;
          setShowLatest(false);
          scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight, behavior: "smooth" });
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
          left: "calc(50% - var(--rc-lane-inset-end, 0px))",
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
