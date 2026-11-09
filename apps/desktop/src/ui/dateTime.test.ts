import { expect, test } from "bun:test";
import { dayStart, daysAhead, localDateTime, monthDays } from "./dateTime";

test("local dates retain calendar day across midnight", () => {
  const day = new Date(2024, 1, 29);
  expect(monthDays(day)).toHaveLength(29);
  expect(localDateTime(day, 0, 15)?.getDate()).toBe(29);
  expect(dayStart(new Date(2024, 1, 29, 23))).toBe(dayStart(day));
});

test("local time rejects the DST gap and preserves midnight", () => {
  const previous = process.env.TZ;
  process.env.TZ = "America/New_York";
  try {
    expect(localDateTime(new Date(2024, 2, 10), 2, 30)).toBeNull();
    expect(localDateTime(new Date(2024, 2, 10), 3, 30)?.toISOString()).toBe("2024-03-10T07:30:00.000Z");
    expect(localDateTime(new Date(2024, 2, 11), 0, 15)?.toISOString()).toBe("2024-03-11T04:15:00.000Z");
    expect(localDateTime(new Date(2024, 10, 3), 1, 30)?.toISOString()).toBe("2024-11-03T05:30:00.000Z");
  } finally {
    process.env.TZ = previous;
  }
});

test("days ahead counts calendar days, not 24-hour spans", () => {
  const now = new Date(2024, 2, 9, 22);
  expect(daysAhead(new Date(2024, 2, 9, 23), now)).toBe("today");
  expect(daysAhead(new Date(2024, 2, 10, 0, 5), now)).toBe("tomorrow");
  expect(daysAhead(new Date(2024, 2, 12, 9), now)).toBe("in 3 days");
});
