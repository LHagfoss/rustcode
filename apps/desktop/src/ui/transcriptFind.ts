import { statusReply } from "../state/delegateStatus";
import type { TranscriptItem } from "../state/thread";

/** Whether the reader has text marked: following the stream would scroll it away. */
export function selecting(): boolean {
  const s = window.getSelection();
  return !!s && s.rangeCount > 0 && !s.isCollapsed;
}

/** The words a row offers find-in-thread. */
export function searchable(item: TranscriptItem): string {
  switch (item.kind) {
    case "user":
    case "assistant":
    case "thought":
    case "notice":
      return item.text;
    case "status":
      return statusReply(item.rows);
    case "question":
      return `${item.header ?? ""} ${item.text} ${item.answer}`;
    case "tool":
      return `${item.title} ${item.summary ?? ""} ${item.detail ?? ""} ${item.output ?? ""}`;
    default:
      return "";
  }
}

/** Marks every visible occurrence of `query` with the CSS highlight `rc-find` (current one: `rc-find-current`). */
export function paintMatches(root: HTMLElement | null, query: string, current: HTMLElement | null) {
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
