// Forks and side chats: threads the user opened from another thread, as opposed to the
// delegates an agent started (state/lineage.ts). The engine records the relationship in the
// child's own metadata (`_meta.rustcode.parent`), so what is derived here survives a reload
// and needs no row stored in the parent's transcript. Pure, so the sidebar, the header, the
// details card and the transcript's lifecycle rows share one answer.

import { relationshipOf, type SessionSummary, type ThreadParent } from "../acp/types";

/** The relationships the user opens a thread with from another one. */
export type BranchKind = "fork" | "side_chat";

/** A thread opened from another. */
export interface Branch {
  id: string;
  title: string;
  kind: BranchKind;
}

/** The thread a fork or side chat came from; absent for any other thread. */
export interface Origin {
  /** The parent's id, when the record has one. */
  id?: string;
  /** The parent as it is now when it is listed, else as it was named when this was opened. */
  title: string;
  kind: BranchKind;
  /** The parent is still a thread the app lists, so it can be opened. */
  listed: boolean;
}

/** Whether a recorded parent makes its thread a fork or a side chat; a delegate is neither. */
export const branchKindOf = (parent: ThreadParent): BranchKind | undefined => {
  const relationship = relationshipOf(parent);
  return relationship === "fork" || relationship === "side_chat" ? relationship : undefined;
};

const branchKind = (session: SessionSummary): BranchKind | undefined =>
  session.parent ? branchKindOf(session.parent) : undefined;

/** Whether `session` is a fork or a side chat (and so not a delegate). */
export const isBranch = (session: SessionSummary): boolean => branchKind(session) !== undefined;

/** The forks and side chats opened from `parentId`, in a stable order (by id, as the sidebar's stacks). */
export function branchesOf(sessions: readonly SessionSummary[], parentId: string): Branch[] {
  const out: Branch[] = [];
  for (const session of sessions) {
    const kind = branchKind(session);
    if (kind && session.parent?.id === parentId && session.id !== parentId) {
      out.push({ id: session.id, title: session.title?.trim() || "New thread", kind });
    }
  }
  return out.sort((a, b) => a.id.localeCompare(b.id));
}

/** Where a fork or side chat came from, else nothing. */
export function originOf(sessions: readonly SessionSummary[], id: string): Origin | undefined {
  const session = sessions.find((x) => x.id === id);
  const kind = session && branchKind(session);
  if (!session?.parent || !kind) return undefined;
  const listed = sessions.find((x) => x.id === session.parent?.id);
  return {
    id: session.parent.id,
    title: listed?.title?.trim() || session.parent.title?.trim() || "another thread",
    kind,
    listed: !!listed,
  };
}

/** The lifecycle row a parent's transcript draws for one of its branches. */
export function branchLine(branch: Pick<Branch, "kind" | "title">): { label: string; title: string } {
  return { label: branch.kind === "fork" ? "Forked to" : "Side chat", title: branch.title };
}

/** The header's line on a fork or side chat, naming where it came from. */
export function originLine(origin: Pick<Origin, "kind" | "title">): { label: string; title: string } {
  return { label: origin.kind === "fork" ? "Forked from" : "Side chat on", title: origin.title };
}
