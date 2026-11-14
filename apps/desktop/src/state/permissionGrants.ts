// The "Allow for This Thread" answers a thread keeps in the link book, in words, for
// Settings › Permissions › Remembered approvals. A grant key is whatever `permitted` in
// host.ts filed the answer under: a thread id for reading and messaging, `start`, `project:…`
// and so on, or a JSON delegate-account key. Pure, so the page and its tests share it.

import type { LinkBook } from "./orchestration";

export interface ThreadGrant {
  /** The thread the answer was given to. */
  caller: string;
  /** The key it was filed under, what a revoke names. */
  key: string;
  /** What it allows, in words: "Start threads in another project". */
  label: string;
}

/** What one grant key allows, in words; `titleOf` names a thread by its id. */
export function describeGrantKey(key: string, titleOf: (id: string) => string | undefined): string {
  if (key.startsWith("[")) {
    try {
      const [kind, machine, harness, account] = JSON.parse(key) as unknown[];
      if (kind === "delegate-account") {
        const who = typeof account === "string" && account ? `account “${account}”` : "the default sign-in";
        const where = machine && machine !== "local" ? " on another computer" : "";
        return `Delegate on ${String(harness)} with ${who}${where}`;
      }
    } catch {
      // Not JSON after all: described as it is below.
    }
  }
  const [head = "", ...rest] = key.split(":");
  const tail = rest.join(":");
  const thread = (id: string) => `“${titleOf(id) ?? id}”`;
  if (head === "start" || head.startsWith("start@")) {
    const machine = head.startsWith("start@") ? head.slice("start@".length) : "local";
    const where = machine && machine !== "local" ? " on another computer" : "";
    return `Start threads${where}${tail ? ` in ${tail}` : ""}`;
  }
  switch (head) {
    case "project":
      return tail.startsWith("clone") ? "Clone a project" : `Change the project ${tail}`;
    case "remove-project":
      return `Remove the project ${tail}`;
    case "schedule":
      return "Change scheduled work";
    case "cleanup":
      return `Clean up ${thread(tail)}`;
    case "handoff":
      return `Hand off ${thread(rest[0] ?? "")}`;
    case "organize":
      return "Change threads";
    case "interrupt":
      return `Stop ${thread(tail)}`;
    case "configure":
      return `Change the model of ${thread(tail)}`;
    case "transfer":
      return `Move ${thread(tail)} to another harness`;
    case "merge":
      return `Merge ${thread(rest[0] ?? "")}'s pull request`;
    case "answer":
      return `Answer what ${thread(rest[0] ?? "")} asks`;
  }
  return `Read and message ${thread(key)}`;
}

/** Every remembered answer in the book, newest thread first as the book lists them. */
export function threadGrants(book: LinkBook, titleOf: (id: string) => string | undefined): ThreadGrant[] {
  return Object.entries(book.grants).flatMap(([caller, keys]) =>
    keys.map((key) => ({ caller, key, label: describeGrantKey(key, titleOf) })),
  );
}

/** The book without one remembered answer; the thread is asked again next time. */
export function revokeThreadGrant(book: LinkBook, caller: string, key: string): LinkBook {
  const keys = book.grants[caller];
  if (!keys?.includes(key)) return book;
  const grants = { ...book.grants };
  const left = keys.filter((k) => k !== key);
  if (left.length) grants[caller] = left;
  else delete grants[caller];
  return { ...book, grants };
}
