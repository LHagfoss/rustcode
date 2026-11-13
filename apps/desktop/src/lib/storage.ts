// App storage (the web view's localStorage) through one pair of calls. Storage can be missing or
// refuse a write (a private window, a full quota, a test with no DOM): a read is then null and a
// write says it was not kept, so no caller needs its own try/catch around it.

/** The stored text for `key`, or null when it was never set or storage cannot be read. */
export function readStorage(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

/** Keeps `value` under `key`; null removes it. False when storage did not take it. */
export function writeStorage(key: string, value: string | null): boolean {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
    return true;
  } catch {
    return false;
  }
}

/** A boolean kept as "1"/"0", or `fallback` when it was never set. */
export function readFlag(key: string, fallback: boolean): boolean {
  const raw = readStorage(key);
  return raw === null ? fallback : raw === "1";
}

export function writeFlag(key: string, on: boolean): boolean {
  return writeStorage(key, on ? "1" : "0");
}
