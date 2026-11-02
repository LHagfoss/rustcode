// The virtualized transcript. Rows are measured as they render; the view
// stays pinned to the bottom while the reply streams unless the reader has
// scrolled up, and a "Latest" button brings them back.

import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { ThreadState, TranscriptItem } from "../state/thread";
import { cx } from "./primitives";
import { AssistantRow, NoticeRow, ThoughtRow, ToolRow, TurnEndRow, UserRow, WorkingRow } from "./Rows";

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

export function Transcript({ thread, bottomInset }: { thread: ThreadState; bottomInset: number }) {
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

  const lastIndex = thread.items.length - 1;
  const items = virtualizer.getVirtualItems();

  return (
    <div className="relative min-h-0 flex-1">
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
                    <UserRow item={item} />
                  ) : item?.kind === "assistant" ? (
                    <AssistantRow item={item} />
                  ) : item?.kind === "thought" ? (
                    <ThoughtRow item={item} live={thread.running && !item.done && v.index === lastIndex} />
                  ) : item?.kind === "tool" ? (
                    <ToolRow item={item} cwd={thread.cwd} />
                  ) : item?.kind === "notice" ? (
                    <NoticeRow item={item} />
                  ) : item?.kind === "turnEnd" ? (
                    <TurnEndRow item={item} reply={replyBefore[item.id]} />
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
