// Assistant replies split into Markdown and the blocks the app draws itself:
// ```rustcode-ui v1``` (Smart UI, docs/smart-ui.md) and ```rustcode-viz```
// (docs/visualizations.md). Only an info string of exactly `rustcode-ui v1`
// runs; any other fence stays Markdown.

export const MAX_SOURCE_BYTES = 64 * 1024;
export const MAX_ELEMENTS = 5000;
export const MIN_HEIGHT = 48;
export const MAX_HEIGHT = 720;
export const MAX_LIVE = 3;

export type Segment =
  | { kind: "markdown"; text: string }
  | { kind: "ui"; source: string; complete: boolean; index: number }
  | { kind: "viz"; source: string; complete: boolean; index: number };

/** Split a reply at its `rustcode-ui v1` and `rustcode-viz` fences. */
export function splitReply(text: string): Segment[] {
  const lines = text.split("\n");
  const out: Segment[] = [];
  let markdown: string[] = [];
  let block = 0;
  const flush = () => {
    if (markdown.join("").trim()) out.push({ kind: "markdown", text: markdown.join("\n") });
    markdown = [];
  };
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]!;
    const open = /^ {0,3}(`{3,}|~{3,})\s*(.*?)\s*$/.exec(line);
    if (!open) {
      markdown.push(line);
      continue;
    }
    const fence = open[1]!;
    const info = open[2]!;
    const kind = info === "rustcode-ui v1" ? "ui" : info === "rustcode-viz" ? "viz" : null;
    // Find this fence's end, so a fence of another kind is skipped whole.
    let end = -1;
    for (let j = i + 1; j < lines.length; j++) {
      const close = /^ {0,3}(`{3,}|~{3,})\s*$/.exec(lines[j]!);
      if (close && close[1]![0] === fence[0] && close[1]!.length >= fence.length) {
        end = j;
        break;
      }
    }
    if (!kind) {
      const stop = end < 0 ? lines.length : end + 1;
      markdown.push(...lines.slice(i, stop));
      i = stop - 1;
      continue;
    }
    flush();
    const body = lines.slice(i + 1, end < 0 ? lines.length : end).join("\n");
    out.push({ kind, source: body, complete: end >= 0, index: block++ });
    i = end < 0 ? lines.length : end;
  }
  flush();
  return out;
}

/** Why a finished block can't run, or null. */
export function blockProblem(source: string): string | null {
  if (!source.trim()) return "The view is empty.";
  if (new TextEncoder().encode(source).length > MAX_SOURCE_BYTES) return "The view is over 64 KB.";
  return null;
}

/** The page's `<title>`, for the hover label and accessibility; never drawn as chrome. */
export function blockTitle(source: string): string {
  const m = /<title[^>]*>([^<]{1,120})<\/title>/i.exec(source);
  return m?.[1]?.trim() || "Interactive view";
}

export const clampHeight = (h: number) => Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, Math.ceil(h)));

/** What a page may ask through `window.rustcode.send`: text, at most 2,000 characters, not blank. */
export function viewMessage(text: unknown): string | null {
  if (typeof text !== "string") return null;
  const t = text.trim();
  if (!t || t.length > 2000) return null;
  return t;
}
