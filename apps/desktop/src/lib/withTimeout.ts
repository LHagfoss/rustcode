// A bound on a wait the app cannot cancel: a native call or a connection that
// never answers must end in an error the page can show, not in a spinner that
// stays for good.

/** Ends with this when the wait ran out; the work behind it may still finish unseen. */
export class WaitTimeoutError extends Error {
  constructor(what: string, ms: number) {
    super(`${what} did not answer in ${Math.round(ms / 1000)} seconds.`);
    this.name = "WaitTimeoutError";
  }
}

/** `work`'s result, or a `WaitTimeoutError` once `ms` passed. The timer never outlives the work. */
export function withTimeout<T>(work: Promise<T>, ms: number, what: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new WaitTimeoutError(what, ms)), ms);
    work.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}
