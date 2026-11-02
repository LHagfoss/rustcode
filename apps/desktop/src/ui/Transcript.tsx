// The virtualized transcript. Rows are measured as they render; the view
// stays pinned to the bottom while the reply streams unless the reader has
// scrolled up, and a "Latest" button brings them back.

import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown, ChevronDown, ChevronUp, Search, X } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { checkpointsByTurn } from "../acp/v2";
import { useApp } from "../state/store";
import { type ThreadState, type TranscriptItem, type TurnCompletion, turnCompletion } from "../state/thread";
import { AgentSpawnRow } from "./AgentViews";
import { cx } from "./primitives";
import { AssistantRow, NoticeRow, ThoughtRow, ToolRow, TurnEndRow, UserRow, WorkingRow } from "./Rows";
import { TurnChanges } from "./TurnChanges";

type Row = { key: string; item?: TranscriptItem; working?: true };

function useNow(active: boolean) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!active) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [active]);
  return now;
}

function workingLabel(thread: ThreadState): string {
  for (let i = thread.items.length - 1; i >= 0; i--) {
    const item = thread.items[i]!;
    if (item.kind === "user") break;
    if (item.kind === "tool" && (item.status === "in_progress" || item.status === "pending")) {
      return item.toolKind === "execute" || /^(bash|shell)/i.test(item.title)
        ? `Running ${item.summary ?? item.title}`
        : (item.summary ?? item.title);
    }
  }
  return "Working";
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

export function Transcript({ thread, bottomInset }: { thread: ThreadState; bottomInset: number }) {
  const [find, setFind] = useState<{ query: string; index: number } | null>(null);
  const findField = useRef<HTMLInputElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const innerRef = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  const [showLatest, setShowLatest] = useState(false);
  const now = useNow(thread.running);

  const rows = useMemo<Row[]>(() => {
    const out: Row[] = thread.items.map((item) => ({ key: item.id, item }));
    if (thread.running) out.push({ key: "working", working: true });
    return out;
  }, [thread.items, thread.running]);

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
      if (item.kind === "turnEnd") map[item.id] = turnCompletion(thread.items, i);
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
      const item = rows[i]?.item;
      if (!item) return 24;
      if (item.kind === "tool" || item.kind === "thought" || item.kind === "turnEnd") return 26;
      return Math.min(600, 40 + ("text" in item ? item.text.length / 4 : 0));
    },
    getItemKey: (i) => rows[i]?.key ?? i,
    overscan: 8,
    paddingStart: 20,
    paddingEnd: bottomInset,
  });

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

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
    pinned.current = atBottom;
    setShowLatest(!atBottom);
  };

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

  const lastIndex = thread.items.length - 1;
  const items = virtualizer.getVirtualItems();

  return (
    <div className="relative min-h-0 flex-1">
      {find && (
        <div className="surface absolute top-2 right-7 z-20 flex h-[32px] items-center gap-1 rounded-full border border-hairline pr-1 pl-3 rc-in">
          <Search size={12} className="text-fg-2" />
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
            className="w-[180px] bg-transparent text-callout outline-none placeholder:text-fg-3"
          />
          <span className="min-w-[52px] text-right text-caption text-fg-2 tabular" aria-live="polite">
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
            className="flex size-6 items-center justify-center rounded-full text-fg-2 hover:bg-fill-2"
          >
            <ChevronUp size={13} />
          </button>
          <button
            type="button"
            aria-label="Next match"
            onClick={() => step(1)}
            className="flex size-6 items-center justify-center rounded-full text-fg-2 hover:bg-fill-2"
          >
            <ChevronDown size={13} />
          </button>
          <button
            type="button"
            aria-label="Close find"
            onClick={() => setFind(null)}
            className="flex size-6 items-center justify-center rounded-full text-fg-2 hover:bg-fill-2"
          >
            <X size={12} />
          </button>
        </div>
      )}
      <div
        ref={scrollRef}
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
            const prev = v.index > 0 ? rows[v.index - 1]?.item : undefined;
            // Rows are 18 apart; consecutive tool calls sit tight.
            const gap = v.index === 0 ? 0 : item?.kind === "tool" && prev?.kind === "tool" ? 2 : 18;
            return (
              <div
                key={v.key}
                data-index={v.index}
                ref={virtualizer.measureElement}
                className="absolute top-0 left-0 w-full"
                style={{ transform: `translateY(${v.start}px)` }}
              >
                <div className="mx-auto w-full max-w-[1036px] px-7" style={{ paddingTop: gap }}>
                  {row.working ? (
                    <WorkingRow label={workingLabel(thread)} startedAt={thread.turnStartedAt} now={now} />
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
          "surface absolute right-7 flex h-[30px] items-center gap-1.5 rounded-full px-3 font-medium text-callout transition-[opacity,transform] duration-200 ease-snappy",
          showLatest ? "scale-100 opacity-100" : "pointer-events-none scale-90 opacity-0",
        )}
        style={{ bottom: bottomInset - 40 }}
      >
        <ArrowDown size={13} />
        Latest
      </button>
    </div>
  );
}
