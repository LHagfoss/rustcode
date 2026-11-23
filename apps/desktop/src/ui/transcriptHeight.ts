// Initial row sizes for the virtualized transcript, before rendered rows are measured.
import { COLLAPSED_MESSAGE_HEIGHT, messageCollapses, splitImageMarkers } from "../state/compose";
import type { Row } from "./Transcript";

/**
 * The pieces an estimate is made of, in the sizes the rows are drawn at: a sent bubble's padding and
 * the line it sits on, the time line under it, and the chips above it when it carries pictures or files.
 */
const BUBBLE_PADDING = 8;
const BODY_LINE = 24;
const META_LINE = 24;
const ATTACHMENT_CHIPS = 32;
/** A row of picture thumbnails (ui/Rows.tsx `THUMBNAIL`, 120px) and the 6px under it. */
const THUMBNAIL_ROW = 126;
/** A reply's picture while it loads (ui/MarkdownMedia.tsx, 240px) and the paragraph gap after it. */
const PICTURE_PLACEHOLDER = 256;
const MARKDOWN_PICTURE = /!\[[^\]\n]*\]\([^)\n]+\)/g;
/** The "Show full message" pill a folded bubble ends with, and its air above it. */
const SHOW_FULL_PILL = 30;

export function estimatedRowHeight(row: Row | undefined, hidden = false): number {
  if (hidden) return 0;
  if (!row) return 24;
  if (row.approval) return 96;
  if (row.childRequest) return row.childRequest.request.question ? 240 : 52;
  if (row.plan) return 260;
  if (row.fold || row.group) return 26;
  if (row.updates) return 26;
  if (row.queued) return 96;
  if (row.resume) return 30;
  if (row.branch) return 24;
  if (row.queueWake || row.queueNotice) return 26;
  // The turn's own lines: `Working for 12s` is `h-[34px]` over its hairline, and the live activity
  // is `min-h-6`. Left at a generic 24 they measured 39px taller a frame after mounting, and the
  // pinned view took that back in one step — the message just sent jumped with it.
  if (row.working) return 35;
  if (row.activity) return 24;
  const item = row.item;
  if (!item) return 24;
  if (item.kind === "tool" || item.kind === "turnEnd") return 26;
  // The status card: a header line, then two lines a delegate.
  if (item.kind === "status") return 34 + item.rows.length * 44;
  // Thoughts start collapsed; the virtualizer measures the row when expanded.
  if (item.kind === "thought") return 24;
  // A sent bubble: its pictures and files above it, 8px of padding each way, its text, and the
  // 24px line under it that holds the time (shown on hover, laid out always). A long one folds at
  // `COLLAPSED_MESSAGE_HEIGHT` and keeps its "Show full message" pill, so its estimate never
  // reserves a screenful the row will not use.
  if (item.kind === "user") {
    const body = lines(item.text, 60) * BODY_LINE;
    const text = messageCollapses(body) ? COLLAPSED_MESSAGE_HEIGHT + SHOW_FULL_PILL : body;
    const sent = item.attachments ?? [];
    const pictures = sent.some((a) => a.kind === "image") || splitImageMarkers(item.text).paths.length > 0;
    const above = pictures
      ? THUMBNAIL_ROW
      : sent.length || splitImageMarkers(item.text).lost
        ? ATTACHMENT_CHIPS
        : 0;
    return above + BUBBLE_PADDING * 2 + text + META_LINE;
  }
  const text = "text" in item ? item.text : "";
  // A reply's pictures take their placeholder's height each (ui/MarkdownMedia.tsx), not a line.
  const pictures = text.includes("![") ? (text.match(MARKDOWN_PICTURE)?.length ?? 0) : 0;
  return Math.min(600, 40 + text.length / 4) + pictures * PICTURE_PLACEHOLDER;
}

/** The lines a string wraps to in the lane, near enough for an estimate. */
function lines(text: string, perLine: number): number {
  return Math.max(1, Math.ceil(text.length / perLine));
}

/** The transcript's vertical rhythm (docs/design.md › Type). */
const FLOW_GAP = 4;
const FOLD_GAP = 8;
const ITEM_GAP = 12;
const FOOTER_GAP = 16;
const TURN_GAP = 28;

/** A step of the work: one line of the turn's process, not its answer. */
function flowRow(row: Row | undefined): boolean {
  if (!row) return false;
  if (row.group || row.agents || row.threads) return true;
  return row.item?.kind === "tool" || row.item?.kind === "thought";
}

/**
 * The air above a row, from the transcript's rhythm (above): a turn starts and a footer ends on the
 * wide gap, the steps of the work sit tight, and the first line under a divider takes the narrow
 * gap. The rows draw it and so does `estimateSize`, because a row that mounts with a size that
 * leaves this out measures a gap taller a frame later — and the pinned transcript takes that
 * difference back in one step, moving every row above it, the message just sent included.
 */
export function rowGap(rows: readonly Row[], index: number): number {
  if (index <= 0) return 0;
  const item = rows[index]?.item;
  const prev = rows[index - 1]?.item;
  if (flowRow(rows[index]) && (flowRow(rows[index - 1]) || rows[index - 1]?.fold)) return FLOW_GAP;
  if (rows[index - 1]?.fold) return FOLD_GAP;
  if (item?.kind === "turnEnd" && prev?.kind === "assistant") return FOOTER_GAP;
  if ((prev?.kind === "notice" && prev.tone === "info") || prev?.kind === "user") return ITEM_GAP;
  return item?.kind === "user" ? TURN_GAP : ITEM_GAP;
}
