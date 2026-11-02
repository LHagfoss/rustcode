// The live Smart UI pages: at most three run at once (docs/smart-ui.md);
// starting a fourth stops the one that started longest ago, which then
// shows its Run button again. Not a failure, so no source is shown.

import { MAX_LIVE } from "./parse";

const live: { id: string; stop: () => void }[] = [];

export function claimLive(id: string, stop: () => void) {
  releaseLive(id);
  live.push({ id, stop });
  while (live.length > MAX_LIVE) live.shift()?.stop();
}

export function releaseLive(id: string) {
  const at = live.findIndex((x) => x.id === id);
  if (at >= 0) live.splice(at, 1);
}

export function liveCount(): number {
  return live.length;
}

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
