// Only the newest ask may be shown (lib/latest.ts): the guard the Changes
// panel needs when a thread's folder changes mid-read.

import { describe, expect, test } from "bun:test";
import { latest } from "./latest";

describe("the newest ask wins", () => {
  test("a newer ticket makes the older one stale", () => {
    const asks = latest();
    const first = asks.take();
    expect(asks.current(first)).toBe(true);
    const second = asks.take();
    expect(asks.current(first)).toBe(false);
    expect(asks.current(second)).toBe(true);
  });

  test("dropping what is in flight leaves the next ask current", () => {
    const asks = latest();
    const first = asks.take();
    asks.drop();
    expect(asks.current(first)).toBe(false);
    const next = asks.take();
    expect(asks.current(next)).toBe(true);
    expect(asks.current(first)).toBe(false);
  });

  test("answers that arrive out of order leave the newest one", async () => {
    const asks = latest();
    const shown: string[] = [];
    const ask = (what: string, ms: number) => {
      const ticket = asks.take();
      return new Promise<string>((resolve) => setTimeout(() => resolve(what), ms)).then((answer) => {
        if (asks.current(ticket)) shown.push(answer);
      });
    };
    await Promise.all([ask("the previous thread's folder", 30), ask("this thread's folder", 1)]);
    expect(shown).toEqual(["this thread's folder"]);
  });
});
