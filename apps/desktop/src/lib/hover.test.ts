// The one tooltip and hover-card timing (lib/hover.ts): the rest before a tip appears, the close
// delay that survives a slip, the instant hand-over between neighbours, and the dismissal that
// keeps a tip from outliving its pointer.

import { describe, expect, test } from "bun:test";
import { HOVER_CLOSE_MS, HOVER_OPEN_MS, HOVER_SWITCH_MS, type HoverTiming, HoverTips } from "./hover";

/** A clock that only moves when the test says so. */
function clock(): HoverTiming & { advance: (ms: number) => void } {
  let now = 0;
  const timers: { at: number; run: () => void }[] = [];
  return {
    now: () => now,
    after: (ms, run) => {
      const timer = { at: now + ms, run };
      timers.push(timer);
      return timer;
    },
    cancel: (handle) => {
      const at = timers.indexOf(handle as { at: number; run: () => void });
      if (at >= 0) timers.splice(at, 1);
    },
    advance(ms) {
      const until = now + ms;
      for (;;) {
        const due = timers.filter((t) => t.at <= until).sort((a, b) => a.at - b.at)[0];
        if (!due) break;
        timers.splice(timers.indexOf(due), 1);
        now = due.at;
        due.run();
      }
      now = until;
    },
  };
}

function tips() {
  const time = clock();
  return { time, store: new HoverTips(time) };
}

const a = Symbol("a");
const b = Symbol("b");

describe("hover tip timing", () => {
  test("a tip waits HOVER_OPEN_MS of resting, and not a millisecond less", () => {
    const { store, time } = tips();
    store.enter(a);
    time.advance(HOVER_OPEN_MS - 1);
    expect(store.isOpen(a)).toBe(false);
    time.advance(1);
    expect(store.isOpen(a)).toBe(true);
  });

  test("a pointer that leaves before the rest is up never shows a tip", () => {
    const { store, time } = tips();
    store.enter(a);
    time.advance(HOVER_OPEN_MS / 2);
    store.leave(a);
    time.advance(HOVER_OPEN_MS * 2);
    expect(store.isOpen(a)).toBe(false);
  });

  test("leaving closes after the short delay, and coming back inside it changes nothing", () => {
    const { store, time } = tips();
    store.enter(a);
    time.advance(HOVER_OPEN_MS);
    store.leave(a);
    time.advance(HOVER_CLOSE_MS - 1);
    expect(store.isOpen(a)).toBe(true);
    store.enter(a);
    time.advance(HOVER_CLOSE_MS * 2);
    expect(store.isOpen(a)).toBe(true);
    store.leave(a);
    time.advance(HOVER_CLOSE_MS);
    expect(store.isOpen(a)).toBe(false);
  });

  test("a neighbour takes the open tip's place at once, and only one is open", () => {
    const { store, time } = tips();
    store.enter(a);
    time.advance(HOVER_OPEN_MS);
    const seen: string[] = [];
    store.subscribe(() => seen.push(`${String(store.isOpen(a))}/${String(store.isOpen(b))}`));
    store.enter(b);
    expect(store.isOpen(a)).toBe(false);
    expect(store.isOpen(b)).toBe(true);
    expect(seen).toHaveLength(1);
  });

  test("a target entered just after a tip closed shows at once, and one entered late waits", () => {
    const { store, time } = tips();
    store.enter(a);
    time.advance(HOVER_OPEN_MS);
    store.leave(a);
    time.advance(HOVER_CLOSE_MS);
    expect(store.isOpen(a)).toBe(false);

    time.advance(HOVER_SWITCH_MS - 10);
    store.enter(b);
    expect(store.isOpen(b)).toBe(true);

    store.leave(b);
    time.advance(HOVER_CLOSE_MS);
    time.advance(HOVER_SWITCH_MS + 10);
    store.enter(a);
    expect(store.isOpen(a)).toBe(false);
    time.advance(HOVER_OPEN_MS);
    expect(store.isOpen(a)).toBe(true);
  });

  test("a press puts the tip out at once, cancels one still waiting, and nothing reopens it", () => {
    const { store, time } = tips();
    store.enter(a);
    time.advance(HOVER_OPEN_MS);
    store.dismiss(a);
    expect(store.isOpen(a)).toBe(false);
    // The pointer still rests on the target: no come-back without a fresh enter.
    time.advance(HOVER_SWITCH_MS * 4);
    expect(store.isOpen(a)).toBe(false);

    store.leave(a);
    store.enter(a);
    store.dismiss(a);
    time.advance(HOVER_OPEN_MS * 2);
    expect(store.isOpen(a)).toBe(false);
  });

  test("a target that goes away leaves no tip behind, and its neighbours still hand over", () => {
    const { store, time } = tips();
    store.enter(a);
    time.advance(HOVER_OPEN_MS);
    store.forget(a);
    expect(store.isOpen(a)).toBe(false);
    store.enter(b);
    expect(store.isOpen(b)).toBe(true);
  });
});
