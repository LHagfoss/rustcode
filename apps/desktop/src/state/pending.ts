// What the threads below a thread wait on: an approval or a question in a child,
// a grandchild or a thread further down is shown in the parent's transcript too,
// so the user does not have to find the child first. The decision is pure, so
// the transcript and its tests read it the same way; answering still goes
// through the card the child's own chat uses (state/store.ts `reply`).

import { useEffect, useMemo } from "react";
import type { PermissionRequest } from "../acp/types";
import type { Link, LinkState } from "./orchestration";
import { openThread, useApp } from "./store";

/** A pending request from a thread below the one showing it. */
export interface ChildRequest {
  /** The thread that asks. */
  thread: string;
  /** Its title, for the card's header. */
  title: string;
  /** The request itself, as its own thread's card shows it. */
  request: PermissionRequest;
  /** When it was asked, for oldest first (the request's arrival, else the link's start). */
  at: number;
  /** How many approvals the asking thread waits on, for `1 of 2` and Allow all. */
  pending: number;
  /** Which of those approvals this is, from 1. */
  number: number;
}

/** What the decision reads of the app. */
export interface PendingRead {
  /** The thread whose transcript shows them. */
  id: string;
  /** Every link in the book. */
  links: readonly Link[];
  /** A thread's pending requests, when the app holds its state. */
  requests: (threadId: string) => readonly PermissionRequest[] | undefined;
  /** A thread's title, when the app holds it. */
  title: (threadId: string) => string | undefined;
}

/** A link that no longer stands for work below it: stopped, or interrupted by a restart. */
const severed = (state: LinkState) => state === "cancelled" || state === "interrupted";

/**
 * The threads under `id`, each with the link it was reached through. A link still
 * in the book carries the thread: a settled one is included, because a middle
 * thread that ended its turn can leave a grandchild working (and asking) below
 * it. A link that was dismissed is gone from the book, and a stopped or
 * interrupted one no longer delegates, so neither carries requests up.
 */
function under(read: PendingRead): Map<string, Link> {
  const found = new Map<string, Link>();
  const seen = new Set([read.id]);
  let frontier = [read.id];
  while (frontier.length) {
    const next: string[] = [];
    for (const link of read.links) {
      if (severed(link.state) || !frontier.includes(link.orchestrator) || seen.has(link.target)) continue;
      seen.add(link.target);
      found.set(link.target, link);
      next.push(link.target);
    }
    frontier = next;
  }
  return found;
}

/**
 * The pending requests a thread shows from the threads below it, oldest first.
 * A thread further down appears once, named as itself (not as the link it came
 * through), so a grandchild's question is answerable from the top-level thread
 * without appearing twice. A request answered in the child leaves that child's
 * `permissions` at once, which is the one signal both chats read.
 */
export function pendingRequestsOn(read: PendingRead): ChildRequest[] {
  const below = under(read);
  const out: ChildRequest[] = [];
  for (const [threadId, link] of below) {
    const requests = read.requests(threadId);
    if (!requests?.length) continue;
    const title = read.title(threadId) ?? link.targetTitle;
    const pending = requests.filter((request) => !request.question).length;
    let number = 0;
    for (const request of requests) {
      if (!request.question) number++;
      out.push({
        thread: threadId,
        title,
        request,
        at: request.at ?? link.createdAt,
        pending: Math.max(1, pending),
        number: request.question ? 1 : number,
      });
    }
  }
  return out.sort((a, b) => a.at - b.at);
}

/**
 * The child requests a thread's transcript shows, read once for it. The store's
 * threads object changes with every streamed chunk, so the requests are read
 * through a compact signature: ids, arrival times and titles, never whole items.
 * A request whose thread is not loaded is loaded now, so it can be answered and
 * its answer recorded in that thread like any other.
 */
export function useChildRequests(threadId: string): ChildRequest[] {
  const links = useApp((s) => s.links.links);
  const signature = useApp((s) => {
    const parts: string[] = [];
    for (const thread of Object.values(s.threads)) {
      if (!thread.permissions.length) continue;
      const requests = thread.permissions.map((p) => `${p.rpcId}@${p.at ?? 0}`).join(",");
      parts.push(`${thread.id}|${thread.loaded ? 1 : 0}|${thread.title ?? ""}|${requests}`);
    }
    return parts.join("\u0000");
  });
  // biome-ignore lint/correctness/useExhaustiveDependencies: the signature stands for the requests and titles it was built from
  const requests = useMemo(() => {
    const s = useApp.getState();
    return pendingRequestsOn({
      id: threadId,
      links,
      requests: (id) => s.threads[id]?.permissions,
      title: (id) => s.threads[id]?.title,
    });
  }, [threadId, links, signature]);
  useEffect(() => {
    for (const entry of requests) {
      const thread = useApp.getState().threads[entry.thread];
      if (thread && !thread.loaded && !thread.loading) void openThread(entry.thread);
    }
  }, [requests]);
  return requests;
}
