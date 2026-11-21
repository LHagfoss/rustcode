// Smart UI shorthand (docs/smart-ui.md › Shorthand): a line that starts with `::` opens a kit
// block the app writes itself, so an agent spends a few tokens per row instead of the
// kit's full markup, and every view of one kind comes out alike. The `- ` lines under
// it are the block's items; a blank line, the next `::` line or a line that starts
// with `<` ends it. Everything else in a block is HTML and passes through unchanged,
// as does a `<script>` or `<style>` body.
//
//   :: title Reconnect timing            <title>, the view's label
//   :: h Why it takes a minute           h3
//   :: p                                 a paragraph; the lines below are its text
//   :: bars Why it takes a minute        .rc-bars; `- label | value | shown | tip`
//   :: stats                             .rc-stats; `- value | label | note`
//   :: steps                             .rc-steps; `- [x] done`, `[>]` active, `[!]` failed, `[ ]` to do
//   :: list                              .rc-list; `- Title | detail | value`
//   :: cards                             .rc-grid of .rc-card; `- Title | body | badge`
//   :: table                             table.rc-table; `a | b` rows, the first is the heading
//   :: callout info                      .rc-callout tinted ok|warn|bad|info; the lines below are its text
//   :: sources                           folded sources; `- Title | url`
//   :: actions                           a row of .rc-chip follow-ups; `- Label | message`
//
// A title after a block's name (`:: bars Why …`) draws an h3 over it. An item may end with
// `{ok}`, `{warn}`, `{bad}` or `{info}` to tint it; on a titled item the tint may also end its
// title cell (`Fast path {ok} | …`), where agents put it as often as at the end. In text, `code` and **bold** work and
// any HTML is kept; cells split at `|` outside backticks.

const TINTS = new Set(["ok", "warn", "bad", "info"]);
const BLOCKS = new Set([
  "title",
  "h",
  "p",
  "bars",
  "stats",
  "steps",
  "list",
  "cards",
  "table",
  "callout",
  "sources",
  "actions",
]);
const DIRECTIVE = /^\s*::\s*([a-z]+)\b\s*(.*)$/;
export const isDirective = (line: string) => BLOCKS.has(DIRECTIVE.exec(line)?.[1] ?? "");
const ITEM = /^\s*[-*]\s+(.*)$/;
const TINT_SUFFIX = /\s*\{(ok|warn|bad|info)\}\s*$/;
const TINT_PREFIX = /^\s*\{(ok|warn|bad|info)\}(?:\s+|$)/;
/** Sortable past this many rows, as the kit's guidance says. */
const SORT_ROWS = 8;
/** A table's divider row, `|---|:--:|`. */
const DIVIDER = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;

const escapeHtml = (text: string) => text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const escapeAttr = (text: string) => escapeHtml(text).replace(/"/g, "&quot;");

/** `code` and **bold** in a line of text; HTML in it is kept as written. */
export function inline(text: string): string {
  return text
    .split(/(`[^`]+`)/)
    .map((part, i) =>
      i % 2 === 1
        ? `<code>${escapeHtml(part.slice(1, -1))}</code>`
        : part.replace(/\*\*([^*]+)\*\*/g, "<b>$1</b>"),
    )
    .join("");
}

/** A row's cells: split at `|` outside backticks and HTML tags/`<code>` (`\|` keeps one), trimmed; at most `max`, the last keeping the rest. */
export function cells(line: string, max = Infinity): string[] {
  const out: string[] = [];
  let cell = "";
  let backtick = false;
  let inTag = false;
  let quote = "";
  let tagBuf = "";
  let codeDepth = 0;
  for (let i = 0; i < line.length; i++) {
    const char = line[i]!;
    if (!inTag && char === "\\" && line[i + 1] === "|") {
      cell += "|";
      i++;
      continue;
    }
    if (!inTag && char === "`") {
      backtick = !backtick;
      cell += char;
      continue;
    }
    if (!backtick) {
      if (inTag) {
        tagBuf += char;
        cell += char;
        if (quote) {
          if (char === quote) quote = "";
        } else if (char === '"' || char === "'") {
          quote = char;
        } else if (char === ">") {
          inTag = false;
          if (/^<code(?:\s|>)/i.test(tagBuf) && !/\/>\s*$/.test(tagBuf)) codeDepth++;
          else if (/^<\/code\s*>/i.test(tagBuf)) codeDepth = Math.max(0, codeDepth - 1);
          tagBuf = "";
        }
        continue;
      }
      if (char === "<" && /[a-zA-Z/!?]/.test(line[i + 1] ?? "")) {
        inTag = true;
        tagBuf = "<";
        cell += char;
        continue;
      }
    }
    if (char === "|" && !backtick && !inTag && codeDepth === 0 && out.length < max - 1) {
      out.push(cell.trim());
      cell = "";
    } else cell += char;
  }
  out.push(cell.trim());
  return out;
}

/** A leading or trailing `{ok|warn|bad|info}` tint name, and the text without it. */
function extractTint(text: string): [string, string] {
  const suffix = TINT_SUFFIX.exec(text);
  if (suffix) return [text.slice(0, suffix.index).trim(), suffix[1]!];
  const prefix = TINT_PREFIX.exec(text);
  if (prefix) return [text.slice(prefix[0].length).trim(), prefix[1]!];
  return [text, ""];
}

/** The tint an item carries, as a class name (` rc-ok`), and the item without it. */
function tinted(text: string): [string, string] {
  const [body, name] = extractTint(text);
  return name ? [body, ` rc-${name}`] : [text, ""];
}

/** An item's cells with a tint taken from the item or from the start/end of any cell. */
function tintedCells(item: string, max: number): [string[], string] {
  const [body, tint] = tinted(item);
  let found = tint;
  const parts = cells(body, max).map((cell) => {
    const [text, own] = tinted(cell);
    if (own && !found) found = own;
    return own ? text : cell;
  });
  return [parts, found];
}

/** The number a bar's value cell starts with: "75", "1,234 ms", "12.5%". */
function barValue(cell: string): number {
  const match = /-?\d[\d,]*(?:\.\d+)?/.exec(cell);
  return match ? Math.abs(Number(match[0].replace(/,/g, ""))) : 0;
}

type Block = { name: string; arg: string; items: string[]; text: string[] };

function heading(arg: string): string {
  return arg ? `<h3>${inline(arg)}</h3>` : "";
}

function bars(block: Block): string {
  const rows = block.items.map((item) => {
    const [body, tint] = tinted(item);
    const [label = "", value = "", shown, tip] = cells(body, 4);
    return { label, value, shown: shown || value, tip, tint };
  });
  // Values that are all percentages are widths already; otherwise the largest bar is full.
  const percent = rows.length > 0 && rows.every((row) => /%\s*$/.test(row.value));
  const max = percent ? 100 : Math.max(0, ...rows.map((row) => barValue(row.value)));
  const out = rows.map((row) => {
    const width = max > 0 ? Math.min(100, Math.round((barValue(row.value) / max) * 1000) / 10) : 0;
    const tip = row.tip ? ` data-rc-tip="${escapeAttr(row.tip)}"` : "";
    return `<div class="rc-bar${row.tint}"${tip}><span>${inline(row.label)}</span><i style="width:${width}%"></i><b>${inline(row.shown)}</b></div>`;
  });
  return `${heading(block.arg)}<div class="rc-bars">${out.join("")}</div>`;
}

function stats(block: Block): string {
  const out = block.items.map((item) => {
    const [body, tint] = tinted(item);
    const [value = "", label = "", note] = cells(body, 3);
    const noteHtml = note ? `<span class="rc-stat-note">${inline(note)}</span>` : "";
    return `<div class="rc-stat${tint}"><span class="rc-stat-value">${inline(value)}</span><span class="rc-stat-label">${inline(label)}</span>${noteHtml}</div>`;
  });
  return `${heading(block.arg)}<div class="rc-stats">${out.join("")}</div>`;
}

const STEP_STATES: Record<string, string> = {
  x: " rc-done",
  ">": " rc-active",
  "!": " rc-failed",
  " ": " rc-todo",
};

function steps(block: Block): string {
  const out = block.items.map((item) => {
    const match = /^\[([xX>! ])\]\s*(.*)$/.exec(item);
    const state = match ? STEP_STATES[match[1]!.toLowerCase()]! : "";
    return `<li class="rc-step${state}">${inline(match ? match[2]! : item)}</li>`;
  });
  return `${heading(block.arg)}<ol class="rc-steps">${out.join("")}</ol>`;
}

/** A titled item: `Title | detail | aside`, or one line of text. */
function titled(item: string): { title: string; detail: string; aside: string; tint: string } {
  const [[title = "", detail, aside], tint] = tintedCells(item, 3);
  return detail === undefined
    ? { title: "", detail: inline(title), aside: "", tint }
    : { title: inline(title), detail: inline(detail), aside: aside ? inline(aside) : "", tint };
}

function list(block: Block): string {
  const out = block.items.map((item) => {
    const { title, detail, aside, tint } = titled(item);
    if (!title) return `<li class="rc-item rc-prose${tint}">${detail}</li>`;
    const value = aside ? `<span class="rc-item-value">${aside}</span>` : "";
    const body = detail ? `<p>${detail}</p>` : "";
    return `<li class="rc-item${tint}"><b>${title}</b>${value}${body}</li>`;
  });
  return `${heading(block.arg)}<ul class="rc-list">${out.join("")}</ul>`;
}

function cards(block: Block): string {
  const out = block.items.map((item) => {
    const { title, detail, aside, tint } = titled(item);
    const badge = aside ? `<span class="rc-badge">${aside}</span>` : "";
    const head = title ? `<b>${title}</b>${badge}` : badge;
    return `<div class="rc-card${tint}">${head}${detail ? `<p>${detail}</p>` : ""}</div>`;
  });
  return `${heading(block.arg)}<div class="rc-grid rc-cards">${out.join("")}</div>`;
}

/** Follow-up chips: `Label | message`, or one line that is both. */
function actions(block: Block): string {
  const out = block.items.map((item) => {
    const [label = "", message] = cells(item, 2);
    const send = (message || label).replace(/`|\*\*/g, "");
    return `<button class="rc-chip" data-rc-send="${escapeAttr(send)}">${inline(label)}</button>`;
  });
  return `${heading(block.arg)}<div class="rc-row rc-gap-2 rc-actions">${out.join("")}</div>`;
}

function tableCell(cell: string): string {
  const [text, tint] = extractTint(cell);
  if (tint) return `<span class="rc-badge rc-${tint}">${inline(text || tint)}</span>`;
  return inline(cell);
}

function table(block: Block): string {
  const raw = block.text
    .filter((line) => !DIVIDER.test(line))
    .map((line) =>
      line
        .trim()
        .replace(/^\|/, "")
        .replace(/(?<!\\)\|$/, ""),
    );
  const [headLine, ...bodyLines] = raw;
  const head = headLine === undefined ? [] : cells(headLine);
  const cols = head.length || Infinity;
  const body = bodyLines.map((line) => cells(line, cols));
  const sort = body.length > SORT_ROWS ? " data-rc-sort" : "";
  const th = head.map((cell) => `<th>${inline(cell)}</th>`).join("");
  const tr = body
    .map((row) => `<tr>${row.map((cell) => `<td>${tableCell(cell)}</td>`).join("")}</tr>`)
    .join("");
  return `${heading(block.arg)}<table class="rc-table"${sort}><thead><tr>${th}</tr></thead><tbody>${tr}</tbody></table>`;
}

function callout(block: Block): string {
  const [first = "", ...rest] = block.arg.split(/\s+/);
  const tint = TINTS.has(first) ? ` rc-${first}` : "";
  const title = tint ? rest.join(" ") : block.arg;
  const text = [...block.items, ...block.text].map(inline).join(" ");
  const lead = title ? `<b>${inline(title)}</b> ` : "";
  return `<div class="rc-callout${tint}">${lead}${text}</div>`;
}

function sources(block: Block): string {
  const out = block.items.map((item) => {
    const [title = "", url] = cells(item, 2);
    const href = url || title;
    return `<li><a href="${escapeAttr(href)}">${inline(title)}</a></li>`;
  });
  const summary = block.arg ? inline(block.arg) : "Sources";
  return `<details class="rc-details rc-sources"><summary>${summary}</summary><ol>${out.join("")}</ol></details>`;
}

function render(block: Block): string {
  switch (block.name) {
    case "title":
      return `<title>${escapeHtml(block.arg)}</title>`;
    case "h":
      return heading(block.arg);
    case "p":
      return `<p>${inline([block.arg, ...block.text, ...block.items].filter(Boolean).join(" "))}</p>`;
    case "bars":
      return bars(block);
    case "stats":
      return stats(block);
    case "steps":
      return steps(block);
    case "list":
      return list(block);
    case "cards":
      return cards(block);
    case "table":
      return table(block);
    case "callout":
      return callout(block);
    case "actions":
      return actions(block);
    default:
      return sources(block);
  }
}

/** Blocks that take their lines as text rather than as `- ` items. */
const TEXT_BLOCKS = new Set(["p", "table", "callout"]);
/** Blocks that are one line, the directive itself. */
const LINE_BLOCKS = new Set(["title", "h"]);

/** Whether a source uses the shorthand at all, so a plain HTML view is never touched. */
export const hasShorthand = (source: string) => /^\s*::\s*[a-z]+\b/m.test(source);

/**
 * A view's source with its shorthand blocks written out as kit HTML. A view without any
 * is returned as is. A block that is still arriving is drawn with the items it has, so
 * a streaming view grows a row at a time.
 */
export function expandShorthand(source: string): string {
  if (!hasShorthand(source)) return source;
  const out: string[] = [];
  let block: Block | null = null;
  let raw: string | null = null;
  const flush = () => {
    if (block) out.push(render(block));
    block = null;
  };
  for (const line of source.split("\n")) {
    if (raw) {
      out.push(line);
      if (new RegExp(`</${raw}\\s*>`, "i").test(line)) raw = null;
      continue;
    }
    const directive = DIRECTIVE.exec(line);
    if (directive && BLOCKS.has(directive[1]!)) {
      flush();
      const next: Block = { name: directive[1]!, arg: directive[2]!.trim(), items: [], text: [] };
      if (LINE_BLOCKS.has(next.name)) out.push(render(next));
      else block = next;
      continue;
    }
    const current = block as Block | null;
    if (current && line.trim() && !/^\s*</.test(line)) {
      const item = ITEM.exec(line);
      if (item && !TEXT_BLOCKS.has(current.name)) current.items.push(item[1]!);
      else if (!item && current.items.length > 0 && !TEXT_BLOCKS.has(current.name)) {
        // A wrapped item: the line continues the one above it.
        current.items[current.items.length - 1] += ` ${line.trim()}`;
      } else current.text.push(line.trim());
      continue;
    }
    flush();
    out.push(line);
    const opened = /<(script|style)\b[^>]*>/i.exec(line);
    if (opened && !new RegExp(`</${opened[1]}\\s*>`, "i").test(line.slice(opened.index)))
      raw = opened[1]!.toLowerCase();
  }
  flush();
  return out.join("\n");
}
