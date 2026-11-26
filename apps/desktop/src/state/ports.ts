import { useEffect, useSyncExternalStore } from "react";
import { arr, type Json, num, obj, str } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { errorText, request, toast } from "./store";

export interface ListeningPort {
  port: number;
  pid: number;
  address: string;
  processName: string;
  label?: string;
  url: string;
}

export interface PortListAnswer {
  cwd: string;
  ports: ListeningPort[];
  error?: string;
}

const POLL_MS = 4_000;
const EMPTY_PORTS: readonly ListeningPort[] = [];

/** Parse one listening port entry from `_rustcode/ports/{list,kill}`. */
export function parseListeningPort(value: Json | undefined): ListeningPort | null {
  const o = obj(value);
  const port = num(o?.port);
  const pid = num(o?.pid);
  if (!o || port === undefined || pid === undefined || port < 1 || port > 65535) return null;
  const address = str(o.address) ?? "127.0.0.1";
  const label = str(o.label)?.trim() || undefined;
  const url = str(o.url) ?? `http://localhost:${port}`;
  return {
    port,
    pid,
    address,
    processName: str(o.processName) ?? "process",
    ...(label ? { label } : {}),
    url,
  };
}

/** Parse the response payload of `_rustcode/ports/list` or `_rustcode/ports/kill`. */
export function parsePortList(result: Json | undefined): PortListAnswer {
  const root = obj(result);
  const err = obj(root?.error);
  if (err) {
    return {
      cwd: "",
      ports: [],
      error: str(err.message) ?? "Could not inspect listening ports",
    };
  }
  const list = obj(root?.list);
  const ports = arr(list?.ports)
    .flatMap((entry) => {
      const parsed = parseListeningPort(entry);
      return parsed ? [parsed] : [];
    })
    .sort((a, b) => a.port - b.port);
  return {
    cwd: str(list?.cwd) ?? "",
    ports,
  };
}

/** Short pill label for one listening port (`Web :3000` or `:3000`). */
export function formatPortLabel(port: Pick<ListeningPort, "port" | "label">): string {
  return port.label ? `${port.label} :${port.port}` : `:${port.port}`;
}

/** Header summary for one or more active ports (`Web :3000` or `:3000 +2`). */
export function formatPortsSummary(ports: readonly ListeningPort[]): string {
  const first = ports[0];
  if (!first) return "";
  const head = formatPortLabel(first);
  return ports.length > 1 ? `${head} +${ports.length - 1}` : head;
}

interface CwdBucket {
  ports: readonly ListeningPort[];
  listeners: Set<() => void>;
  timer: number | null;
  inflight: boolean;
}

const buckets = new Map<string, CwdBucket>();

function getBucket(cwd: string): CwdBucket {
  let bucket = buckets.get(cwd);
  if (!bucket) {
    bucket = { ports: EMPTY_PORTS, listeners: new Set(), timer: null, inflight: false };
    buckets.set(cwd, bucket);
  }
  return bucket;
}

function samePorts(a: readonly ListeningPort[], b: readonly ListeningPort[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const x = a[i]!;
    const y = b[i]!;
    if (
      x.port !== y.port ||
      x.pid !== y.pid ||
      x.address !== y.address ||
      x.processName !== y.processName ||
      x.label !== y.label ||
      x.url !== y.url
    ) {
      return false;
    }
  }
  return true;
}

function setBucketPorts(cwd: string, ports: readonly ListeningPort[]) {
  const bucket = buckets.get(cwd);
  if (!bucket || samePorts(bucket.ports, ports)) return;
  bucket.ports = ports;
  for (const listener of bucket.listeners) listener();
}

/** Refresh active listening ports for `cwd` once. */
export async function refreshListeningPorts(cwd: string): Promise<readonly ListeningPort[]> {
  const bucket = getBucket(cwd);
  if (bucket.inflight) return bucket.ports;
  bucket.inflight = true;
  try {
    const answer = await request(Method.portsList, { cwd }, 10_000);
    const parsed = parsePortList(answer);
    if (!parsed.error) setBucketPorts(cwd, parsed.ports);
    return bucket.ports;
  } catch {
    return bucket.ports;
  } finally {
    bucket.inflight = false;
  }
}

/** Stop the process owning `port` inside `cwd` and immediately update the cached list. */
export async function killListeningPort(cwd: string, target: ListeningPort): Promise<boolean> {
  try {
    const answer = await request(Method.portsKill, { cwd, pid: target.pid, port: target.port }, 15_000);
    const parsed = parsePortList(answer);
    if (parsed.error) {
      toast(parsed.error, "error");
      return false;
    }
    setBucketPorts(cwd, parsed.ports);
    toast(`Stopped ${target.processName} on :${target.port}`, "success");
    return true;
  } catch (error) {
    toast(`Could not stop :${target.port}: ${errorText(error)}`, "error");
    return false;
  }
}

function subscribeCwd(cwd: string, listener: () => void): () => void {
  const bucket = getBucket(cwd);
  bucket.listeners.add(listener);
  if (bucket.timer === null && typeof window !== "undefined") {
    void refreshListeningPorts(cwd);
    bucket.timer = window.setInterval(() => {
      if (typeof document !== "undefined" && document.hidden) return;
      void refreshListeningPorts(cwd);
    }, POLL_MS);
  }
  return () => {
    bucket.listeners.delete(listener);
    if (bucket.listeners.size === 0 && bucket.timer !== null) {
      window.clearInterval(bucket.timer);
      bucket.timer = null;
    }
  };
}

/** Subscribe to active listening TCP ports scoped to `cwd`. */
export function useListeningPorts(cwd: string | undefined): readonly ListeningPort[] {
  const subscribe = (listener: () => void) => (cwd ? subscribeCwd(cwd, listener) : () => {});
  const snapshot = () => (cwd ? (buckets.get(cwd)?.ports ?? EMPTY_PORTS) : EMPTY_PORTS);
  const ports = useSyncExternalStore(subscribe, snapshot, () => EMPTY_PORTS);

  useEffect(() => {
    if (!cwd || typeof document === "undefined") return;
    const onVisible = () => {
      if (!document.hidden) void refreshListeningPorts(cwd);
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [cwd]);

  return ports;
}
