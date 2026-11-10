// Assistant replies split into Markdown and the blocks the app draws itself:
// ```rustcode-ui v1``` (Smart UI, docs/smart-ui.md) and ```rustcode-viz```
// (docs/visualizations.md). Only an info string of exactly `rustcode-ui v1`
// runs; any other fence stays Markdown.

export const MAX_SOURCE_BYTES = 64 * 1024;
export const MAX_ELEMENTS = 5000;
export const MIN_HEIGHT = 48;
export const MAX_LIVE = 6;

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

/** Elements that never hold content, so they never need closing. */
const VOID_TAGS = new Set([
  "area",
  "base",
  "br",
  "col",
  "embed",
  "hr",
  "img",
  "input",
  "link",
  "meta",
  "param",
  "source",
  "track",
  "wbr",
]);

/** Elements whose content is text until their own closing tag. */
const RAW_TAGS = new Set(["script", "style", "textarea", "title", "xmp", "noembed", "noframes"]);

const TAG_NAME = /^[a-zA-Z][^\s/>]*/;

/** Where a tag's `>` is, skipping quoted attribute values; -1 until it has arrived. */
function tagEnd(html: string, from: number): number {
  let quote = "";
  for (let i = from; i < html.length; i++) {
    const char = html[i];
    if (quote) {
      if (char === quote) quote = "";
    } else if (char === '"' || char === "'") quote = char;
    else if (char === ">") return i;
  }
  return -1;
}

/**
 * A fragment that has only partly arrived, with the tags it still has open
 * closed, so a streaming block can be drawn before its fence does
 * (docs/smart-ui.md). A tag that is still being written is dropped rather than
 * shown as text. Its `<script>` content is inert: the app injects this HTML,
 * and injected HTML never runs a script.
 */
export function closeOpenTags(html: string): string {
  const open: string[] = [];
  const finish = (upto: number) =>
    html.slice(0, upto) +
    open
      .reverse()
      .map((name) => `</${name}>`)
      .join("");
  let i = 0;
  while (i < html.length) {
    const lt = html.indexOf("<", i);
    if (lt < 0) break;
    if (html.startsWith("<!--", lt)) {
      const end = html.indexOf("-->", lt + 4);
      if (end < 0) return finish(lt);
      i = end + 3;
      continue;
    }
    if (html.startsWith("<!", lt) || html.startsWith("<?", lt)) {
      const end = tagEnd(html, lt + 2);
      if (end < 0) return finish(lt);
      i = end + 1;
      continue;
    }
    const closing = html[lt + 1] === "/";
    const nameAt = lt + (closing ? 2 : 1);
    const name = TAG_NAME.exec(html.slice(nameAt, nameAt + 48))?.[0]?.toLowerCase();
    if (!name) {
      // A `<` in the text ("a < b"), not a tag.
      i = lt + 1;
      continue;
    }
    const end = tagEnd(html, nameAt + name.length);
    if (end < 0) return finish(lt);
    const selfClosing = html[end - 1] === "/";
    i = end + 1;
    if (closing) {
      const at = open.lastIndexOf(name);
      if (at >= 0) open.length = at;
    } else if (VOID_TAGS.has(name) || selfClosing) {
    } else if (RAW_TAGS.has(name)) {
      // Raw text runs to its closing tag, or to the end of what has arrived.
      const close = new RegExp(`</${name}\\s*>`, "i").exec(html.slice(i));
      if (!close) {
        open.push(name);
        return finish(html.length);
      }
      i += close.index + close[0].length;
    } else {
      open.push(name);
    }
  }
  return finish(html.length);
}

const EMPTY_TAG = /<(style|script)\b[^>]*>\s*<\/\1\s*>/gi;

/** An empty `<style></style>` or `<script></script>` does nothing; agents emit them by habit. */
export const stripEmptyTags = (source: string) => source.replace(EMPTY_TAG, "");

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

/** A view takes its content's full height; only a title row's minimum keeps it readable. */
export const clampHeight = (h: number) => Math.max(MIN_HEIGHT, Math.ceil(h));

/** What a page may ask through `window.rustcode.send`: text, at most 2,000 characters, not blank. */
export function viewMessage(text: unknown): string | null {
  if (typeof text !== "string") return null;
  const t = text.trim();
  if (!t || t.length > 2000) return null;
  return t;
}
