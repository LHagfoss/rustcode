// The right panel's overview as data (ui/ThreadDetails.tsx draws it): which
// sections are folded away, the pull requests a thread's branches have, what it
// delivered, and the log of its turns. The drawing lives in the section files
// beside this one; the rules live here so the tests drive them directly.

import type { PullRequest } from "../acp/rustcode";
import type { FileDiff } from "../acp/types";
import { rowTokens, usageDelta } from "../lib/trace";
import { type DelegateNode, delegateTree } from "../state/delegates";
import type { Link } from "../state/orchestration";
import { diffCounts, stripHarnessMarkup, type TranscriptItem, type TurnUsage } from "../state/thread";

// ---------------------------------------------------------------- folded sections

/** The overview's sections, in the order the panel draws them. */
export const OVERVIEW_SECTIONS = ["properties", "pulls", "deliverables", "log", "details"] as const;
export type OverviewSection = (typeof OVERVIEW_SECTIONS)[number];

/**
 * The sections a user opens on: everything the panel is for, then the rare
 * facts. A section folded away is kept folded for every thread, not per thread:
 * it is the reader's habit, not the thread's state.
 */
export const FOLDED_BY_DEFAULT: readonly OverviewSection[] = ["details"];

/** Where the folded sections are kept: the app's storage, so every window agrees. */
export const FOLDED_KEY = "rustcode.overview.folded";

/** The folded sections in a stored value; a missing or broken one is the default. */
export function parseFolded(raw: string | null | undefined): OverviewSection[] {
  if (raw === null || raw === undefined || raw === "") return [...FOLDED_BY_DEFAULT];
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [...FOLDED_BY_DEFAULT];
    return OVERVIEW_SECTIONS.filter((id) => parsed.includes(id));
  } catch {
    return [...FOLDED_BY_DEFAULT];
  }
}

/** The folded sections after a press on one header: it flips, the others keep their place in order. */
export function toggleFolded(folded: readonly OverviewSection[], id: OverviewSection): OverviewSection[] {
  return OVERVIEW_SECTIONS.filter((section) =>
    section === id ? !folded.includes(section) : folded.includes(section),
  );
}

export const isFolded = (folded: readonly OverviewSection[], id: OverviewSection): boolean =>
  folded.includes(id);

export function readFolded(): OverviewSection[] {
  try {
    return parseFolded(localStorage.getItem(FOLDED_KEY));
  } catch {
    return [...FOLDED_BY_DEFAULT];
  }
}

export function writeFolded(folded: readonly OverviewSection[]): void {
  try {
    localStorage.setItem(FOLDED_KEY, JSON.stringify(folded));
  } catch {
    // Not kept in a private window.
  }
}

// ---------------------------------------------------------------- deliverables

/** How the Deliverables section draws what it has. */
export type DeliverableMode = "grid" | "list";
export const DELIVERABLE_MODE_KEY = "rustcode.overview.deliverables";

export function readDeliverableMode(): DeliverableMode {
  try {
    return localStorage.getItem(DELIVERABLE_MODE_KEY) === "list" ? "list" : "grid";
  } catch {
    return "grid";
  }
}

export function writeDeliverableMode(mode: DeliverableMode): void {
  try {
    localStorage.setItem(DELIVERABLE_MODE_KEY, mode);
  } catch {
    // Not kept in a private window.
  }
}

/** A picture a reply shows, by the path the reply named. */
export interface DeliverablePicture {
  key: string;
  src: string;
  label: string;
}

/** A file the thread wrote or edited: its path inside the thread's folder. */
export interface DeliverableFile {
  key: string;
  path: string;
  label: string;
  added: number;
  removed: number;
}

export interface Deliverables {
  pictures: DeliverablePicture[];
  files: DeliverableFile[];
}

/** `![alt](src)` in a reply; a title after the source is allowed and dropped. */
const MARKDOWN_IMAGE = /!\[([^\]]*)\]\(\s*([^)\s]+)(?:\s+"[^"]*")?\s*\)/g;

/** The last folder or file a path names, for a tile whose path is too long to read. */
export function baseName(path: string): string {
  const parts = path.split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/**
 * What each delivery costs to find, kept per item: the panel reads a transcript
 * that grows by a chunk at a time, and a line diff or a scan of a long reply is
 * not worth repeating while the thread streams.
 */
const pictureCache = new WeakMap<object, { text: string; found: DeliverablePicture[] }>();
const countCache = new WeakMap<FileDiff, { added: number; removed: number }>();

function picturesOf(item: { kind: "assistant"; text: string }, pictures: DeliverablePicture[]): void {
  const cached = pictureCache.get(item);
  if (cached?.text === item.text) {
    for (const picture of cached.found)
      if (!pictures.some((p) => p.src === picture.src)) pictures.push(picture);
    return;
  }
  const found: DeliverablePicture[] = [];
  for (const match of item.text.matchAll(MARKDOWN_IMAGE)) {
    const src = decodeURIComponentSafe(match[2] ?? "");
    // A web or data picture is drawn in the reply already, and is not the thread's.
    if (!src || /^(?:https?:|data:|blob:)/i.test(src)) continue;
    if (!found.some((picture) => picture.src === src)) {
      found.push({ key: src, src, label: match[1]?.trim() || baseName(src) });
    }
  }
  pictureCache.set(item, { text: item.text, found });
  for (const picture of found) if (!pictures.some((p) => p.src === picture.src)) pictures.push(picture);
}

function countsOf(diff: FileDiff): { added: number; removed: number } {
  let counts = countCache.get(diff);
  if (!counts) {
    counts = diffCounts([diff]);
    countCache.set(diff, counts);
  }
  return counts;
}

/**
 * What a thread delivered: the pictures its replies show and the files it wrote.
 * A picture from the web or a `data:` URL is the reply's, not a file of the
 * thread's, so it is left out; a file is counted once however often it was
 * edited, with every edit's lines added up.
 */
export function deliverables(items: readonly TranscriptItem[]): Deliverables {
  const pictures: DeliverablePicture[] = [];
  const files = new Map<string, DeliverableFile>();
  for (const item of items) {
    if (item.kind === "assistant") {
      picturesOf(item, pictures);
      continue;
    }
    if (item.kind !== "tool" || item.status === "failed") continue;
    for (const diff of item.diffs) addFile(files, diff);
  }
  return { pictures, files: [...files.values()] };
}

function addFile(files: Map<string, DeliverableFile>, diff: FileDiff): void {
  const counts = countsOf(diff);
  const seen = files.get(diff.path);
  files.set(diff.path, {
    key: diff.path,
    path: diff.path,
    label: baseName(diff.path),
    added: (seen?.added ?? 0) + counts.added,
    removed: (seen?.removed ?? 0) + counts.removed,
  });
}

function decodeURIComponentSafe(text: string): string {
  try {
    return decodeURIComponent(text);
  } catch {
    return text;
  }
}

// ---------------------------------------------------------------- pull requests

/** The pill a pull request's row ends on: its state in one word, in its tone. */
export interface PullPill {
  label: "Draft" | "Merged" | "Closed" | "Checks failing" | "Checks running" | "Ready" | "Open";
  tone: "accent" | "success" | "warning" | "muted" | "danger";
}

/**
 * What a pull request's pill says. A merged pull is the thread's outcome and
 * keeps its own word; a draft says so before its checks, an open one reads its
 * checks, and a fresh pull with nothing reported is simply ready.
 */
export function pullPill(pull: PullRequest): PullPill {
  if (pull.state === "merged") return { label: "Merged", tone: "accent" };
  if (pull.state === "closed") return { label: "Closed", tone: "muted" };
  if (pull.draft) return { label: "Draft", tone: "muted" };
  if (pull.checks.failed > 0) return { label: "Checks failing", tone: "warning" };
  if (pull.checks.pending > 0) return { label: "Checks running", tone: "muted" };
  if (pull.checks.passed > 0) return { label: "Ready", tone: "success" };
  return { label: "Open", tone: "muted" };
}

/** Open work first, then what became of the older branches, the busiest branch last. */
const STATE_RANK: Record<PullRequest["state"], number> = { open: 0, merged: 1, closed: 2 };

/**
 * The pull requests a thread's branches have: its own first, then the branches
 * of the threads it started. A thread with no branch (one that never worked in
 * a worktree) matches nothing, and a pull is drawn once however many branches
 * name it.
 */
export function pullsForBranches(
  pulls: readonly PullRequest[] | undefined,
  branches: readonly (string | undefined)[],
): PullRequest[] {
  const wanted = branches.filter((branch): branch is string => !!branch);
  if (!wanted.length || !pulls?.length) return [];
  const seen = new Set<number>();
  const found: PullRequest[] = [];
  for (const pull of pulls) {
    if (!wanted.includes(pull.headRef) || seen.has(pull.number)) continue;
    seen.add(pull.number);
    found.push(pull);
  }
  const rank = (pull: PullRequest) => STATE_RANK[pull.state] ?? 3;
  return found.sort(
    (one, other) =>
      rank(one) - rank(other) ||
      wanted.indexOf(one.headRef) - wanted.indexOf(other.headRef) ||
      (other.updatedAt ?? 0) - (one.updatedAt ?? 0),
  );
}

/** Every thread a thread started, however deep, in the order the tree draws them. */
export function delegatedIds(root: string, links: readonly Link[]): string[] {
  return flattenTree(delegateTree(root, links)).map((node) => node.id);
}

function flattenTree(nodes: readonly DelegateNode[]): DelegateNode[] {
  return nodes.flatMap((node) => [node, ...flattenTree(node.children)]);
}

/** The branches a thread's pull requests can be on: its own, then its delegates'. */
export function pullBranches(own: string | undefined, delegated: readonly (string | undefined)[]): string[] {
  return [own, ...delegated].filter((branch): branch is string => !!branch);
}

// ---------------------------------------------------------------- the execution log

/** One finished turn, as the Execution log lists it. */
export interface TurnEntry {
  id: string;
  /** The first line of the message that opened it, or what woke the thread. */
  prompt: string;
  at?: number;
  /** When the app saw the turn begin; a replayed turn has none, and so no waterfall. */
  startedAt?: number;
  outcome: "completed" | "cancelled" | "failed";
  elapsedMs?: number;
  /** What this turn cost, when the engine reported usage while it ran. */
  cost?: number;
  /** Its input and output tokens together, when the engine reported them. */
  tokens?: number;
}

/** What the log's stats row adds up (docs/design.md › the panel's rates). */
export interface LogTotals {
  /** The thread's own reported cost, which is what the store says it spent. */
  spent?: number;
  currency: string;
  tokens?: number;
  /** How long the thread's turns ran, in all. */
  agentMs: number;
}

/** What a turn used: the last sample the engine sent it, minus the session's totals at its start. */
function turnUsage(usage: TurnUsage | undefined): { cost?: number; tokens?: number } {
  const last = usage?.samples.at(-1)?.usage;
  const delta = usageDelta(last, usage?.before);
  if (!delta) return {};
  return { cost: delta.cost, tokens: rowTokens(delta) };
}

/** A turn's opening line, without harness markup or the wake marker that starts one. */
function promptLine(text: string): string {
  const line = stripHarnessMarkup(text)
    .split("\n")
    .find((part) => part.trim());
  return line?.trim() ?? "";
}

/**
 * The thread's turns, oldest first, each with the message that opened it. A
 * turn the app woke the thread for has no message of its own, so its row says
 * what woke it instead of borrowing the one above.
 */
export function turnEntries(items: readonly TranscriptItem[]): TurnEntry[] {
  const entries: TurnEntry[] = [];
  let prompt = "";
  let woke = false;
  for (const item of items) {
    if (item.kind === "user") {
      woke = !!item.wake;
      prompt = woke ? "Woken for a delegate" : promptLine(item.text);
      continue;
    }
    if (item.kind !== "turnEnd") continue;
    entries.push({
      id: item.id,
      prompt: prompt || "No message",
      at: item.at,
      startedAt: item.startedAt,
      outcome: item.outcome,
      elapsedMs: item.elapsedMs,
      ...turnUsage(item.usage),
    });
  }
  return entries;
}

/** The log's stats: what the thread spent, the tokens it read and wrote, and how long it worked. */
export function logTotals(
  turns: readonly TurnEntry[],
  usage: { inputTokens?: number; outputTokens?: number; cost?: number; currency?: string } | undefined,
): LogTotals {
  const agentMs = turns.reduce((sum, turn) => sum + (turn.elapsedMs ?? 0), 0);
  const reported = (usage?.inputTokens ?? 0) + (usage?.outputTokens ?? 0);
  const counted = usage?.inputTokens !== undefined || usage?.outputTokens !== undefined;
  // A harness that reported no session total keeps the turns' own numbers.
  const tokens = counted
    ? reported
    : turns.some((turn) => turn.tokens !== undefined)
      ? turns.reduce((sum, turn) => sum + (turn.tokens ?? 0), 0)
      : undefined;
  const spent = usage?.cost;
  return {
    spent,
    currency: usage?.currency ?? "USD",
    tokens,
    agentMs,
  };
}

/**
 * Whether a turn's log row has a waterfall to open: the same rule the transcript
 * uses for its Trace chip (`hasTimeline`, lib/trace.ts), so the two agree.
 */
export const turnHasTrace = (turn: TurnEntry): boolean =>
  turn.startedAt !== undefined && turn.at !== undefined && turn.at > turn.startedAt;

/** A path a diff names, made absolute against the thread's folder for the shell to open. */
export function absolutePath(folder: string, path: string): string {
  if (/^(?:[a-zA-Z]:[\\/]|[\\/])/.test(path)) return path;
  return `${folder.replace(/[\\/]+$/, "")}/${path}`;
}
