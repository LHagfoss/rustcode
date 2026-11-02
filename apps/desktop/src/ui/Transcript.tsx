// The virtualized transcript. Rows are measured as they render; the view
// stays pinned to the bottom while the reply streams unless the reader has
// scrolled up, and a "Latest" button brings them back.

import { measureElement, useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown, ChevronDown, ChevronUp, Search, X } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { checkpointsByTurn } from "../acp/v2";
import { observeSize } from "../lib/resize";
import { foldTurns, groupTools, type ToolGroup, type TurnFold } from "../state/fold";
import { useApp } from "../state/store";
import {
  type ThreadState,
  type TranscriptItem,
  type TurnCompletion,
  turnCompletion,
  workingLabel,
} from "../state/thread";
import { AgentSpawnRow } from "./AgentViews";
import { cx } from "./primitives";
import {
  AssistantRow,
  NoticeRow,
  ThoughtRow,
  ToolGroupRow,
  ToolRow,
  TurnEndRow,
  TurnFoldRow,
  UserRow,
  WorkingRow,
} from "./Rows";
import { TimelineRail, type Turn, turnTitle } from "./TimelineRail";
import { TurnChanges } from "./TurnChanges";

type Row = {
  key: string;
  item?: TranscriptItem;
  working?: true;
  fold?: TurnFold;
  group?: ToolGroup;
  inFold?: string;
};

/** The turns the reader opened, by thread: kept while the app runs, so going back keeps them. */
const openFolds = new Map<string, Set<string>>();
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

function searchable(item: TranscriptItem): string {
  switch (item.kind) {
    case "user":
    case "assistant":
    case "thought":
    case "notice":
      return item.text;
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

export function Transcript({
  thread,
  bottomInset,
  topInset = 0,
}: {
  thread: ThreadState;
  /** Room the floating composer stack takes at the foot. */
  bottomInset: number;
  /** Room the glass header takes at the top: the rows scroll under it. */
  topInset?: number;
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
  const rows = useMemo<Row[]>(() => {
    // Finished turns fold, then runs of finished tool calls inside what shows fold into one row each.
    const shown = searching ? ALL_OPEN : open;
    const out: Row[] = groupTools(foldTurns(thread.items, thread.running, shown), shown);
    if (thread.running) out.push({ key: "working", working: true });
    return out;
  }, [thread.items, thread.running, open, searching]);
  // Footers of folded turns: their time is in the summary row, not repeated.
  const foldedEnds = useMemo(() => {
    const set = new Set<string>();
    for (const row of rows) if (row.fold) set.add(row.fold.key.slice("fold-".length));
    return set;
  }, [rows]);

  // The reply each turn footer copies: the last reply before it.
  const replyBefore = useMemo(() => {
    const map: Record<string, string> = {};
    let last: string | undefined;
    for (const item of thread.items) {
      if (item.kind === "assistant") last = item.text;
      else if (item.kind === "user") last = undefined;
      else if (item.kind === "turnEnd" && last) map[item.id] = last;
    }
    return map;
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
  const rewind = useApp((s) => !!s.agent?.rewind) && (thread.harness?.capabilities.rewind ?? true);
  const checkpoints = useApp((s) => s.checkpoints[thread.id]);
  // Each finished turn's checkpoint, for its Changes chip.
  const turnCheckpoints = useMemo(() => {
    if (!checkpoints?.length) return {};
    const turns: { id: string; prompt: string }[] = [];
    let prompt = "";
    for (const item of thread.items) {
      if (item.kind === "user") prompt = item.text;
      else if (item.kind === "turnEnd") turns.push({ id: item.id, prompt });
    }
    return checkpointsByTurn(turns, checkpoints);
  }, [thread.items, checkpoints]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => {
      const known = rowSizes.get(rows[i]?.key ?? "");
      if (known !== undefined) return known;
      const item = rows[i]?.item;
      if (rows[i]?.fold || rows[i]?.group) return 26;
      if (!item) return 24;
      if (item.kind === "tool" || item.kind === "thought" || item.kind === "turnEnd") return 26;
      return Math.min(600, 40 + ("text" in item ? item.text.length / 4 : 0));
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
  const [readTurn, setReadTurn] = useState(0);
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
    let at = 0;
    for (let t = 0; t < turns.length; t++) {
      const start = starts[turns[t]!.row]?.start;
      if (start !== undefined && start + 16 <= reading) at = t;
      else if (start !== undefined) break;
    }
    setReadTurn(at);
  };

  const jumpToTurn = useCallback(
    (row: number) => {
      pinned.current = false;
      setShowLatest(true);
      virtualizer.scrollToIndex(row, { align: "start" });
    },
    [virtualizer],
  );

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

  // ⌘F opens find in this thread; Esc closes it.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f") {
        e.preventDefault();
        setFind((f) => f ?? { query: "", index: 0 });
        requestAnimationFrame(() => findField.current?.select());
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

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

  // The last item's row: the running turn never folds, so it is the last row before the working line.
  const lastIndex = rows.length - (thread.running ? 2 : 1);
  // The first render draws no rows: the virtualizer has no scroll element yet and would pick the
  // thread's first rows. `attachScroll` then puts the view at the end, and the second render
  // (still before the first paint) draws the last rows straight away.
  const items = ready ? virtualizer.getVirtualItems() : [];

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
          {items.map((v) => {
            const row = rows[v.index]!;
            const item = row.item;
            if (row.fold) {
              return (
                <div
                  key={v.key}
                  data-index={v.index}
                  ref={measureRef}
                  className="absolute top-0 left-0 w-full"
                  style={{ transform: `translateY(${v.start}px)` }}
                >
                  <div className="mx-auto w-full max-w-[1036px] px-7" style={{ paddingTop: 18 }}>
                    <TurnFoldRow fold={row.fold} onToggle={toggleFold} />
                  </div>
                </div>
              );
            }
            // Rows are 18 apart; consecutive tool calls (and their groups) sit tight.
            const toolLike = (r?: Row) => !!r?.group || r?.item?.kind === "tool";
            const gap = v.index === 0 ? 0 : toolLike(row) && toolLike(rows[v.index - 1]) ? 2 : 18;
            return (
              <div
                key={v.key}
                data-index={v.index}
                ref={measureRef}
                className="absolute top-0 left-0 w-full"
                style={{ transform: `translateY(${v.start}px)` }}
              >
                <div
                  className={cx("mx-auto w-full max-w-[1036px] px-7", row.inFold && "rc-expand")}
                  style={{ paddingTop: gap }}
                >
                  {row.group ? (
                    <ToolGroupRow group={row.group} onToggle={toggleFold} />
                  ) : row.working ? (
                    <WorkingRow
                      label={workingLabel(thread.items)}
                      startedAt={thread.turnStartedAt}
                      now={now}
                    />
                  ) : item?.kind === "user" ? (
                    <UserRow item={item} threadId={thread.running ? undefined : thread.id} rewind={rewind} />
                  ) : item?.kind === "assistant" ? (
                    <AssistantRow
                      item={item}
                      threadId={thread.id}
                      streaming={thread.running && v.index === lastIndex}
                    />
                  ) : item?.kind === "thought" ? (
                    <ThoughtRow item={item} live={thread.running && !item.done && v.index === lastIndex} />
                  ) : item?.kind === "tool" && item.title === "spawn_agent" ? (
                    <AgentSpawnRow item={item} threadId={thread.id} />
                  ) : item?.kind === "tool" ? (
                    <ToolRow item={item} cwd={thread.cwd} />
                  ) : item?.kind === "notice" ? (
                    <NoticeRow item={item} />
                  ) : item?.kind === "turnEnd" ? (
                    <TurnEndRow
                      item={item}
                      folded={foldedEnds.has(item.id)}
                      reply={replyBefore[item.id]}
                      completion={completions[item.id]}
                      changes={
                        turnCheckpoints[item.id] && (
                          <TurnChanges threadId={thread.id} checkpoint={turnCheckpoints[item.id]!} />
                        )
                      }
                    />
                  ) : null}
                </div>
              </div>
            );
          })}
        </div>
      </div>
      <TimelineRail
        turns={turns}
        current={showLatest ? Math.min(readTurn, turns.length - 1) : turns.length - 1}
        width={size.width}
        visible={size.height - bottomInset + 20 - topInset}
        offset={topInset}
        onJump={jumpToTurn}
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
          // Above the floating composer (its top is `bottomInset - 20` up) and over it in z.
          "rc-glass rc-glass-strong absolute left-1/2 z-30 flex h-[30px] -translate-x-1/2 items-center gap-1.5 rounded-full px-3 font-medium text-callout text-fg shadow-1 transition-[opacity,transform] duration-200 ease-snappy hover:bg-surface-3",
          showLatest ? "translate-y-0 opacity-100" : "pointer-events-none translate-y-1 opacity-0",
        )}
        style={{ bottom: bottomInset - 8 }}
      >
        <ArrowDown size={13} />
        Latest
      </button>
    </div>
  );
}
