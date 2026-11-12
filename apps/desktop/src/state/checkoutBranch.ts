import { useEffect, useState } from "react";
import { obj, str } from "../acp/jsonrpc";
import { Method, type SessionSummary } from "../acp/types";
import { connectionForThread } from "./store";

// Share reads between threads in one checkout, and bound concurrent git processes.
const cache = new Map<string, { at: number; value?: string; pending?: Promise<string | undefined> }>();
const queue: (() => void)[] = [];
let active = 0;
function run<T>(task: () => Promise<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    const start = () => {
      active++;
      void task()
        .then(resolve, reject)
        .finally(() => {
          active--;
          queue.shift()?.();
        });
    };
    if (active < 4) start();
    else queue.push(start);
  });
}

export function readCheckoutBranch(
  id: string,
  cwd: string,
  host: string | undefined,
): Promise<string | undefined> {
  const conn = connectionForThread(id);
  if (!conn || !cwd) return Promise.resolve(undefined);
  const key = JSON.stringify([host ?? "", cwd]);
  const previous = cache.get(key);
  if (previous?.pending) return previous.pending;
  if (previous && Date.now() - previous.at < 30_000) return Promise.resolve(previous.value);
  const pending = run(async () => {
    try {
      const response = await conn.request(Method.projectRead, { path: cwd }, 15_000);
      return str(obj(response)?.branch);
    } catch {
      return undefined;
    }
  }).then((value) => {
    cache.set(key, { at: Date.now(), value });
    return value;
  });
  cache.set(key, { at: 0, pending });
  return pending;
}

/** The current branch of a checkout, refreshed on focus and periodically. */
export function useCheckoutBranch(
  session: SessionSummary,
  connectionState: string | undefined,
  enabled: boolean,
): string | undefined {
  const [branch, setBranch] = useState<string>();
  const { id, cwd, host } = session;
  useEffect(() => {
    void connectionState;
    if (!enabled) return;
    let live = true;
    const refresh = () =>
      void readCheckoutBranch(id, cwd, host).then((value) => {
        if (live) setBranch(value);
      });
    refresh();
    window.addEventListener("focus", refresh);
    const timer = window.setInterval(refresh, 60_000);
    return () => {
      live = false;
      window.removeEventListener("focus", refresh);
      window.clearInterval(timer);
    };
  }, [id, cwd, host, connectionState, enabled]);
  return branch;
}
