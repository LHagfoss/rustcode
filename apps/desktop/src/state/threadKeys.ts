// A thread's key: its own id on the computer it runs on, and the machine's
// id in front of it when that is not this one. Local threads keep their bare
// id, so everything that knew one thread before the machine network is
// unchanged; a thread on a paired machine reads `gw-…/‹id›`, which is what
// the sidebar, the selection and the store's `threads` record key by
// (docs/desktop.md › Several machines).
//
// The raw id on the machine is what its ACP calls take (`session/load`,
// `session/prompt`), so `splitThreadKey` is applied at the connection.

/** The machine this app runs on: no host prefix at all. */
export const LOCAL_HOST = "";

/** A thread's key, as the store and the UI know it. */
export function threadKey(host: string, id: string): string {
  return host ? `${host}/${id}` : id;
}

/** The machine a thread key names, and the thread's id on it. */
export function splitThreadKey(key: string): { host: string; id: string } {
  const at = key.indexOf("/");
  if (at < 0) return { host: LOCAL_HOST, id: key };
  return { host: key.slice(0, at), id: key.slice(at + 1) };
}

/** Whether the thread runs on another computer. */
export function isRemoteThread(key: string): boolean {
  return key.includes("/");
}
