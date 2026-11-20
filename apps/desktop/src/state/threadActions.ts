import { isLoaded } from "./thread";
// Thread actions: confirming, retrying and editing a message, the inbox, pins and sidebar order,
// archiving, renaming and trashing a thread. Split out of state/store.ts; the state these write still
// lives in the app store.

import { type AcpConnection, obj, RpcError, str } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { V2Method } from "../acp/v2";
import { trashName } from "../lib/platform";
import { shellCall } from "../lib/shell";
import { forgetAutoCompact } from "./autoCompactRun";
import { type Attachment, emptyExtras, type SentAttachment, splitImageMarkers } from "./compose";
import { releaseLimitWaitsFrom, stopLinksFrom, wakeTick } from "./host";
import type { InboxItem } from "./inbox";
import { dismissRows, restoreRows } from "./inboxDismissed";
import { forgetThreadOutbox, queueChange } from "./machineOutbox";
import { forget } from "./orchestration";
import { keepThreads, rememberOrder, type SidebarOrder, withoutThreads } from "./sidebarOrder";
import {
  type AppState,
  applySnooze,
  clearFinished,
  closeThread,
  connectionForThread,
  errorText,
  keptUnlistedThreads,
  machineOfThread,
  openThread,
  runPrompt,
  STORAGE,
  selectThread,
  setExtras,
  startNewThread,
  stopNow,
  store,
  submit,
  syncNote,
  threadIsRemote,
  toast,
  tombstoneThread,
  updateThread,
  useApp,
  without,
} from "./store";
import { isRunning, localId, messagesAfter, type ThreadState, truncateBefore, userIndexOf } from "./thread";
import { isRemoteThread, splitThreadKey } from "./threadKeys";
import { clearStatusMark } from "./threadStatus";

// Read through `useApp` at call time: store.ts imports this module, so `useApp` is not defined yet
// while this one loads.
const get = () => useApp.getState();
const set = (patch: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => useApp.setState(patch);

export function askConfirm(confirm: NonNullable<AppState["confirm"]>) {
  set({ confirm });
}

export function closeConfirm() {
  set({ confirm: null });
}

/** Whether the thread can be taken back to one of its messages. */
export function canRewind(thread: ThreadState): boolean {
  return !!get().agent?.rewind && (thread.harness?.capabilities.rewind ?? true) && !isRunning(thread);
}

export async function rewindTo(id: string, itemId: string, text: string): Promise<boolean> {
  const thread = get().threads[id];
  if (!thread) return false;
  const userIndex = userIndexOf(thread, itemId);
  if (userIndex < 0) return false;
  try {
    const conn = connectionForThread(id);
    if (!conn) throw new Error("That computer is not connected.");
    await conn.request(Method.rewind, { sessionId: splitThreadKey(id).id, userIndex, text }, 10_000);
  } catch (error) {
    toast(`Could not rewind: ${errorText(error)}`, "error");
    return false;
  }
  updateThread(id, (t) => truncateBefore(t, itemId));
  return true;
}

/** Retry: the thread goes back to just before the message and sends it again (after a confirm when later messages go too). */
export function retryMessage(id: string, itemId: string) {
  const thread = get().threads[id];
  const item = thread?.items.find((i) => i.id === itemId);
  if (!thread || item?.kind !== "user") return;
  const extras = { ...emptyExtras, attachments: pastesOf(item.attachments, item.text) };
  const go = async () => {
    if (canRewind(thread)) {
      if (await rewindTo(id, itemId, item.text)) void runPrompt(id, item.text, extras);
    } else void submit(item.text, extras);
  };
  const later = messagesAfter(thread, itemId);
  if (later > 0 && canRewind(thread)) {
    askConfirm({
      title: "Retry this message?",
      message:
        later === 1
          ? "The message after it and its reply will be removed."
          : `The ${later} messages after it and their replies will be removed.`,
      action: "Retry",
      destructive: true,
      run: () => void go(),
    });
  } else void go();
}

/**
 * Put rows away without answering them: a failure nobody will retry, a request the
 * user would rather answer in the thread itself. Only the inbox row and its count
 * change — a failed turn still shows in the transcript and a request still waits in
 * its chat — and the row comes back if the thread asks again (state/inboxDismissed.ts).
 * One Undo toast puts back every row of this call.
 */
export function dismissInbox(items: readonly InboxItem[]): void {
  if (!items.length) return;
  const keys = items.map((item) => item.key);
  const next = dismissRows(get().inboxDismissed, items);
  set({ inboxDismissed: next });
  store(STORAGE.inboxDismissed, JSON.stringify(next));
  const title = items.length === 1 ? items[0]!.title : "";
  toast(items.length === 1 ? `Dismissed "${title}"` : `Dismissed ${items.length} items`, "info", {
    id: items.length === 1 ? `inbox-dismiss:${keys[0]}` : "inbox-dismiss",
    action: { label: "Undo", run: () => restoreInbox(keys) },
  });
}

/** Undo: the rows go back in the inbox exactly as they were. */
export function restoreInbox(keys: readonly string[]): void {
  const before = get().inboxDismissed;
  const next = restoreRows(before, keys);
  if (next === before) return;
  set({ inboxDismissed: next });
  store(STORAGE.inboxDismissed, JSON.stringify(next));
}

/** Retry a thread's failed turn: the inbox's action for a failure. The thread is
 * opened first when the app has not read it, because the message to send again
 * lives in its transcript; its last message is the one the failure answered.
 */
export async function retryThread(id: string): Promise<void> {
  if (!isLoaded(get().threads[id])) await openThread(id);
  const thread = get().threads[id];
  if (!thread) return;
  for (let i = thread.items.length - 1; i >= 0; i--) {
    const item = thread.items[i]!;
    if (item.kind !== "user") continue;
    // Retried: the failure is answered, so the inbox and the row stop saying Failed.
    clearFinished(id);
    retryMessage(id, item.id);
    return;
  }
  // Nothing to send again (an empty transcript): at least show the thread.
  selectThread(id);
}

/**
 * The pastes a sent message kept, back in a message that is sent again: a retry or an edit must
 * carry the log the message was about, or the model is asked about text it cannot see. Only a
 * message sent before pastes went into the text has these; one written since carries the text
 * itself, so `at` puts them back at its end, where the chip used to send them.
 */
function pastesOf(attachments: SentAttachment[] | undefined, text: string): Attachment[] {
  return (attachments ?? []).flatMap((a) =>
    a.kind === "text" && a.text
      ? [{ id: localId("paste"), kind: "text" as const, text: a.text, at: text.length }]
      : [],
  );
}

/** Edit: the message goes back to the composer; sending it rewinds the thread to it first. */
export function editMessage(id: string, itemId: string) {
  const thread = get().threads[id];
  const item = thread?.items.find((i) => i.id === itemId);
  if (!thread || item?.kind !== "user") return;
  const text = splitImageMarkers(item.text).text;
  // What the message held past the text comes back with it, as the chips it was written as. The
  // composer's own paste chips go: the draft is this message now, not the one edited before it.
  const pastes = pastesOf(item.attachments, text);
  setExtras(id, (e) => ({
    ...e,
    attachments: [...e.attachments.filter((a) => a.kind !== "text"), ...pastes],
  }));
  set((s) => ({
    drafts: { ...s.drafts, [id]: text },
    editing: canRewind(thread)
      ? { ...s.editing, [id]: { itemId, userIndex: userIndexOf(thread, itemId), original: item.text } }
      : s.editing,
  }));
}

export function cancelEdit(key: string) {
  set((s) => {
    const { [key]: _gone, ...editing } = s.editing;
    return { editing };
  });
}

/**
 * Pin or unpin threads: the app's list first, so the sidebar answers at once, then the engine, one
 * request each. A failure puts back only the threads the engine refused, and says so once for the
 * whole selection rather than once per thread (ui/Sidebar.tsx acts on a multi-selection).
 */
export function setPinned(ids: string[], on: boolean) {
  const before = get().pinned;
  const wanted = new Set(ids);
  const next = on
    ? [...ids.filter((id) => !before.includes(id)), ...before]
    : before.filter((id) => !wanted.has(id));
  set({ pinned: next });
  store(STORAGE.pinned, JSON.stringify(next));
  const many = ids.length > 1;
  if (!get().features.pins) return;
  // A pin is kept by the computer the thread runs on: a machine's threads go over that machine's
  // connection, with the machine's own id (`state/threadKeys.ts`). A local thread with no engine
  // keeps the app's pin, as before.
  const groups = new Map<AcpConnection, string[]>();
  const unreachable: string[] = [];
  const queued: string[] = [];
  for (const id of ids) {
    const conn = connectionForThread(id);
    if (conn) groups.set(conn, [...(groups.get(conn) ?? []), id]);
    // A paired machine that is away gets the pin when it is back (`state/machineOutbox.ts`).
    else if (threadIsRemote(id) && machineOfThread(id)) {
      queueChange({ kind: "pin", key: id, value: on, base: before.includes(id), at: Date.now() });
      queued.push(id);
    } else if (threadIsRemote(id)) unreachable.push(id);
  }
  if (queued.length) toast(`${on ? "Pinned" : "Unpinned"}.${syncNote(queued)}`, "info");
  if (!groups.size && !unreachable.length) return;
  type PinResult = { id: string; ok: true } | { id: string; ok: false; error: unknown };
  void Promise.all<PinResult>([
    ...[...groups].flatMap(([conn, group]) =>
      group.map((id) =>
        conn.request(V2Method.sessionPin, { session_id: splitThreadKey(id).id, pinned: on }, 10_000).then(
          (): PinResult => ({ id, ok: true }),
          (error: unknown): PinResult =>
            error instanceof RpcError && error.isMethodNotFound ? { id, ok: true } : { id, ok: false, error },
        ),
      ),
    ),
    ...unreachable.map(
      (id): PinResult => ({ id, ok: false, error: new Error("That computer is not connected.") }),
    ),
  ]).then((results) => {
    const failed = results.filter((r): r is { id: string; ok: false; error: unknown } => !r.ok);
    if (!failed.length) return;
    const refused = new Set(failed.map((f) => f.id));
    // Unpinning: keep what was pinned before, less the ones the engine took. Pinning: drop the
    // ones it refused and leave the rest.
    const pinned = on
      ? get().pinned.filter((id) => !refused.has(id))
      : before.filter((id) => !wanted.has(id) || refused.has(id));
    set({ pinned });
    store(STORAGE.pinned, JSON.stringify(pinned));
    const first = errorText(failed[0]!.error);
    toast(
      many
        ? `Could not ${on ? "pin" : "unpin"} ${failed.length} of ${ids.length} threads: ${first}`
        : `Could not ${on ? "pin" : "unpin"} the thread: ${first}`,
      "error",
    );
  });
}

export function togglePin(id: string) {
  setPinned([id], !get().pinned.includes(id));
}

/**
 * Arrange one sidebar section (`state/sidebarOrder.ts`): the ids in the order the user left them,
 * kept in app storage beside `pinned`, so a drag or ⌥↑/↓ outlives the window.
 */
export function setSectionOrder(key: string, ids: string[]) {
  const sidebarOrder = { ...get().sidebarOrder, [key]: ids };
  set({ sidebarOrder });
  store(STORAGE.sidebarOrder, JSON.stringify(sidebarOrder));
}

/** Remember newly seen rows while retaining positions for currently hidden rows. */
export function rememberSidebarOrder(seen: SidebarOrder) {
  const before = get().sidebarOrder;
  let next = before;
  for (const [key, ids] of Object.entries(seen)) {
    const order = rememberOrder(before[key], ids, key !== "projects");
    if (!order.length || order === before[key]) continue;
    if (next === before) next = { ...before };
    next[key] = order;
  }
  saveSidebarOrder(next, before);
}

/** The arrangement, written only when it changed (`state/sidebarOrder.ts`). */
function saveSidebarOrder(next: SidebarOrder, before: SidebarOrder) {
  if (next === before) return;
  set({ sidebarOrder: next });
  store(STORAGE.sidebarOrder, JSON.stringify(next));
}

/**
 * A thread that is gone keeps no place in any section's arrangement: without this a trashed thread
 * would leave its id in app storage for good. A no-op when it had no place.
 */
export function forgetSidebarOrder(ids: readonly string[]) {
  saveSidebarOrder(withoutThreads(get().sidebarOrder, ids), get().sidebarOrder);
}

/**
 * The arrangement limited to the threads the engine still lists: a thread trashed from another app,
 * or before this field existed, loses its place here. Callers pass the whole list — a capped one is
 * not the whole list and prunes nothing (state/store.ts `refreshSessions`).
 */
export function pruneSidebarOrder(known: readonly string[]) {
  const ids = new Set(known);
  saveSidebarOrder(
    keepThreads(get().sidebarOrder, (id) => isRemoteThread(id) || ids.has(id)),
    get().sidebarOrder,
  );
}

export function setArchived(id: string, on: boolean) {
  const archived = { ...get().archived };
  if (on) {
    if (isRunning(get().threads[id])) {
      toast("Stop the running turn before archiving this thread.", "info");
      return;
    }
    archived[id] = Date.now();
  } else delete archived[id];
  set({ archived });
  store(STORAGE.archived, JSON.stringify(archived));
  // Delegates waiting out a usage limit for this thread stop waiting for it.
  if (on) releaseLimitWaitsFrom(id);

  if (on && get().selectedId === id) startNewThread();
}

/**
 * Renames a thread. `user` is a name the user typed: the thread records that they chose it, and an
 * agent naming its own thread with `update_session` leaves it alone (issue 3599).
 */
export async function renameThread(
  id: string,
  title: string,
  options: { user?: boolean } = {},
): Promise<boolean> {
  // A thread's name lives on the computer that owns it, and renaming one is that computer's
  // folder (`thread_rename` is this shell's): another machine's thread is not renamed from here.
  if (threadIsRemote(id)) {
    toast("A thread on another computer is renamed on that computer.", "info");
    return false;
  }
  try {
    const saved = await shellCall<string>("thread_rename", { id, title, user: !!options.user });
    set((s) => ({ sessions: s.sessions.map((x) => (x.id === id ? { ...x, title: saved } : x)) }));
    updateThread(id, (t) => ({ ...t, title: saved }));
    return true;
  } catch (error) {
    toast(`Could not rename the thread: ${errorText(error)}`, "error");
    return false;
  }
}

/** Take a thread out of every list, mark and stored place a trashed thread leaves behind. */
function forgetThreadRow(id: string): void {
  keptUnlistedThreads.delete(id);
  forgetThreadOutbox(id);
  tombstoneThread(id);
  const { [id]: _gone, ...threads } = get().threads;
  const archived = { ...get().archived };
  delete archived[id];
  const pinned = get().pinned.filter((x) => x !== id);
  const autoCompactOff = { ...get().autoCompactOff };
  delete autoCompactOff[id];
  forgetAutoCompact(id);
  applySnooze(id, null);
  clearStatusMark(id);
  set((s) => {
    const peeking = s.peekId === id;
    return {
      threads,
      archived,
      pinned,
      autoCompactOff,
      sessions: s.sessions.filter((x) => x.id !== id),
      links: forget(s.links, id),
      finished: without(s.finished, id),
      failed: without(s.failed, id),
      // A thread in the panel's Thread tab that no longer exists is not shown,
      // and the panel does not stay open on the tab it was in.
      peekId: peeking ? null : s.peekId,
      ...(peeking && s.panel === "peek" ? { panel: null } : {}),
    };
  });
  // Its place in the sidebar's arrangement goes with it, or the stored order keeps an id forever.
  forgetSidebarOrder([id]);
  store(STORAGE.archived, JSON.stringify(archived));
  store(STORAGE.pinned, JSON.stringify(pinned));
  store(STORAGE.autoCompactOff, JSON.stringify(autoCompactOff));
}

/**
 * A thread trashed by another client on this engine (`_rustcode/threads/deleted`): the app on the
 * machine and the app that started the thread there see the row go together. The machine's own
 * list cache drops it too (`state/machines.ts`).
 */
export function threadDeleted(params: unknown) {
  const key = str(obj(params)?.sessionId);
  if (!key) return;
  // Links into the trashed thread failed: their parents may now have something to hear.
  forgetThreadRow(key);
  wakeTick();
  if (get().selectedId === key) startNewThread();
}

/** Move a thread's folder to the Trash, closing it in the engine first, as the SwiftUI app does. */
export async function trashThread(id: string): Promise<boolean> {
  // A thread that still runs is stopped first, forced when it will not stop, so moving it to the
  // Trash works in every state (issue 3652).
  if (isRunning(get().threads[id]) && !(await stopNow(id))) {
    toast(`Could not stop the running turn before moving this thread to the ${trashName()}.`, "error");
    return false;
  }
  // Its delegates stop with it: forgetting their links would free slots they still hold.
  await stopLinksFrom(id);
  if (get().threads[id] && get().agent?.closeSession) await closeThread(id);
  try {
    await shellCall("thread_trash", { id });
  } catch (error) {
    toast(errorText(error), "error");
    return false;
  }
  // Every other client on this engine drops the row; an older engine has no such request.
  try {
    await connectionForThread(id)?.request(Method.threadDelete, { sessionId: splitThreadKey(id).id }, 10_000);
  } catch {
    // The folder is gone either way.
  }
  forgetThreadRow(id);
  // A machine whose link is down never heard of it: its list cache drops the row here.
  if (threadIsRemote(id)) {
    const { host, id: raw } = splitThreadKey(id);
    void import("./machines").then((m) => m.forgetMachineThread(host, raw));
  }
  // Links into the trashed thread failed: their parents may now have something to hear.
  wakeTick();
  if (get().selectedId === id) startNewThread();
  return true;
}
