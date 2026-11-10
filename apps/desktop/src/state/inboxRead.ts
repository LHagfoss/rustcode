// The hooks the app reads the inbox and the away recap with, kept out of the
// pure modules so their derivation and tests stay free of React and the store.
// Both read the store through compact signatures: the threads object changes on
// every streamed chunk, and none of those chunks changes a row.

import { useMemo } from "react";
import { deriveInbox, type InboxItem } from "./inbox";
import { type Recap, recapOf } from "./recap";
import { useApp } from "./store";

/** Titles and requests of every loaded thread: the one string the inbox changes with. */
function threadSignature(
  threads: Record<
    string,
    { title?: string; cwd?: string; permissions: readonly { rpcId: number | string; at?: number }[] }
  >,
): string {
  return Object.values(threads)
    .map(
      (t) =>
        `${t.title ?? ""}|${t.cwd ?? ""}|${t.permissions.map((p) => `${p.rpcId}@${p.at ?? 0}`).join(",")}`,
    )
    .join("\u0000");
}

/** Every pending request, failed turn and delegate across the app, newest first. */
export function useInboxItems(): InboxItem[] {
  const sessions = useApp((s) => s.sessions);
  const failed = useApp((s) => s.failed);
  const links = useApp((s) => s.links.links);
  const archived = useApp((s) => s.archived);
  const dismissed = useApp((s) => s.inboxDismissed);
  const snoozed = useApp((s) => s.snoozed);
  const signature = useApp((s) => threadSignature(s.threads));
  // biome-ignore lint/correctness/useExhaustiveDependencies: the signature stands for the threads it was built from
  return useMemo(
    () =>
      deriveInbox({
        sessions,
        failed,
        links,
        archived,
        dismissed,
        snoozed,
        threads: useApp.getState().threads,
      }),
    [sessions, failed, links, archived, dismissed, snoozed, signature],
  );
}

/**
 * What happened while the user was away, or null when there is no recap to
 * show: none was started, or it was dismissed, or all three groups are empty.
 */
export function useRecap(): Recap | null {
  const items = useInboxItems();
  const since = useApp((s) => s.recapSince);
  const dismissed = useApp((s) => s.recapDismissed);
  const finished = useApp((s) => s.finished);
  const sessions = useApp((s) => s.sessions);
  const snoozed = useApp((s) => s.snoozed);
  return useMemo(() => {
    if (since === null || dismissed) return null;
    return recapOf({ items, finished, sessions, threads: {}, since, snoozed });
  }, [items, since, dismissed, finished, sessions, snoozed]);
}
