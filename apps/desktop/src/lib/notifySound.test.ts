import { describe, expect, test } from "bun:test";
import {
  clampNotifySoundVolume,
  cueForNotifyKind,
  DEFAULT_NOTIFY_SOUND_TONE,
  DEFAULT_NOTIFY_SOUND_VOLUME,
  NOTIFY_SOUND_TONES,
  parseNotifySoundTone,
  parseNotifySoundVolume,
  playNotificationSound,
  toneNotes,
} from "./notifySound";

describe("notification sound settings", () => {
  test("parses valid tones and falls back to the default for unknown values", () => {
    for (const option of NOTIFY_SOUND_TONES) {
      expect(parseNotifySoundTone(option.value)).toBe(option.value);
    }
    expect(parseNotifySoundTone(null)).toBe(DEFAULT_NOTIFY_SOUND_TONE);
    expect(parseNotifySoundTone("")).toBe(DEFAULT_NOTIFY_SOUND_TONE);
    expect(parseNotifySoundTone("airhorn")).toBe(DEFAULT_NOTIFY_SOUND_TONE);
  });

  test("clamps and parses volume within 0..100", () => {
    expect(clampNotifySoundVolume(-10)).toBe(0);
    expect(clampNotifySoundVolume(150)).toBe(100);
    expect(clampNotifySoundVolume(42.6)).toBe(43);
    expect(clampNotifySoundVolume(Number.NaN)).toBe(DEFAULT_NOTIFY_SOUND_VOLUME);
    expect(parseNotifySoundVolume(null)).toBe(DEFAULT_NOTIFY_SOUND_VOLUME);
    expect(parseNotifySoundVolume("")).toBe(DEFAULT_NOTIFY_SOUND_VOLUME);
    expect(parseNotifySoundVolume("bad")).toBe(DEFAULT_NOTIFY_SOUND_VOLUME);
    expect(parseNotifySoundVolume("0")).toBe(0);
    expect(parseNotifySoundVolume("80")).toBe(80);
  });

  test("maps finished to the finished cue and attention kinds to attention", () => {
    expect(cueForNotifyKind("finished")).toBe("finished");
    expect(cueForNotifyKind("approval")).toBe("attention");
    expect(cueForNotifyKind("question")).toBe("attention");
    expect(cueForNotifyKind("limit")).toBe("attention");
    expect(cueForNotifyKind("failed")).toBe("attention");
  });

  test("synthesizes distinct positive-frequency note schedules for every tone and cue", () => {
    for (const option of NOTIFY_SOUND_TONES) {
      const attention = toneNotes(option.value, "attention");
      const finished = toneNotes(option.value, "finished");
      expect(attention.length).toBeGreaterThan(0);
      expect(finished.length).toBeGreaterThan(0);
      for (const note of [...attention, ...finished]) {
        expect(note.freq).toBeGreaterThan(100);
        expect(note.duration).toBeGreaterThan(0);
        expect(note.gain).toBeGreaterThan(0);
      }
      expect(attention).not.toEqual(finished);
    }
  });

  test("playNotificationSound returns false when muted or when Web Audio is unavailable", async () => {
    expect(await playNotificationSound("chime", 0, "attention")).toBe(false);
    expect(await playNotificationSound("chime", 50, "finished")).toBe(false);
  });
});
