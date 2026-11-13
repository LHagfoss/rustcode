// What a screen reader is told about threads (issue 3609): the window's title names the open
// thread and its status, and a thread's major moments are announced once each (`lib/announce.ts`).
// The open thread says when it starts working, finishes, asks, waits on an approval or fails; a
// thread in the background only interrupts for what needs the user: a question, an approval or a
// failure. Streamed text and tool calls are never announced.

import { isTauri } from "../acp/transport";
import type { ThreadState } from "./thread";

/** The part of a thread an announcement is read from. */
export interface ThreadMoment {
  /** A turn of its own runs. */
  running: boolean;
  /** The running turn is the app's own (an automatic compaction): it is not the user's news. */
  quiet?: boolean;
  /** The request it waits on: the oldest, so a second one queued behind it is not news yet. */
  request?: { key: string; question: boolean };
  /** Its last turn, or opening it, failed. */
  failed: boolean;
  /** It goes on by itself: background commands it parked on, or a queued message. */
  continues: boolean;
  /** The replay of `session/load` is still arriving: nothing in it is news. */
  loading: boolean;
}

export interface Announcement {
  text: string;
  assertive: boolean;
}

/** One thread's moment, from what the store holds of it; `failedAt` is `AppState.failed[id]`. */
export function momentOf(thread: ThreadState | undefined, failedAt: number | undefined): ThreadMoment {
  const request = thread?.permissions.reduce<ThreadState["permissions"][number] | undefined>(
    (oldest, p) => (!oldest || (p.at ?? 0) < (oldest.at ?? 0) ? p : oldest),
    undefined,
  );
  return {
    running: thread?.running ?? false,
    quiet: thread?.quiet,
    request: request ? { key: String(request.rpcId), question: !!request.question } : undefined,
    failed: !thread?.running && (!!thread?.error || failedAt !== undefined),
    continues: (thread?.waiting ?? 0) > 0 || (thread?.queue.length ?? 0) > 0,
    loading: thread?.loading ?? false,
  };
}

/**
 * What changed between two moments of one thread that is worth saying, or null. `active` is the
 * thread on screen: it says every major moment, a background thread only what needs the user.
 * A thread seen for the first time says nothing: what it already was is not a change.
 */
export function threadAnnouncement(
  title: string,
  prev: ThreadMoment | undefined,
  next: ThreadMoment,
  active: boolean,
): Announcement | null {
  if (!prev || next.loading) return null;
  if (next.request && next.request.key !== prev.request?.key) {
    return {
      text: `${title}: ${next.request.question ? "asks a question" : "needs approval"}`,
      assertive: true,
    };
  }
  if (next.failed && !prev.failed) return { text: `${title}: failed`, assertive: true };
  if (!active) return null;
  if (!prev.running && next.running && !next.quiet) return { text: `${title}: working`, assertive: false };
  if (prev.running && !next.running && !prev.quiet && !next.failed && !next.request && !next.continues) {
    return { text: `${title}: finished`, assertive: false };
  }
  return null;
}

/** A thread's name as the reader hears it: an untitled thread is the sidebar's "New thread". */
export function spokenTitle(...titles: (string | undefined)[]): string {
  for (const title of titles) {
    const trimmed = title?.trim();
    if (trimmed) return trimmed;
  }
  return "New thread";
}

/** The status the window's title gives the open thread, or undefined when it has none to say. */
export function windowStatus(moment: ThreadMoment, listed: string | undefined): string | undefined {
  if (moment.request) return moment.request.question ? "Asks a question" : "Needs approval";
  if (moment.running) return "Working";
  if (moment.failed) return "Failed";
  return listed;
}

/** The window's title: "<thread> — <status> — RustCode", or the app's name alone with none open. */
export function windowTitle(thread: string | undefined, status?: string): string {
  return [thread, status, "RustCode"].filter(Boolean).join(" — ");
}

/** Sets the window's title, in the page and in the native window a screen reader reads. */
export function applyWindowTitle(title: string): void {
  if (typeof document === "undefined") return;
  document.title = title;
  if (!isTauri()) return;
  void import("@tauri-apps/api/window")
    .then(({ getCurrentWindow }) => getCurrentWindow().setTitle(title))
    .catch(() => {
      // The page's title still says it.
    });
}
