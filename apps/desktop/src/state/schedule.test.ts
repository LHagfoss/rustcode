// The presets, the countdown and the book send-later keeps (state/schedule.ts): what a preset
// resolves to on the user's clock, how a card says what is left, and a book that survives a restart.

import { describe, expect, test } from "bun:test";
import {
  addScheduled,
  countdown,
  dueScheduled,
  MAX_SCHEDULED_EXTRAS,
  MAX_SCHEDULED_ROWS,
  MAX_SCHEDULED_TEXT,
  parseScheduleBook,
  persistableSchedule,
  presetTime,
  type ScheduleBook,
  type ScheduledMessage,
  SEND_LATER_PRESETS,
  scheduledCount,
  scheduledSummary,
  scheduleRefusalText,
  whenText,
  withoutScheduled,
} from "./schedule";

const at = (day: number, hour: number, minute = 0) => new Date(2024, 0, day, hour, minute, 0, 0).getTime();
const message = (id: string, due: number, text = `message ${id}`): ScheduledMessage => ({ id, due, text });
/** The book with a message added, for a test that expects it to be kept. */
const added = (book: ScheduleBook, threadId: string, item: ScheduledMessage): ScheduleBook => {
  const next = addScheduled(book, threadId, item);
  if ("refused" in next) throw new Error(`refused: ${next.refused}`);
  return next.book;
};
const picture = (bytes: number) => ({
  links: [],
  attachments: [
    { id: "p", kind: "image" as const, name: "shot.png", mimeType: "image/png", data: "x".repeat(bytes) },
  ],
});

describe("the presets", () => {
  test("the offsets send from now, to the minute", () => {
    const now = at(10, 12);
    expect(presetTime("30m", now)).toBe(now + 30 * 60_000);
    expect(presetTime("1h", now)).toBe(now + 60 * 60_000);
    expect(presetTime("2h", now)).toBe(now + 120 * 60_000);
  });

  test("this evening is 18:00 today, and tomorrow once it has passed", () => {
    expect(presetTime("evening", at(10, 9))).toBe(at(10, 18));
    expect(presetTime("evening", at(10, 17, 59))).toBe(at(10, 18));
    // Exactly 18:00 is already the evening: the next one is tomorrow.
    expect(presetTime("evening", at(10, 18))).toBe(at(11, 18));
    expect(presetTime("evening", at(10, 23))).toBe(at(11, 18));
  });

  test("tomorrow morning is 09:00 the next day", () => {
    expect(presetTime("morning", at(10, 9))).toBe(at(11, 9));
    expect(presetTime("morning", at(10, 23))).toBe(at(11, 9));
    expect(presetTime("morning", at(31, 20))).toBe(at(1 + 31, 9));
  });

  test("every preset has a label", () => {
    expect(SEND_LATER_PRESETS.map((row) => row.id)).toEqual(["30m", "1h", "2h", "evening", "morning"]);
    for (const row of SEND_LATER_PRESETS) expect(row.label.length).toBeGreaterThan(0);
  });
});

describe("what the card says", () => {
  const now = at(10, 12);

  test("the countdown rounds up to the minute, then counts hours and days", () => {
    expect(countdown(now + 30_000, now)).toBe("in 1 min");
    expect(countdown(now + 29 * 60_000, now)).toBe("in 29 min");
    expect(countdown(now + 60 * 60_000, now)).toBe("in 1 h");
    expect(countdown(now + 65 * 60_000, now)).toBe("in 1 h 5 min");
    expect(countdown(now + 26 * 60 * 60_000, now)).toBe("in 1 d 2 h");
    expect(countdown(now, now)).toBe("now");
    expect(countdown(now - 60_000, now)).toBe("now");
  });

  test("the time is said against today and tomorrow", () => {
    expect(whenText(at(10, 18), now)).toContain("today at");
    expect(whenText(at(11, 9), now)).toContain("tomorrow at");
    // Further out it names the day, never "in N days" alone.
    expect(whenText(at(14, 9), now)).not.toContain("today");
  });

  test("the summary is the first line, or what rides above an empty one", () => {
    expect(scheduledSummary("hello\nthere")).toBe("hello");
    expect(scheduledSummary("x".repeat(200))).toHaveLength(141);
    expect(scheduledSummary("", { links: [], attachments: [{ id: "a", kind: "file", name: "a.txt" }] })).toBe(
      "1 attachment",
    );
    expect(scheduledSummary("")).toBe("A message");
  });
});

describe("the book as it is kept", () => {
  const now = at(10, 12);
  const empty: ScheduleBook = {};

  test("a message comes back as it went in, with what rides with it", () => {
    const extras = { links: ["src/state/store.ts"], attachments: [] };
    const book = added(empty, "t1", { ...message("a", now + 60_000), extras });
    expect(parseScheduleBook(JSON.stringify(book))).toEqual(book);
  });

  test("a thread's messages are kept in the order they send", () => {
    let book = added(empty, "t1", message("late", now + 120_000));
    book = added(book, "t1", message("soon", now + 30_000));
    book = added(book, "t1", message("later", now + 180_000));
    expect(book.t1!.map((row) => row.id)).toEqual(["soon", "late", "later"]);
    expect(dueScheduled(book, now + 130_000).map((row) => row.item.id)).toEqual(["soon", "late"]);
  });

  test("a row nobody could read back is dropped, and the rest of the book still parses", () => {
    const raw = JSON.stringify({
      t1: [message("ok", now), { id: "", text: "x", due: now }, { id: "no-due", text: "x" }],
      t2: "not a list",
    });
    expect(parseScheduleBook(raw).t1!.map((row) => row.id)).toEqual(["ok"]);
    expect(parseScheduleBook(null)).toEqual({});
    expect(parseScheduleBook("not json")).toEqual({});
  });

  test("a message too big to keep is refused rather than scheduled", () => {
    expect(addScheduled(empty, "t1", message("huge", now, "x".repeat(MAX_SCHEDULED_TEXT + 1)))).toEqual({
      refused: "text",
    });
    expect(addScheduled(empty, "t1", message("empty", now, ""))).toEqual({ refused: "empty" });
  });

  test("attachments too big to keep refuse the message instead of being dropped from it", () => {
    const big = { ...message("big", now), extras: picture(MAX_SCHEDULED_EXTRAS) } as ScheduledMessage;
    expect(addScheduled(empty, "t1", big)).toEqual({ refused: "extras" });
    expect(scheduleRefusalText("extras")).toContain("attachments are too big");
  });

  test("a message that is only an attachment is scheduled, and comes back after a restart", () => {
    const only = { ...message("pic", now, ""), extras: picture(100) } as ScheduledMessage;
    const book = added(empty, "t1", only);
    expect(parseScheduleBook(JSON.stringify(book))).toEqual(book);
    // Without its attachment there is nothing left of it, so a stripped copy is not kept.
    const stripped = JSON.stringify({ t1: [{ ...message("pic", now, ""), extras: { bad: true } }] });
    expect(parseScheduleBook(stripped)).toEqual({});
  });

  test("a full thread refuses one more instead of evicting one already scheduled", () => {
    let book = empty;
    for (let i = 0; i < MAX_SCHEDULED_ROWS; i++) book = added(book, "t1", message(`m${i}`, now + 10 + i));
    expect(addScheduled(book, "t1", message("soonest", now))).toEqual({ refused: "full" });
    expect(book.t1).toHaveLength(MAX_SCHEDULED_ROWS);
  });

  test("the stored book is bounded per thread, keeping the soonest", () => {
    // An older build could hold more than the cap: what is read back keeps the next ones to send.
    const rows = Array.from({ length: MAX_SCHEDULED_ROWS + 3 }, (_, i) => message(`m${i}`, now + i));
    const book = persistableSchedule({ t1: rows });
    expect(book.t1).toHaveLength(MAX_SCHEDULED_ROWS);
    expect(book.t1!.map((row) => row.id)).toEqual(
      Array.from({ length: MAX_SCHEDULED_ROWS }, (_, i) => `m${i}`),
    );
    expect(parseScheduleBook(JSON.stringify({ t1: [...rows].reverse() })).t1![0]!.id).toBe("m0");
  });

  test("removing a message leaves the others, and a thread with none keeps no entry", () => {
    const first = added(empty, "t1", message("a", now));
    const book = added(first, "t1", message("b", now + 1));
    const one = withoutScheduled(book, "t1", "a");
    expect(one.t1!.map((row) => row.id)).toEqual(["b"]);
    expect(withoutScheduled(one, "t1", "b")).toEqual({});
    // An id that is not there changes nothing, so nothing is written again.
    expect(withoutScheduled(one, "t1", "gone")).toBe(one);
  });

  test("the count is what the store's timer watches", () => {
    expect(scheduledCount({})).toBe(0);
    const one = added(empty, "t1", message("a", now));
    const book = added(one, "t2", message("b", now));
    expect(scheduledCount(book)).toBe(2);
  });
});
