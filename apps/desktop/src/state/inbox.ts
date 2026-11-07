// The needs-you inbox, across every thread: the approvals and questions that wait,
// the turns that failed while you looked elsewhere, and the delegates that ask for
// you — one row each, newest first. Pure, so the sidebar's badge, the inbox view,
// the palette and their tests read the same list (ui/Inbox.tsx).
//
// Answered rows do not vanish from the view: the row a click or a key answered is
// remembered (`foldDecided`) and shown in a faded "Decided" section until the app
// is left. Nothing is written to disk, so a reload starts the inbox clean.

import { type PermissionRequest, projectName, type SessionSummary } from "../acp/types";
import type { Link } from "./orchestration";

/** What a row is. `delegate` is a link that asks for the user without a request in the app yet. */
export type InboxKind = "approval" | "question" | "failed" | "delegate";

export interface InboxItem {
  /** Stable within a view: the request's rpc id, the failure's thread, or the link's id. */
  key: string;
  kind: InboxKind;
  threadId: string;
  /** The thread's title, or a stand-in when the app never read one. */
  title: string;
  /** The thread's folder, for its project mark and name. */
  project: string;
  /** What it wants, in one line: a command, a question, a failure, a delegate. */
  detail: string;
  /** When it arrived, newest first in the list. */
  at: number;
  /** The pending request, for the row's actions. */
  request?: PermissionRequest;
  /** How many approvals the thread waits on, and which one this is. */
  pending: number;
  number: number;
}

/** What {@link deriveInbox} reads of the app. */
export interface InboxRead {
  /** The thread list, for titles and folders of threads the app has not opened. */
  sessions: readonly SessionSummary[];
  /** Each loaded thread's pending requests, its title, and how its last turn ended. */
  threads: Readonly<
    Record<
      string,
      {
        title?: string;
        cwd?: string;
        permissions: readonly PermissionRequest[];
        items?: readonly { kind: string; outcome?: string; at?: number }[];
      }
    >
  >;
  /** Threads whose last turn failed while you looked elsewhere (state/store.ts `failed`). */
  failed: Readonly<Record<string, number>>;
  /** The link book, for delegates that ask for the user. */
  links: readonly Link[];
}

/** The title a thread is known by, when the app has one. */
function knownTitle(read: InboxRead, id: string): string | undefined {
  return read.threads[id]?.title ?? read.sessions.find((s) => s.id === id)?.title;
}

/** The title a thread is known by, or a stand-in. */
function titleOf(read: InboxRead, id: string): string {
  return knownTitle(read, id) || "Untitled thread";
}

/** The folder a thread runs in, for the row's project mark. */
function projectOf(read: InboxRead, id: string): string {
  const thread = read.threads[id];
  if (thread?.cwd) return thread.cwd;
  const session = read.sessions.find((s) => s.id === id);
  return session?.worktree?.repo || session?.cwd || thread?.title || "";
}

/** What a request asks for, in one line. */
export function requestDetail(request: PermissionRequest): string {
  if (request.question) return request.question.text;
  return `Wants to ${request.title}`;
}

/** One request as a row, with its place in the thread's approvals. */
function requestItem(
  read: InboxRead,
  threadId: string,
  request: PermissionRequest,
  pending: number,
  number: number,
  fallback: number,
): InboxItem {
  return {
    key: `${threadId}:request:${request.rpcId}`,
    kind: request.question ? "question" : "approval",
    threadId,
    title: titleOf(read, threadId),
    project: projectOf(read, threadId),
    detail: requestDetail(request),
    at: request.at ?? fallback,
    request,
    pending: Math.max(1, pending),
    number,
  };
}

/**
 * Every row the inbox shows, newest first. A pending request names its thread;
 * a failed turn names the failure; a delegate that asks is a row of its own only
 * when the app does not already hold the request it asks with, so one question is
 * never listed twice.
 */
export function deriveInbox(read: InboxRead, now = Date.now()): InboxItem[] {
  const items: InboxItem[] = [];
  const asked = new Set<string>();

  for (const [threadId, thread] of Object.entries(read.threads)) {
    if (!thread.permissions.length) continue;
    asked.add(threadId);
    const approvals = thread.permissions.filter((request) => !request.question).length;
    let number = 0;
    for (const request of thread.permissions) {
      if (!request.question) number++;
      items.push(requestItem(read, threadId, request, approvals, request.question ? 1 : number, now));
    }
  }

  const failedThreads = new Set<string>();
  for (const [threadId, at] of Object.entries(read.failed)) {
    failedThreads.add(threadId);
    items.push({
      key: `${threadId}:failed`,
      kind: "failed",
      threadId,
      title: titleOf(read, threadId),
      project: projectOf(read, threadId),
      detail: "Its turn failed",
      at,
      pending: 1,
      number: 1,
    });
  }
  // A replayed thread whose last turn failed is a failure too, even when this app
  // sent no prompt: the mock seeds one, and a thread opened after a restart has one.
  for (const [threadId, thread] of Object.entries(read.threads)) {
    if (failedThreads.has(threadId)) continue;
    const tail = thread.items?.[thread.items.length - 1];
    if (tail?.kind !== "turnEnd" || tail.outcome !== "failed") continue;
    items.push({
      key: `${threadId}:failed`,
      kind: "failed",
      threadId,
      title: titleOf(read, threadId),
      project: projectOf(read, threadId),
      detail: "Its turn failed",
      at: tail.at ?? now,
      pending: 1,
      number: 1,
    });
  }

  for (const link of read.links) {
    if (link.state !== "needs_user" || asked.has(link.target)) continue;
    items.push({
      key: `${link.id}:delegate`,
      kind: "delegate",
      threadId: link.target,
      title: knownTitle(read, link.target) || link.targetTitle || "Untitled thread",
      project: projectOf(read, link.target),
      detail: "A delegated thread needs you",
      at: link.createdAt,
      pending: 1,
      number: 1,
    });
  }

  return items.sort((a, b) => b.at - a.at);
}

/** A row that left the list because it was answered, retried or opened. */
export interface DecidedItem extends InboxItem {
  /** When it left the list. */
  decidedAt: number;
  /** What the user did: `Approved`, `Declined`, `Answered`, `Retried`, `Opened`. */
  label: string;
}

/** How many decided rows the view keeps before the oldest fall away. */
export const DECIDED_MAX = 12;

/**
 * The decided section after a change: rows the inbox had and no longer has are
 * folded in, newest first, capped at {@link DECIDED_MAX}. A first render (nothing
 * seen yet) decides nothing, so opening the view does not fold a history it never
 * showed.
 */
export function foldDecided(
  previous: readonly InboxItem[],
  next: readonly InboxItem[],
  decided: readonly DecidedItem[],
  opts: { now?: number; labels?: Readonly<Record<string, string>> } = {},
): DecidedItem[] {
  if (!previous.length) return [...decided];
  const now = opts.now ?? Date.now();
  const live = new Set(next.map((item) => item.key));
  const gone = previous.filter((item) => !live.has(item.key));
  if (!gone.length) return [...decided];
  const folded = gone.map((item) => ({
    ...item,
    decidedAt: now,
    label: opts.labels?.[item.key] ?? "Handled",
  }));
  const seen = new Set(folded.map((item) => item.key));
  return [...folded, ...decided.filter((item) => !seen.has(item.key))].slice(0, DECIDED_MAX);
}

/** The project name a row's mark shows (`acp/types.ts` `projectName`). */
export function inboxProjectName(item: InboxItem): string {
  return projectName(item.project);
}
