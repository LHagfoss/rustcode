// A turn whose harness could not reach its provider because the machine had no
// network waits instead of dying. The wait is a schedule, not a timer of its own:
// while any thread waits, the app asks the webview whether the machine is online
// every few seconds (and at once when the network comes back or the window
// wakes), and sends "Please continue." once the connection has stayed up long
// enough. Each retry demands a longer stable period, so flaky Wi-Fi cannot burn
// them, and the wait gives up after a few hours and says so. Pure, so the store
// and its tests share one answer.

/** A wait with no network at all (ms), the unit the app thinks in. */
export const INTERNET_POLL_MS = 5_000;

/**
 * How long the connection must have stayed up before the next retry: 5 s, 30 s,
 * 2 m, 5 m. The last one holds for every retry after it.
 */
export const INTERNET_STABLE_MS = [5_000, 30_000, 120_000, 300_000] as const;

/** Past this, the thread stops waiting on its own and says so: a day of Wi-Fi is not coming. */
export const INTERNET_GIVE_UP_MS = 6 * 3_600_000;

/** What the thread is told when the connection is back. */
export const INTERNET_RESUME_TEXT = "The internet connection is back. Please continue.";

export interface InternetWait {
  /** When the turn failed and the wait began (ms); it does not move across retries. */
  since: number;
  /** How many retries have gone out; the stable period grows with it. */
  attempts: number;
  /** What the thread ran on when it stopped: another harness or login is not this wait. */
  harness?: string;
  account: string | null;
}

/** How long the connection must have been up for retry number `attempts + 1`. */
export function stablePeriodMs(attempts: number): number {
  const index = Math.min(Math.max(Math.trunc(attempts), 0), INTERNET_STABLE_MS.length - 1);
  return INTERNET_STABLE_MS[index]!;
}

/**
 * Retries a wait gets. A failure that comes back while the machine is online (a local model server
 * that is down answers "connection refused" too) is not a lost network, and must not be told to
 * continue every few minutes for six hours.
 */
export const INTERNET_MAX_ATTEMPTS = 6;

/** Whether the wait has run out of patience: the thread stays stopped and the app says so. */
export function internetGaveUp(wait: InternetWait, now: number): boolean {
  return now - wait.since >= INTERNET_GIVE_UP_MS || wait.attempts >= INTERNET_MAX_ATTEMPTS;
}

/** The wait a network failure starts, or the one it continues: the first failure's time stays. */
export function nextInternetWait(
  previous: InternetWait | undefined,
  now: number,
  on: { harness?: string; account: string | null },
): InternetWait {
  return previous
    ? { ...previous, harness: on.harness, account: on.account }
    : { since: now, attempts: 0, harness: on.harness, account: on.account };
}

/**
 * Whether the next retry is due: the connection has been up for the period this
 * wait's attempt count asks for, and the wait has not given up.
 * `upSince` is null while the machine is offline.
 */
export function internetDue(wait: InternetWait, upSince: number | null, now: number): boolean {
  if (upSince === null || internetGaveUp(wait, now)) return false;
  return now - upSince >= stablePeriodMs(wait.attempts);
}

/** Whether the thread still runs on what lost the network; on anything else it is not this wait. */
export function internetHolds(
  wait: InternetWait,
  now: { harness?: string; account: string | null },
): boolean {
  return wait.harness === now.harness && wait.account === now.account;
}
