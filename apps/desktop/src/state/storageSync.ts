/**
 * Stored values a module keeps in memory, read again when another window writes them. A module
 * registers its key here, and `windowSync.ts` hands it every `storage` event for that key. Kept
 * free of imports, so a module low in the graph can register without a cycle.
 */

const handlers = new Map<string, (value: string | null) => void>();

/** Read `key` again with `apply` whenever another window changes it. */
export function syncStorageKey(key: string, apply: (value: string | null) => void): void {
  handlers.set(key, apply);
}

/** Apply another window's change to a registered key. Returns whether one was registered. */
export function applySyncedStorage(key: string, value: string | null): boolean {
  const apply = handlers.get(key);
  if (!apply) return false;
  apply(value);
  return true;
}
