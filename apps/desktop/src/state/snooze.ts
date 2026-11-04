// Snoozing a thread: out of the list until a time. Pure, so the menu, the shelf and the tests
// share it. Kept by the app (`rustcode.snoozed`), one wake time per thread.

export interface SnoozePreset {
  label: string;
  at: number;
}

const at = (base: Date, days: number, hour: number) => {
  const d = new Date(base);
  d.setDate(d.getDate() + days);
  d.setHours(hour, 0, 0, 0);
  return d.getTime();
};

const clock = (time: number) =>
  new Date(time).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });

/** The menu's choices: this evening only while it is still ahead, then the next two mornings to mention. */
export function snoozePresets(now: number): SnoozePreset[] {
  const today = new Date(now);
  const presets: SnoozePreset[] = [{ label: "In 1 hour", at: now + 3_600_000 }];
  if (today.getHours() < 17) presets.push({ label: "This evening", at: at(today, 0, 18) });
  presets.push({ label: "Tomorrow morning", at: at(today, 1, 9) });
  const untilMonday = ((8 - today.getDay()) % 7) + (today.getDay() === 1 ? 7 : 0) || 7;
  presets.push({ label: "Next week", at: at(today, untilMonday, 9) });
  return presets;
}

/**
 * What `update_session`'s `snoozed_until` means, in ms: a named choice, as an
 * agent without a clock can give, or an ISO 8601 time. `NaN` for anything else.
 */
export function snoozeAt(value: string, now: number): number {
  const today = new Date(now);
  const pick = (label: string) => snoozePresets(now).find((p) => p.label === label)?.at;
  switch (value) {
    case "1h":
      return now + 3_600_000;
    case "evening":
      return pick("This evening") ?? at(today, 1, 18);
    case "tomorrow":
      return at(today, 1, 9);
    case "next_week":
      return pick("Next week") ?? Number.NaN;
    default:
      return Date.parse(value);
  }
}

/** When a snoozed thread comes back, short: `18:00`, `Tomorrow 09:00`, `Mon 09:00`, `20 Nov`. */
export function wakeLabel(time: number, now: number): string {
  const days = Math.round((at(new Date(time), 0, 0) - at(new Date(now), 0, 0)) / 86_400_000);
  if (days <= 0) return clock(time);
  if (days === 1) return `Tomorrow ${clock(time)}`;
  const date = new Date(time);
  if (days < 7) return `${date.toLocaleDateString([], { weekday: "short" })} ${clock(time)}`;
  return date.toLocaleDateString([], { day: "numeric", month: "short" });
}

/** The threads whose time has come. */
export function dueSnoozes(snoozed: Record<string, number>, now: number): string[] {
  return Object.keys(snoozed).filter((id) => (snoozed[id] ?? 0) <= now);
}
