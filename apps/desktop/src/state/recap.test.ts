import { describe, expect, test } from "bun:test";
import {
  chatRecapOf,
  parseRecapMinutes,
  plainLine,
  pruneSeen,
  RECAP_MINUTES_DEFAULT,
  recapDue,
  recapMinutesLabel,
  SEEN_LIMIT,
  shorten,
} from "./recap";
import type { TranscriptItem } from "./thread";

const NOW = 1_700_000_000_000;
const MINUTE = 60_000;

const user = (
  text: string,
  extra: Partial<Extract<TranscriptItem, { kind: "user" }>> = {},
): TranscriptItem => ({
  kind: "user",
  id: `u-${text}`,
  text,
  ...extra,
});
const reply = (text: string): TranscriptItem => ({ kind: "assistant", id: `a-${text}`, text });

describe("when a chat earns a recap", () => {
  test("a chat never seen never earns one, and a long enough gap does", () => {
    expect(recapDue(undefined, NOW, 60)).toBe(false);
    expect(recapDue(NOW - 59 * MINUTE, NOW, 60)).toBe(false);
    expect(recapDue(NOW - 60 * MINUTE, NOW, 60)).toBe(true);
  });

  test("a stored threshold outside the options falls back to the default", () => {
    expect(parseRecapMinutes(null)).toBe(RECAP_MINUTES_DEFAULT);
    expect(parseRecapMinutes("15")).toBe(15);
    expect(parseRecapMinutes("7")).toBe(RECAP_MINUTES_DEFAULT);
    expect(parseRecapMinutes("nonsense")).toBe(RECAP_MINUTES_DEFAULT);
    expect(recapMinutesLabel(15)).toBe("15m");
    expect(recapMinutesLabel(240)).toBe("4h");
  });

  test("the last-seen map keeps only the most recent chats", () => {
    const seen: Record<string, number> = {};
    for (let i = 0; i < SEEN_LIMIT + 5; i++) seen[`t${i}`] = i + 1;
    const kept = pruneSeen(seen);
    expect(Object.keys(kept)).toHaveLength(SEEN_LIMIT);
    expect(kept.t0).toBeUndefined();
    expect(kept[`t${SEEN_LIMIT + 4}`]).toBe(SEEN_LIMIT + 5);
  });
});

describe("what the recap says", () => {
  test("the last question and the reply to it", () => {
    const recap = chatRecapOf({
      items: [
        user("old"),
        reply("old answer"),
        user("Fix the **composer**"),
        reply("## Done\nThe composer keeps `focus`."),
      ],
      pending: 0,
      running: false,
    });
    expect(recap).toEqual({
      asked: "Fix the composer",
      reply: "Done The composer keeps focus.",
      state: "done",
    });
  });

  test("an older answer is not the reply to a newer question", () => {
    const recap = chatRecapOf({
      items: [user("one"), reply("answer one"), user("two")],
      pending: 0,
      running: true,
    });
    expect(recap).toEqual({ asked: "two", reply: undefined, state: "running" });
  });

  test("wakes and compaction dividers are not questions", () => {
    const recap = chatRecapOf({
      items: [user("real"), reply("first"), user("woke", { wake: true }), reply("second")],
      pending: 0,
      running: false,
    });
    expect(recap?.asked).toBe("real");
    expect(recap?.reply).toBe("second");
  });

  test("a waiting request and a failed turn say so", () => {
    const failed: TranscriptItem = { kind: "notice", id: "n", text: "boom", tone: "error" };
    expect(chatRecapOf({ items: [user("go"), failed], pending: 0, running: false })?.state).toBe("failed");
    expect(chatRecapOf({ items: [user("go")], pending: 1, running: true })?.state).toBe("needs");
  });

  test("an empty chat has nothing to recap", () => {
    expect(chatRecapOf({ items: [], pending: 0, running: false })).toBeNull();
  });

  test("long text is cut at a sentence or a word", () => {
    expect(shorten("One two three. Four five six seven.", 22)).toBe("One two three.");
    expect(shorten("alpha beta gamma delta epsilon", 20)).toBe("alpha beta gamma…");
    expect(plainLine("```ts\nconst a = 1;\n```\nafter")).toBe("after");
  });
});
