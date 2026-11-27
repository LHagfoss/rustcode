// Smart UI shorthand, the view's component catalog (docs/smart-ui.md › Shorthand: the
// component catalog, which lists every block): a line that starts with `::` opens a kit
// block the app writes itself, and the `- ` lines under it are its items. A blank line,
// the next `::` line or a line that starts with `<` ends it. Everything else is HTML
// and passes through unchanged, as does a `<script>` or `<style>` body.

const TINTS = new Set(["ok", "warn", "bad", "info"]);
/** Every block the shorthand knows, the catalog `smart_ui_check` and the skill name. */
export const BLOCKS = new Set([
  "title",
  "head",
  "h",
  "p",
  "callout",
  "divider",
  "tab",
  "end",
  "details",
  "hero",
  "stats",
  "bars",
  "progress",
  "chart",
  "heatmap",
  "table",
  "compare",
  "list",
  "kv",
  "cards",
  "steps",
  "timeline",
  "findings",
  "files",
  "tags",
  "term",
  "code",
  "actions",
  "form",
  "sources",
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

/** `code`, **bold** and `[label](url)` links in a line of text; HTML in it is kept as written. */
export function inline(text: string): string {
  const code = (value: string) =>
    value.replace(/`([^`]+)`/g, (_, body: string) => `<code>${escapeHtml(body)}</code>`);
  const styled = (value: string) =>
    value.replace(/`([^`]+)`|\*\*((?:`[^`]+`|[^*`])+)\*\*/g, (match, literal, bold) =>
      literal === undefined ? `<b>${code(bold)}</b>` : code(match),
    );
  const link = (value: string) =>
    value.replace(
      /\[([^\]]+)\]\(((?:https?:\/\/|mailto:)(?:[^\s()]|\([^\s()]*\))+)\)/gi,
      (_, label: string, href: string) => `<a href="${escapeAttr(href)}">${styled(label)}</a>`,
    );
  return text.replace(
    /`([^`]+)`|\*\*((?:`[^`]+`|[^*`])+)\*\*|\[([^\]]+)\]\(((?:https?:\/\/|mailto:)(?:[^\s()]|\([^\s()]*\))+)\)/gi,
    (match, literal, bold, label, href) => {
      if (literal !== undefined) return code(match);
      if (bold !== undefined) return `<b>${link(code(bold))}</b>`;
      return `<a href="${escapeAttr(href)}">${styled(label)}</a>`;
    },
  );
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
    const [[value = "", label = "", note, spark], tint] = tintedCells(item, 4);
    const noteHtml = note ? `<span class="rc-stat-note">${inline(note)}</span>` : "";
    const sparkHtml = spark && numbers(spark).length > 1 ? sparkline(spark) : "";
    return `<div class="rc-stat${tint}"><span class="rc-stat-value">${inline(value)}</span><span class="rc-stat-label">${inline(label)}</span>${noteHtml}${sparkHtml}</div>`;
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

function stripRow(line: string): string {
  return line
    .trim()
    .replace(/^[-*]\s+/, "")
    .replace(/^\|/, "")
    .replace(/(?<!\\)\|$/, "");
}

/** A table-like block's heading, header cells and body rows. Accepts column headers on the `::` line too. */
function tableParts(block: Block): { title: string; head: string[]; body: string[][] } {
  const lines = [...block.text, ...block.items];
  const argLine = stripRow(block.arg);
  const argCells = argLine ? cells(argLine) : [];
  const raw = lines.filter((line) => !DIVIDER.test(line)).map(stripRow);
  const firstRow = raw[0] === undefined ? [] : cells(raw[0]);
  const hasHeaderDivider = lines.length >= 2 && !DIVIDER.test(lines[0]!) && DIVIDER.test(lines[1]!);
  const firstIsItem = lines[0] !== undefined && ITEM.test(lines[0]);
  const firstIsTinted = firstRow.some((cell) => extractTint(cell)[1] !== "");
  const argIsHeader =
    argCells.length > 1 &&
    !hasHeaderDivider &&
    (raw.length === 0 ||
      DIVIDER.test(lines[0] ?? "") ||
      firstIsItem ||
      firstIsTinted ||
      firstRow.length === argCells.length ||
      raw.some((line) => cells(line).length === argCells.length));
  if (argIsHeader) {
    const cols = argCells.length;
    return { title: "", head: argCells, body: raw.map((line) => cells(line, cols)) };
  }
  const [headLine, ...bodyLines] = raw;
  if (headLine === undefined) return { title: block.arg, head: [], body: [] };
  if (!hasHeaderDivider && firstIsTinted) {
    const cols = firstRow.length || Infinity;
    return { title: block.arg, head: [], body: raw.map((line) => cells(line, cols)) };
  }
  const head = cells(headLine);
  const cols = head.length || Infinity;
  return { title: block.arg, head, body: bodyLines.map((line) => cells(line, cols)) };
}

function table(block: Block): string {
  const { title, head, body } = tableParts(block);
  const sort = head.length > 0 && body.length > SORT_ROWS ? " data-rc-sort" : "";
  const thead = head.length
    ? `<thead><tr>${head.map((cell) => `<th>${inline(cell)}</th>`).join("")}</tr></thead>`
    : "";
  const tr = body
    .map((row) => `<tr>${row.map((cell) => `<td>${tableCell(cell)}</td>`).join("")}</tr>`)
    .join("");
  return `${heading(title)}<table class="rc-table"${sort}>${thead}<tbody>${tr}</tbody></table>`;
}

function callout(block: Block): string {
  const [argBody, argTint] = tinted(block.arg);
  const [first = "", ...rest] = argBody.split(/\s+/);
  const leadTint = TINTS.has(first) ? ` rc-${first}` : "";
  const title = leadTint ? rest.join(" ") : argBody;
  let found = leadTint || argTint;
  const text = [...block.items, ...block.text]
    .map((line) => {
      const [clean, own] = tinted(line);
      if (own && !found) found = own;
      return inline(own ? clean : line);
    })
    .join(" ");
  const lead = title ? `<b>${inline(title)}</b> ` : "";
  return `<div class="rc-callout${found}">${lead}${text}</div>`;
}

const LINK_URL = /^(?:https?:\/\/|mailto:)/i;
const SOURCES_HEADING = /^sources?(?:\s*[:·-]?\s*\(?\d+\)?)?$/i;
const MD_SOURCE =
  /^\[([^\]]+)\]\(\s*((?:https?:\/\/|mailto:)(?:[^\s()]|\([^\s()]*\))+)\s*\)(?:\s*(?:\||—|–|-)\s*(.*))?$/i;

function stripUrl(raw: string): string {
  const trimmed = raw.trim();
  return trimmed.startsWith("<") && trimmed.endsWith(">") ? trimmed.slice(1, -1).trim() : trimmed;
}

function sourceItem(item: string): string {
  const trimmed = item.trim();
  if (!trimmed) return "";
  if (/^<a\b[^>]*\bhref\s*=/i.test(trimmed)) return `<li>${inline(trimmed)}</li>`;
  const md = MD_SOURCE.exec(trimmed);
  if (md) {
    const note = md[3]?.trim() ? ` <span class="rc-muted">${inline(md[3].trim())}</span>` : "";
    return `<li><a href="${escapeAttr(md[2]!)}">${inline(md[1]!)}</a>${note}</li>`;
  }
  const [first = "", second, third] = cells(trimmed, 3);
  if (second !== undefined) {
    const url = stripUrl(second);
    if (LINK_URL.test(url)) {
      const note = third ? ` <span class="rc-muted">${inline(third)}</span>` : "";
      return `<li><a href="${escapeAttr(url)}">${inline(first || url)}</a>${note}</li>`;
    }
    const firstUrl = stripUrl(first);
    if (LINK_URL.test(firstUrl)) {
      const label = third ? `${second} — ${third}` : second;
      return `<li><a href="${escapeAttr(firstUrl)}">${inline(label)}</a></li>`;
    }
    const rest = third ? `${second} — ${third}` : second;
    return `<li>${inline(first)} <span class="rc-muted">${inline(rest)}</span></li>`;
  }
  const onlyUrl = stripUrl(first);
  if (LINK_URL.test(onlyUrl)) return `<li><a href="${escapeAttr(onlyUrl)}">${escapeHtml(onlyUrl)}</a></li>`;
  return `<li>${inline(first)}</li>`;
}

function sources(block: Block): string {
  const entries = [...block.text.map((line) => line.replace(/^\d+[.)]\s+/, "")), ...block.items];
  const out = entries.map(sourceItem).filter(Boolean);
  const [rawSummary = ""] = cells(block.arg, 2);
  const summary =
    rawSummary && !SOURCES_HEADING.test(rawSummary)
      ? inline(rawSummary)
      : /^source\b/i.test(rawSummary) && !/^sources\b/i.test(rawSummary)
        ? "Source"
        : "Sources";
  return `<details class="rc-details rc-sources"><summary>${summary}</summary><ol>${out.join("")}</ol></details>`;
}

/** The numbers in a cell: "1,4,2" and "1 4 2" give [1, 4, 2]; a thousands comma needs no space after it. */
function numbers(cell: string): number[] {
  return cell
    .split(/[\s,;]+/)
    .filter(Boolean)
    .map(Number)
    .filter((value) => Number.isFinite(value));
}

function sparkline(values: string): string {
  return `<span class="rc-spark" data-values="${escapeAttr(numbers(values).join(","))}"></span>`;
}

/** A measurement in a cell, signed: "1,234", "-3.5 ms", "12%"; null when there is none. */
function measure(cell: string): number | null {
  const match = /^[^\d-]*(-?\d[\d,]*(?:\.\d+)?)/.exec(cell.replace(/−/g, "-"));
  return match ? Number(match[1]!.replace(/,/g, "")) : null;
}

/** A `:: table`-style block's rows: dividers dropped, outer pipes trimmed, split into cells. */
function rows(block: Block): string[][] {
  const lines = [...block.text, ...block.items].filter((line) => !DIVIDER.test(line)).map(stripRow);
  const [headLine, ...bodyLines] = lines;
  if (headLine === undefined) return [];
  const head = cells(headLine);
  return [head, ...bodyLines.map((line) => cells(line, head.length || Infinity))];
}

function head(block: Block): string {
  const [title = "", caption] = cells(block.arg, 2);
  return `<div class="rc-head"><b>${inline(title)}</b>${caption ? `<span>${inline(caption)}</span>` : ""}</div>`;
}

function hero(block: Block): string {
  const out = block.items.map((item) => {
    const [[value = "", delta, lead], tint] = tintedCells(item, 3);
    const deltaHtml = delta ? `<span class="rc-hero-delta${tint}">${inline(delta)}</span>` : "";
    const leadHtml = lead ? `<p class="rc-lead">${inline(lead)}</p>` : "";
    return `<span class="rc-hero-value">${inline(value)}</span>${deltaHtml}${leadHtml}`;
  });
  const label = block.arg ? `<span class="rc-label">${inline(block.arg)}</span>` : "";
  return `<div class="rc-hero">${label}${out.join("")}</div>`;
}

/** A progress value as a width: "72%", "3/5", or a bare number read as a percentage. */
function progressWidth(cell: string): number {
  const fraction = /(-?\d[\d,]*(?:\.\d+)?)\s*\/\s*(\d[\d,]*(?:\.\d+)?)/.exec(cell);
  const ratio = fraction
    ? Number(fraction[1]!.replace(/,/g, "")) / Math.max(1e-9, Number(fraction[2]!.replace(/,/g, "")))
    : (measure(cell) ?? 0) / 100;
  return Math.max(0, Math.min(100, Math.round(ratio * 1000) / 10));
}

function progress(block: Block): string {
  const out = block.items.map((item) => {
    const [[label = "", value = "", note], tint] = tintedCells(item, 3);
    const noteHtml = note ? `<span class="rc-meter-note">${inline(note)}</span>` : "";
    return `<div class="rc-meter${tint}"><div class="rc-meter-head"><span>${inline(label)}</span><b>${inline(value)}</b></div><div class="rc-progress"><span style="width:${progressWidth(value)}%"></span></div>${noteHtml}</div>`;
  });
  return `${heading(block.arg)}<div class="rc-meters">${out.join("")}</div>`;
}

const CHART_TYPES = new Set(["line", "area", "bar", "stacked", "hbar", "donut", "pie"]);
/** The tag that makes a page inline the bundled Chart.js; the page keeps one copy. */
export const CHART_LIBRARY_TAG = '<script src="rustcode-lib:chart.js"></script>';

/** `:: chart` options after the title: `size=lg`, `line=150 normal`, `band=75-270`, `end`. */
function chartOptions(parts: string[]) {
  const options: {
    size?: string;
    line?: number;
    lineLabel?: string;
    band?: [number, number];
    end?: boolean;
  } = {};
  for (const part of parts) {
    const [key = "", value = ""] = part.split(/\s*=\s*/, 2).map((piece) => piece.trim());
    if (key === "size" && /^(sm|lg|xl)$/.test(value)) options.size = value;
    else if (key === "line") {
      const [at = "", ...label] = value.split(/\s+/);
      const line = measure(at);
      if (line !== null) options.line = line;
      if (label.length) options.lineLabel = label.join(" ");
    } else if (key === "band") {
      const match = /^(-?[\d.,]+)\s*(?:-|–|\.\.|to)\s*(-?[\d.,]+)$/.exec(value);
      if (match) options.band = [Number(match[1]!.replace(/,/g, "")), Number(match[2]!.replace(/,/g, ""))];
    } else if (key === "end") options.end = true;
  }
  return options;
}

/** Plain text for a canvas, which draws no markup: tags, backticks and bold marks dropped. */
const plain = (text: string) =>
  text
    .replace(/<[^>]*>/g, "")
    .replace(/`|\*\*/g, "")
    .trim();

function chart(block: Block): string {
  const [first = "", ...optionParts] = cells(block.arg);
  const [word = "", ...titleWords] = first.split(/\s+/);
  const kind = CHART_TYPES.has(word) ? word : "line";
  const title = CHART_TYPES.has(word) ? titleWords.join(" ") : first;
  const options = chartOptions(optionParts);
  const table = rows(block);
  const headed = table.length > 1 && !(table[0] ?? []).slice(1).every((cell) => measure(cell) !== null);
  const header = headed
    ? (table[0] ?? [])
    : ["", ...Array.from({ length: Math.max(1, (table[0]?.length ?? 2) - 1) }, () => "")];
  const data = headed ? table.slice(1) : table;
  const labels = data.map((row) => plain(row[0] ?? ""));
  const names = header.slice(1).map(plain);
  const series = (names.length ? names : [""]).map((label, i) => ({
    label,
    data: data.map((row) => measure(row[i + 1] ?? "")),
  }));
  const radial = kind === "donut" || kind === "pie";
  const type = radial
    ? kind === "donut"
      ? "doughnut"
      : "pie"
    : kind === "line" || kind === "area"
      ? "line"
      : "bar";
  const datasets = radial ? series.slice(0, 1) : series;
  const marks: Record<string, unknown> = {};
  if (options.band) marks.band = options.band;
  if (options.line !== undefined) marks.line = options.line;
  if (options.lineLabel) marks.lineLabel = options.lineLabel;
  if (options.end) marks.end = true;
  const config = {
    type,
    data: { labels, datasets },
    options: {
      ...(kind === "hbar" ? { indexAxis: "y" } : {}),
      plugins: {
        legend: { display: radial ? false : datasets.length > 1 },
        ...(Object.keys(marks).length ? { rcMarks: marks } : {}),
      },
      ...(kind === "stacked"
        ? { scales: { x: { stacked: true }, y: { stacked: true, beginAtZero: true } } }
        : {}),
      ...(!radial && kind !== "stacked"
        ? { scales: { [kind === "hbar" ? "x" : "y"]: { beginAtZero: true } } }
        : {}),
    },
  };
  const size = options.size ?? (radial ? "sm" : "");
  const label = plain(title) || names.join(", ") || "Chart";
  const canvas = `<canvas role="img" aria-label="${escapeAttr(label)}" data-rc-chart="${escapeAttr(JSON.stringify(config))}"></canvas>`;
  let total = "";
  if (radial) {
    const sum = (datasets[0]?.data ?? []).reduce<number>((acc, value) => acc + (value ?? 0), 0);
    const unit = names[0] || "total";
    total = `<div class="rc-donut-total"><b>${Math.round(sum * 100) / 100}</b><span>${escapeHtml(unit)}</span></div>`;
  }
  const box = `<div class="rc-chart${radial ? " rc-chart-radial" : ""}"${size ? ` data-size="${size}"` : ""}>${canvas}${total}</div>`;
  const legend = radial
    ? `<ul class="rc-legend">${labels
        .map(
          (name, i) =>
            `<li style="--rc-tint:var(--rc-series-${(i % 8) + 1})"><span>${escapeHtml(name)}</span><b>${escapeHtml(data[i]?.[1] ?? "")}</b></li>`,
        )
        .join("")}</ul>`
    : "";
  const body = radial ? `<div class="rc-chart-split">${box}${legend}</div>` : box;
  return `${CHART_LIBRARY_TAG}${heading(title)}${body}`;
}

/** A table block's cells: a number in a heatmap gets a shade from where it falls between the smallest and largest. */
function heatmap(block: Block): string {
  const { title, head: header, body: data } = tableParts(block);
  const values = data
    .flatMap((row) => row.slice(1).map((cell) => measure(cell)))
    .filter((v): v is number => v !== null);
  const min = Math.min(...values);
  const max = Math.max(...values);
  const thead = header.length
    ? `<thead><tr>${header.map((cell, i) => `<th${i ? ' class="rc-num"' : ""}>${inline(cell)}</th>`).join("")}</tr></thead>`
    : "";
  const tr = data
    .map((row) => {
      const tds = row.map((cell, i) => {
        const value = i ? measure(cell) : null;
        if (value === null) return `<td>${inline(cell)}</td>`;
        const share = max > min ? (value - min) / (max - min) : 1;
        return `<td class="rc-num rc-heat" style="--rc-heat:${Math.round(6 + share * 46)}%">${inline(cell)}</td>`;
      });
      return `<tr>${tds.join("")}</tr>`;
    })
    .join("");
  return `${heading(title)}<table class="rc-table rc-heatmap">${thead}<tbody>${tr}</tbody></table>`;
}

const MARKS: Record<string, [string, string]> = {
  yes: ["✓", "ok"],
  y: ["✓", "ok"],
  true: ["✓", "ok"],
  "✓": ["✓", "ok"],
  "✔": ["✓", "ok"],
  no: ["✕", "bad"],
  n: ["✕", "bad"],
  false: ["✕", "bad"],
  "✕": ["✕", "bad"],
  "✗": ["✕", "bad"],
  x: ["✕", "bad"],
  partial: ["◐", "warn"],
  some: ["◐", "warn"],
  "~": ["◐", "warn"],
};

/** A comparison cell: yes / no / partial (or a tick, a cross, `~`) as a tinted mark, then any words after it. */
function compareCell(cell: string): string {
  const match = /^(\S+)(?:\s+(.*))?$/.exec(cell.trim());
  const mark = match ? MARKS[match[1]!.toLowerCase()] : undefined;
  if (!mark) return tableCell(cell);
  const [symbol, tint] = mark;
  const rest = match![2] ? ` <span class="rc-muted">${inline(match![2])}</span>` : "";
  return `<span class="rc-mark rc-${tint}" aria-label="${escapeAttr(match![1]!)}">${symbol}</span>${rest}`;
}

function compare(block: Block): string {
  const { title, head: header, body: data } = tableParts(block);
  const thead = header.length
    ? `<thead><tr>${header.map((cell) => `<th>${inline(cell)}</th>`).join("")}</tr></thead>`
    : "";
  const tr = data
    .map(
      (row) =>
        `<tr>${row.map((cell, i) => (i ? `<td>${compareCell(cell)}</td>` : `<td><b>${inline(cell)}</b></td>`)).join("")}</tr>`,
    )
    .join("");
  return `${heading(title)}<table class="rc-table rc-compare">${thead}<tbody>${tr}</tbody></table>`;
}

function kv(block: Block): string {
  const out = block.items.map((item) => {
    const [[key = "", value = ""], tint] = tintedCells(item, 2);
    const shown = tint
      ? `<span class="rc-badge${tint}">${inline(value || tint.trim().slice(3))}</span>`
      : inline(value);
    return `<dt>${inline(key)}</dt><dd>${shown}</dd>`;
  });
  return `${heading(block.arg)}<dl class="rc-kv">${out.join("")}</dl>`;
}

function timeline(block: Block): string {
  const out = block.items.map((item) => {
    const [[time = "", event, detail], tint] = tintedCells(item, 3);
    if (event === undefined) return `<li class="rc-step${tint}">${inline(time)}</li>`;
    const detailHtml = detail ? `<p>${inline(detail)}</p>` : "";
    return `<li class="rc-step${tint}"><span class="rc-time">${inline(time)}</span><b>${inline(event)}</b>${detailHtml}</li>`;
  });
  return `${heading(block.arg)}<ol class="rc-steps rc-timeline">${out.join("")}</ol>`;
}

/** Severities from the most to the least urgent; anything else goes last. */
const SEVERITY_ORDER = ["critical", "high", "medium", "low", "nit"];

const severityRank = (severity: string) => {
  const at = SEVERITY_ORDER.indexOf(severity);
  return at < 0 ? SEVERITY_ORDER.length : at;
};

/**
 * A review's findings. One without a note is still open and gets a Fix chip that sends it back as
 * a message; two or more open ones also get Fix all open, most urgent first. A fixed (`{ok}`) or
 * set-aside (a plain note) finding gets none.
 */
function findings(block: Block): string {
  const open: { severity: string; where: string; finding: string }[] = [];
  const out = block.items.map((item) => {
    const [[severity = "", where = "", finding = "", note], tint] = tintedCells(item, 4);
    const level = plain(severity).toLowerCase();
    const noteHtml = note
      ? `<p class="rc-finding-note${tint === " rc-ok" ? " rc-ok" : ""}">${inline(note)}</p>`
      : "";
    let fix = "";
    if (!note && tint !== " rc-ok") {
      const entry = { severity: level, where: plain(where).split(" · ")[0] ?? "", finding: plain(finding) };
      open.push(entry);
      const send = `Fix this finding: ${entry.where ? `${entry.where} — ` : ""}${entry.finding}`;
      fix = `<div class="rc-finding-fix"><button class="rc-chip" data-rc-send="${escapeAttr(send)}">Fix</button></div>`;
    }
    return `<div class="rc-finding" data-severity="${escapeAttr(level)}"><div>${inline(where)}</div><p>${inline(finding)}</p>${noteHtml}${fix}</div>`;
  });
  let all = "";
  if (open.length > 1) {
    const lines = [...open]
      .sort((a, b) => severityRank(a.severity) - severityRank(b.severity))
      .map((f) => `- ${f.severity || "finding"}${f.where ? ` · ${f.where}` : ""}: ${f.finding}`);
    const send = ["Fix these open findings:", ...lines].join("\n");
    all = `<div class="rc-row rc-gap-2 rc-actions"><button class="rc-chip" data-rc-send="${escapeAttr(send)}">Fix all open</button></div>`;
  }
  return `${heading(block.arg)}<div class="rc-findings">${out.join("")}</div>${all}`;
}

/** `+12 -3` as added and removed counts; anything else as written. */
function changes(cell: string): string {
  if (!/^[+\-−]\d/.test(cell.trim())) return inline(cell);
  return cell
    .trim()
    .split(/\s+/)
    .map((part) =>
      /^\+\d/.test(part)
        ? `<span class="rc-add">${escapeHtml(part)}</span>`
        : /^[-−]\d/.test(part)
          ? `<span class="rc-del">${escapeHtml(part.replace("-", "−"))}</span>`
          : inline(part),
    )
    .join(" ");
}

function files(block: Block): string {
  const out = block.items.map((item) => {
    const [[path = "", change, note], tint] = tintedCells(item, 3);
    const noteHtml = note ? `<span class="rc-file-note">${inline(note)}</span>` : "";
    const name = /`/.test(path) ? inline(path) : `<code>${escapeHtml(path)}</code>`;
    return `<div class="rc-file${tint}"><span>${name}${noteHtml}</span>${change ? `<span>${changes(change)}</span>` : ""}</div>`;
  });
  return `${heading(block.arg)}<div class="rc-files">${out.join("")}</div>`;
}

function tags(block: Block): string {
  const out = [...block.items, ...block.text.flatMap((line) => cells(line))].map((item) => {
    const [text, tint] = tinted(item);
    return `<span class="rc-badge${tint}">${inline(text)}</span>`;
  });
  return `${heading(block.arg)}<div class="rc-row rc-gap-1 rc-tags">${out.join("")}</div>`;
}

function term(block: Block): string {
  const out = block.text.map((line) => {
    const [text, tint] = tinted(line);
    return `<div${tint ? ` class="${tint.trim()}"` : ""}>${escapeHtml(text) || " "}</div>`;
  });
  return `${heading(block.arg)}<div class="rc-term">${out.join("")}</div>`;
}

function code(block: Block): string {
  const [lang = ""] = block.arg.split(/\s+/);
  const label = lang ? ` data-lang="${escapeAttr(lang)}"` : "";
  return `<pre${label}><code>${escapeHtml(block.text.join("\n"))}</code></pre>`;
}

function detailsOpen(arg: string): string {
  const [summary = "", aside] = cells(arg, 2);
  const asideHtml = aside ? `<span class="rc-muted">${inline(aside)}</span>` : "";
  return `<details class="rc-details"><summary><span>${inline(summary || "Details")}</span>${asideHtml}</summary>`;
}

function details(block: Block): string {
  const [summary = "", aside] = cells(block.arg, 2);
  if (SOURCES_HEADING.test(summary) && (!aside || /^\d+$/.test(aside))) {
    return sources({ ...block, arg: summary });
  }
  const items = block.items.length ? list({ ...block, arg: "" }) : "";
  const text = block.text.length ? `<p>${inline(block.text.join(" "))}</p>` : "";
  return `${detailsOpen(block.arg)}${text}${items}</details>`;
}

/** Option separators: `|`, or a comma outside parentheses and brackets, so `waves (faster, more usage)` stays one option. */
function splitOptions(extra: string): string {
  let depth = 0;
  let out = "";
  for (const char of extra) {
    if (char === "(" || char === "[") depth++;
    else if ((char === ")" || char === "]") && depth > 0) depth--;
    out += char === "," && depth === 0 ? "|" : char;
  }
  return out;
}

const FIELD_KINDS = new Set(["text", "number", "date", "select", "choice", "check", "range"]);
let fieldIds = 0;

/** One form field: `kind name | Label | extra`, where extra is options, a placeholder, `0-100 = 40` or `on`. */
function field(item: string): string {
  const [spec = "", labelCell = "", extra = ""] = cells(item, 3);
  const [kindWord = "", nameWord = ""] = spec.split(/\s+/);
  const kind = FIELD_KINDS.has(kindWord) ? kindWord : "text";
  const name = (FIELD_KINDS.has(kindWord) ? nameWord : kindWord) || "value";
  const label = inline(labelCell || name);
  const attrName = escapeAttr(name);
  const id = `rc-f${++fieldIds}`;
  const options = cells(splitOptions(extra)).filter(Boolean);
  const chosen = (option: string) => /^\*/.test(option);
  const bare = (option: string) => option.replace(/^\*/, "").trim();
  const preset = options.findIndex(chosen);
  switch (kind) {
    case "select": {
      const opts = options
        .map(
          (option, i) =>
            `<option${i === Math.max(0, preset) ? " selected" : ""}>${escapeHtml(bare(option))}</option>`,
        )
        .join("");
      // design-lint: allow (markup for the sandboxed view page, not app chrome)
      return `<label class="rc-field" for="${id}"><span>${label}</span><select id="${id}" name="${attrName}">${opts}</select></label>`;
    }
    case "choice": {
      const opts = options
        .map(
          (option, i) =>
            `<label><input type="radio" name="${attrName}" value="${escapeAttr(bare(option))}"${i === Math.max(0, preset) ? " checked" : ""}><span>${escapeHtml(bare(option))}</span></label>`,
        )
        .join("");
      const words = options.map((option) => plain(bare(option)).length);
      const fitsOnOneSegmentedRow =
        options.length <= 4 && Math.max(0, ...words) <= 16 && words.reduce((a, b) => a + b, 0) <= 40;
      const variant = fitsOnOneSegmentedRow ? "rc-choice" : "rc-choice rc-choice-list";
      return `<div class="rc-field" role="radiogroup" aria-label="${escapeAttr(plain(labelCell || name))}"><span>${label}</span><div class="${variant}">${opts}</div></div>`;
    }
    case "check":
      return `<label class="rc-field rc-check"><input type="checkbox" name="${attrName}"${/^(on|yes|true|checked)$/i.test(extra.trim()) ? " checked" : ""}><span>${label}</span></label>`;
    case "range": {
      const match = /(-?[\d.]+)\s*(?:-|–|\.\.|to)\s*(-?[\d.]+)(?:\s*=\s*(-?[\d.]+))?/.exec(extra);
      const min = match ? match[1]! : "0";
      const max = match ? match[2]! : "100";
      const value = match?.[3] ?? String((Number(min) + Number(max)) / 2);
      return `<label class="rc-field" for="${id}"><span>${label}</span><span class="rc-range"><input id="${id}" type="range" name="${attrName}" min="${escapeAttr(min)}" max="${escapeAttr(max)}" value="${escapeAttr(value)}" data-rc-out><output class="rc-num">${escapeHtml(value)}</output></span></label>`;
    }
    default: {
      const type = kind === "number" || kind === "date" ? kind : "text";
      const placeholder = extra ? ` placeholder="${escapeAttr(plain(extra))}"` : "";
      return `<label class="rc-field" for="${id}"><span>${label}</span><input id="${id}" type="${type}" name="${attrName}"${placeholder}></label>`;
    }
  }
}

function form(block: Block): string {
  const [message = "", button] = cells(block.arg, 2);
  const send = plain(message) || "Apply";
  const fields = block.items.map(field).join("");
  return `<form class="rc-form" data-rc-send="${escapeAttr(send)}">${fields}<div class="rc-row rc-gap-2"><button data-primary>${inline(button || "Apply")}</button></div></form>`;
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
    case "head":
      return head(block);
    case "divider":
      return "<hr>";
    case "hero":
      return hero(block);
    case "progress":
      return progress(block);
    case "chart":
      return chart(block);
    case "heatmap":
      return heatmap(block);
    case "compare":
      return compare(block);
    case "kv":
      return kv(block);
    case "timeline":
      return timeline(block);
    case "findings":
      return findings(block);
    case "files":
      return files(block);
    case "tags":
      return tags(block);
    case "term":
      return term(block);
    case "code":
      return code(block);
    case "details":
      return details(block);
    case "form":
      return form(block);
    default:
      return sources(block);
  }
}

/** Blocks that take their lines as text rather than as `- ` items. */
const TEXT_BLOCKS = new Set(["p", "table", "callout", "chart", "heatmap", "compare", "term", "code"]);
/** Table-shaped blocks whose rows split into cells at `|`. */
const TABLE_BLOCKS = new Set(["table", "compare", "heatmap", "chart"]);
/** Blocks whose lines are kept as written: a line that starts with `<` or `- ` is still theirs. */
const RAW_BLOCKS = new Set(["term", "code"]);
/** Blocks that are one line, the directive itself. */
const LINE_BLOCKS = new Set(["title", "head", "h", "divider"]);
const HTML_TAG_LINE = /^\s*<[a-zA-Z/!?]/;
const INLINE_HTML_LINE =
  /^\s*<(?:a|abbr|b|bdi|bdo|cite|code|data|del|dfn|em|i|ins|kbd|mark|q|s|samp|small|span|strong|sub|sup|time|u|var)(?=[\s/>])/i;
const BLOCK_HTML_LINE =
  /^\s*<(?:\/|!--|!doctype|address|article|aside|blockquote|canvas|dd|details|dialog|div|dl|dt|fieldset|figcaption|figure|footer|form|h[1-6]|header|hr|iframe|li|link|main|meta|nav|ol|p|pre|script|section|style|summary|svg|table|tbody|td|template|tfoot|th|thead|title|tr|ul)(?=[\s/>]|$)/i;

/** Whether a non-empty line belongs to the open block rather than starting raw HTML after it. */
function continuesBlock(block: Block, line: string): boolean {
  if (!HTML_TAG_LINE.test(line)) return true;
  if (INLINE_HTML_LINE.test(line) && (TEXT_BLOCKS.has(block.name) || block.name === "sources")) return true;
  if (TABLE_BLOCKS.has(block.name) && !BLOCK_HTML_LINE.test(line) && cells(line).length > 1) return true;
  return false;
}

/** Whether a source uses the shorthand at all, so a plain HTML view is never touched. */
export const hasShorthand = (source: string) => /^\s*::\s*[a-z]+\b/m.test(source);

type TabSet = { slot: number; names: string[]; id: number };

const HTML_VOID_TAGS = new Set([
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

const HTML_OPEN_LINE = /^\s*<(?!(?:script|style|title)\b)[a-zA-Z][a-zA-Z0-9-]*(?=[\s/>])/i;

/** Update an open-tag stack across one HTML line so a `:: details` fold closes after its element. */
function advanceHtmlTags(line: string, stack: string[]) {
  let i = 0;
  while (i < line.length) {
    const lt = line.indexOf("<", i);
    if (lt < 0) break;
    if (line.startsWith("<!--", lt)) {
      const end = line.indexOf("-->", lt + 4);
      if (end < 0) break;
      i = end + 3;
      continue;
    }
    const closing = line[lt + 1] === "/";
    const nameAt = lt + (closing ? 2 : 1);
    const name = /^[a-zA-Z][a-zA-Z0-9-]*/.exec(line.slice(nameAt, nameAt + 48))?.[0]?.toLowerCase();
    if (!name) {
      i = lt + 1;
      continue;
    }
    let quote = "";
    let end = -1;
    for (let j = nameAt + name.length; j < line.length; j++) {
      const char = line[j];
      if (quote) {
        if (char === quote) quote = "";
      } else if (char === '"' || char === "'") quote = char;
      else if (char === ">") {
        end = j;
        break;
      }
    }
    if (end < 0) break;
    const selfClosing = line[end - 1] === "/";
    i = end + 1;
    if (closing) {
      const at = stack.lastIndexOf(name);
      if (at >= 0) stack.length = at;
    } else if (!HTML_VOID_TAGS.has(name) && !selfClosing) {
      stack.push(name);
    }
  }
}

/** Count `:: end` directives ahead that are not paired with a nested `:: tab` set. */
function countTrailingEnds(lines: string[], from: number): number {
  let ends = 0;
  let inTab = false;
  for (let j = from; j < lines.length; j++) {
    const match = DIRECTIVE.exec(lines[j]!);
    if (!match || !BLOCKS.has(match[1]!)) continue;
    if (match[1] === "tab") inTab = true;
    else if (match[1] === "end") {
      if (inTab) inTab = false;
      else ends++;
    }
  }
  return ends;
}

/**
 * A view's source with its shorthand blocks written out as kit HTML. A view without any
 * is returned as is. A block that is still arriving is drawn with the items it has, so
 * a streaming view grows a row at a time.
 */
export function expandShorthand(source: string): string {
  if (!hasShorthand(source)) return source;
  fieldIds = 0;
  const out: string[] = [];
  let block: Block | null = null;
  let raw: string | null = null;
  let tabs: TabSet | null = null;
  let tabSets = 0;
  let detailsHtml: string[] | null = null;
  let openDetails = 0;
  const closeTabs = () => {
    const set = tabs;
    if (!set) return;
    out.push("</section></div>");
    out[set.slot] = `<div class="rc-tabs"><div role="tablist">${set.names
      .map((name, i) => `<button role="tab" data-rc-tab="t${set.id}-${i}">${inline(name)}</button>`)
      .join("")}</div>`;
    tabs = null;
  };
  const closeDetailsHtml = () => {
    if (detailsHtml === null) return;
    out.push("</details>");
    detailsHtml = null;
  };
  const flush = () => {
    if (block) out.push(render(block));
    block = null;
  };
  const lines = source.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]!;
    if (raw) {
      out.push(line);
      if (new RegExp(`</${raw}\\s*>`, "i").test(line)) raw = null;
      continue;
    }
    if (detailsHtml !== null) {
      if (detailsHtml.length > 0) {
        out.push(line);
        advanceHtmlTags(line, detailsHtml);
        continue;
      }
      const directiveAfterHtml = DIRECTIVE.exec(line);
      if (directiveAfterHtml && directiveAfterHtml[1] === "end") {
        closeDetailsHtml();
        continue;
      }
      if (HTML_OPEN_LINE.test(line)) {
        out.push(line);
        advanceHtmlTags(line, detailsHtml);
        continue;
      }
      closeDetailsHtml();
    }
    const directive = DIRECTIVE.exec(line);
    if (directive && BLOCKS.has(directive[1]!)) {
      const name = directive[1]!;
      const arg = directive[2]!.trim();
      const emptyDetails =
        block !== null && block.name === "details" && block.items.length === 0 && block.text.length === 0;
      if (emptyDetails && name === "sources") {
        if (!arg) {
          block = { name: "sources", arg: block!.arg, items: [], text: [] };
          continue;
        }
        if (SOURCES_HEADING.test(cells(block!.arg, 2)[0] ?? "")) {
          block = { name: "sources", arg, items: [], text: [] };
          continue;
        }
      }
      if (
        emptyDetails &&
        name !== "tab" &&
        name !== "end" &&
        name !== "title" &&
        countTrailingEnds(lines, i + 1) > (tabs ? 1 : 0) + openDetails
      ) {
        out.push(detailsOpen(block!.arg));
        openDetails++;
        block = null;
      } else {
        flush();
      }
      if (name === "tab") {
        while (openDetails > 0) {
          out.push("</details>");
          openDetails--;
        }
        let set = tabs as TabSet | null;
        if (set) out.push("</section>");
        else {
          set = { slot: out.length, names: [], id: ++tabSets };
          tabs = set;
          out.push("");
        }
        set.names.push(arg || `Tab ${set.names.length + 1}`);
        out.push(`<section data-rc-panel="t${set.id}-${set.names.length - 1}">`);
        continue;
      }
      if (name === "end") {
        if (openDetails > 0) {
          out.push("</details>");
          openDetails--;
        } else {
          closeTabs();
        }
        continue;
      }
      const next: Block = { name, arg, items: [], text: [] };
      if (LINE_BLOCKS.has(next.name)) out.push(render(next));
      else block = next;
      continue;
    }
    const current = block as Block | null;
    if (current && current.name === "details" && current.items.length === 0 && current.text.length === 0) {
      if (HTML_OPEN_LINE.test(line)) {
        out.push(detailsOpen(current.arg));
        block = null;
        detailsHtml = [];
        out.push(line);
        advanceHtmlTags(line, detailsHtml);
        continue;
      }
      if (!line.trim()) {
        let nextIndex = i + 1;
        while (nextIndex < lines.length && !lines[nextIndex]!.trim()) nextIndex++;
        const nextLine = lines[nextIndex] ?? "";
        const nextDirective = DIRECTIVE.exec(nextLine);
        if (
          HTML_OPEN_LINE.test(nextLine) ||
          (nextDirective && nextDirective[1] === "sources" && !nextDirective[2]!.trim())
        ) {
          continue;
        }
      }
    }
    if (current && RAW_BLOCKS.has(current.name) && line.trim()) {
      current.text.push(current.name === "code" ? line : line.trim());
      continue;
    }
    if (current && line.trim() && continuesBlock(current, line)) {
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
  closeDetailsHtml();
  while (openDetails > 0) {
    out.push("</details>");
    openDetails--;
  }
  closeTabs();
  return out.join("\n");
}
