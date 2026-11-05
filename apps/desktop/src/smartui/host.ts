// The live Smart UI pages: at most MAX_LIVE run at once (docs/smart-ui.md).
// A view runs by itself when it is seen, so more views can be in sight than
// may run: starting one past the limit stops another, out of sight first, then
// the one that started longest ago. That is not a failure, and the stopped
// view keeps out until it has left the viewport and come back (`AutoRun`), or
// views in sight would keep evicting each other for ever.

import { MAX_LIVE } from "./parse";

const live: { id: string; stop: () => void; visible: () => boolean }[] = [];

export function claimLive(id: string, stop: () => void, visible: () => boolean = () => true) {
  releaseLive(id);
  while (live.length >= MAX_LIVE) {
    const hidden = live.findIndex((x) => !x.visible());
    const [evicted] = live.splice(Math.max(hidden, 0), 1);
    evicted?.stop();
  }
  live.push({ id, stop, visible });
}

export function releaseLive(id: string) {
  const at = live.findIndex((x) => x.id === id);
  if (at >= 0) live.splice(at, 1);
}

export function liveCount(): number {
  return live.length;
}

/** Whether a view may start by itself: in sight, and not evicted since it last came into sight. */
export class AutoRun {
  private seen = false;
  private held = false;
  see(inView: boolean) {
    this.seen = inView;
    if (!inView) this.held = false;
  }
  /** A view evicted out of sight has already left, so it comes back like any other. */
  evicted() {
    this.held = this.seen;
  }
  get inView() {
    return this.seen;
  }
  get ready() {
    return this.seen && !this.held;
  }
}

/** A Map that forgets its least recently written keys, for what outlives a view's row. */
class Recent<V> {
  private readonly map = new Map<string, V>();
  constructor(private readonly limit: number) {}
  get(key: string) {
    return this.map.get(key);
  }
  set(key: string, value: V) {
    this.map.delete(key);
    this.map.set(key, value);
    if (this.map.size > this.limit) {
      const oldest = this.map.keys().next().value;
      if (oldest !== undefined) this.map.delete(oldest);
    }
  }
  delete(key: string) {
    this.map.delete(key);
  }
}

const KEPT = 200;
const heights = new Recent<number>(KEPT);

/** The last measured height of a block, so a re-run does not move the transcript. */
export const rememberedHeight = (source: string) => heights.get(source);
export const rememberHeight = (source: string, height: number) => heights.set(source, height);

/**
 * Why a view does not run by itself: the user pressed Stop, or it failed (a
 * busy loop must not freeze the app again each time it scrolls into sight).
 * Kept here because the transcript unmounts a row once it is far off screen.
 */
export type Halt = { kind: "stopped" } | { kind: "failed"; reason: string };
const halts = new Recent<Halt>(KEPT);

export const haltOf = (key: string) => halts.get(key);
export const haltView = (key: string, halt: Halt) => halts.set(key, halt);
export const clearHalt = (key: string) => halts.delete(key);

/** A random per-frame token: the page's messages must carry it. */
export function frameToken(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** Rate limit for `rustcode.send`: one message per 2 s per page. */
export class SendGate {
  private last = 0;
  constructor(private readonly gapMs = 2000) {}
  allow(now = Date.now()): boolean {
    if (now - this.last < this.gapMs) return false;
    this.last = now;
    return true;
  }
}
