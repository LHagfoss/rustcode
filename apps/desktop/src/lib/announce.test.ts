import { describe, expect, test } from "bun:test";
import { COALESCE_MS, createAnnouncer, REPEAT_MS } from "./announce";

function harness() {
  let now = 1_000;
  const timers: { at: number; run: () => void }[] = [];
  const said: { text: string; assertive: boolean }[] = [];
  const announcer = createAnnouncer({
    now: () => now,
    schedule: (run, ms) => void timers.push({ at: now + ms, run }),
    write: (text, assertive) => void said.push({ text, assertive }),
  });
  const advance = (ms: number) => {
    now += ms;
    for (const timer of timers.splice(0).sort((a, b) => a.at - b.at)) {
      if (timer.at <= now) timer.run();
      else timers.push(timer);
    }
  };
  return { announcer, said, advance };
}

describe("createAnnouncer", () => {
  test("says a polite message on the next tick and an assertive one at once", () => {
    const { announcer, said, advance } = harness();
    announcer.announce("A: needs approval", { assertive: true });
    expect(said).toEqual([{ text: "A: needs approval", assertive: true }]);
    announcer.announce("A: working");
    expect(said).toHaveLength(1);
    advance(0);
    expect(said[1]).toEqual({ text: "A: working", assertive: false });
  });

  test("coalesces a burst into one announcement", () => {
    const { announcer, said, advance } = harness();
    announcer.announce("A: finished");
    advance(0);
    announcer.announce("B: working");
    announcer.announce("C: working");
    advance(COALESCE_MS - 1);
    expect(said).toHaveLength(1);
    advance(1);
    expect(said.map((x) => x.text)).toEqual(["A: finished", "B: working. C: working"]);
  });

  test("drops identical repeats until the repeat window passes", () => {
    const { announcer, said, advance } = harness();
    announcer.announce("A: failed", { assertive: true });
    announcer.announce("A: failed", { assertive: true });
    advance(REPEAT_MS - 1);
    announcer.announce("A: failed", { assertive: true });
    expect(said).toHaveLength(1);
    advance(1);
    announcer.announce("A: failed", { assertive: true });
    expect(said).toHaveLength(2);
  });

  test("ignores empty text", () => {
    const { announcer, said, advance } = harness();
    announcer.announce("   ");
    advance(COALESCE_MS);
    expect(said).toEqual([]);
  });
});
