// What the user changed about a paired machine's threads while that machine was away: settling one
// (or marking it for review, or back to active) and pinning one. The change shows at once from the
// copy this computer keeps (`state/machineCache.ts`), waits here, in app storage so it outlives a
// restart, and is sent to the machine in the order it was made once the machine lists its threads
// again (`state/store.ts` `flushMachineOutbox`, docs/desktop.md › Several machines).
//
// One change per thread and kind: a second settle or unsettle of the same thread replaces the
// first, and one that puts it back the way the machine had it leaves nothing to send. Sending is
// idempotent: a change the machine already has is dropped, not sent again.
//
// When the machine changed the same thing itself meanwhile, the newer change wins: the machine's
// when its thread was active there after the user's change (its `updatedAt` is later), the user's
// otherwise (`resolveChange`).

import { create } from "zustand";
import type { SessionSummary } from "../acp/types";
import type { ThreadStatus } from "../acp/v2";
import type { CacheStorage } from "./machineCache";
import { splitThreadKey } from "./threadKeys";

/** Where the changes live in app storage: `{ ‹machine id›: Change[] }`. */
export const OUTBOX_KEY = "rustcode.machineOutbox";

/** A status the user can store on a thread. */
export type StoredStatus = "idle" | "review" | "done";

/** One change to a thread on a machine that was away when it was made. */
export type OutboxChange =
  | { kind: "status"; key: string; value: StoredStatus; base: StoredStatus; at: number }
  | { kind: "pin"; key: string; value: boolean; base: boolean; at: number };

let backing: CacheStorage | null | undefined;

function storage(): CacheStorage | null {
  if (backing !== undefined) return backing;
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

const isObject = (value: unknown): value is Record<string, unknown> =>
  !!value && typeof value === "object" && !Array.isArray(value);

const STORED: readonly string[] = ["idle", "review", "done"];

function parseChange(value: unknown): OutboxChange | null {
  if (!isObject(value) || typeof value.key !== "string" || typeof value.at !== "number") return null;
  if (
    value.kind === "status" &&
    STORED.includes(value.value as string) &&
    STORED.includes(value.base as string)
  ) {
    return {
      kind: "status",
      key: value.key,
      value: value.value as StoredStatus,
      base: value.base as StoredStatus,
      at: value.at,
    };
  }
  if (value.kind === "pin" && typeof value.value === "boolean" && typeof value.base === "boolean") {
    return { kind: "pin", key: value.key, value: value.value, base: value.base, at: value.at };
  }
  return null;
}

function load(): Record<string, OutboxChange[]> {
  try {
    const raw = storage()?.getItem(OUTBOX_KEY);
    const value = raw ? (JSON.parse(raw) as unknown) : null;
    if (!isObject(value)) return {};
    const byHost: Record<string, OutboxChange[]> = {};
    for (const [host, list] of Object.entries(value)) {
      if (!Array.isArray(list)) continue;
      const changes = list.map(parseChange).filter((change): change is OutboxChange => !!change);
      if (changes.length) byHost[host] = changes;
    }
    return byHost;
  } catch {
    return {};
  }
}

function save(byHost: Record<string, OutboxChange[]>): void {
  try {
    const target = storage();
    if (!target) return;
    if (Object.keys(byHost).length) target.setItem(OUTBOX_KEY, JSON.stringify(byHost));
    else target.removeItem(OUTBOX_KEY);
  } catch {
    // Storage full or unavailable: the change still waits in memory until the app quits.
  }
}

/** The waiting changes, by machine id, in the order they were made. */
export const useOutbox = create<{ byHost: Record<string, OutboxChange[]> }>(() => ({ byHost: load() }));

function commit(byHost: Record<string, OutboxChange[]>): void {
  const kept = Object.fromEntries(Object.entries(byHost).filter(([, list]) => list.length));
  useOutbox.setState({ byHost: kept });
  save(kept);
}

/** Tests only: keep the outbox in this storage (or none) instead of the web view's, and read it. */
export function useOutboxStorage(next: CacheStorage | null | undefined): void {
  backing = next;
  useOutbox.setState({ byHost: load() });
}

/** A thread's status as it can be stored: running and waiting on the user are the machine's now. */
export function storedStatus(status: ThreadStatus | undefined): StoredStatus {
  return status === "done" || status === "review" ? status : "idle";
}

/**
 * Keep a change to send when its machine is back. It replaces a waiting change of the same kind to
 * the same thread, keeping that one's base and place; a change back to the base cancels it.
 */
export function queueChange(change: OutboxChange): void {
  const host = splitThreadKey(change.key).host;
  if (!host) return;
  const byHost = useOutbox.getState().byHost;
  const list = byHost[host] ?? [];
  const index = list.findIndex((c) => c.key === change.key && c.kind === change.kind);
  const merged = (index >= 0 ? { ...change, base: list[index]!.base } : change) as OutboxChange;
  const next =
    merged.value === merged.base
      ? list.filter((_, i) => i !== index)
      : index >= 0
        ? list.map((c, i) => (i === index ? merged : c))
        : [...list, merged];
  commit({ ...byHost, [host]: next });
}

/** Take one change out, once it was sent or dropped. A newer change to the same thread stays. */
export function removeChange(change: OutboxChange): void {
  const host = splitThreadKey(change.key).host;
  const byHost = useOutbox.getState().byHost;
  const list = byHost[host];
  if (!list?.includes(change)) return;
  commit({ ...byHost, [host]: list.filter((c) => c !== change) });
}

/** The changes waiting for one machine. */
export function changesFor(host: string): OutboxChange[] {
  return useOutbox.getState().byHost[host] ?? [];
}

/** The status waiting to be sent for a thread, if any. */
export function pendingStatus(key: string): StoredStatus | undefined {
  const host = splitThreadKey(key).host;
  if (!host) return undefined;
  const list = useOutbox.getState().byHost[host];
  return list?.find(
    (c): c is Extract<OutboxChange, { kind: "status" }> => c.kind === "status" && c.key === key,
  )?.value;
}

/** Whether a thread has a change waiting for its machine. */
export function hasPendingChange(byHost: Record<string, OutboxChange[]>, key: string): boolean {
  const host = splitThreadKey(key).host;
  return !!host && !!byHost[host]?.some((c) => c.key === key);
}

/** Drop everything waiting for a machine (it was removed: there is nowhere to send it). */
export function forgetMachineOutbox(host: string): void {
  const byHost = useOutbox.getState().byHost;
  if (!byHost[host]) return;
  const { [host]: _gone, ...rest } = byHost;
  commit(rest);
}

/** Drop what waits for one thread (it was deleted). */
export function forgetThreadOutbox(key: string): void {
  const host = splitThreadKey(key).host;
  const list = useOutbox.getState().byHost[host];
  if (!list?.some((c) => c.key === key)) return;
  commit({ ...useOutbox.getState().byHost, [host]: list.filter((c) => c.key !== key) });
}

/**
 * What to do with a waiting change, given the machine's own row for the thread now:
 *  - `send`: the machine still has what the user saw, or changed it before the user did;
 *  - `done`: the machine already has the change;
 *  - `machine`: the machine changed it after the user did (the thread was active there later),
 *    so the machine's newer state stands;
 *  - `gone`: the machine no longer lists the thread.
 */
export function resolveChange(
  change: OutboxChange,
  row: SessionSummary | undefined,
): "send" | "done" | "machine" | "gone" {
  if (!row) return "gone";
  const now = change.kind === "status" ? storedStatus(row.status) : !!row.pinned;
  // A thread running or waiting there is the machine's newest state whatever the stored one was.
  const live = change.kind === "status" && (row.status === "running" || row.status === "needs_input");
  if (now === change.value && !live) return "done";
  const changedThere = live || now !== change.base;
  if (changedThere && (row.updatedAt ?? 0) > change.at) return "machine";
  return "send";
}
