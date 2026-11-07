import { describe, expect, test } from "bun:test";
import { flightLog, MAX_RISE, motionAllowed, SEND_INSET, SEND_MS, sendRise } from "./sendFlight";

describe("a sent message's flight out of the composer", () => {
  test("travels the gap between the row and the field", () => {
    // The composer's card 16px below the row's bottom, as the lane at the foot draws it.
    expect(sendRise(1_016, 1_000)).toBe(16 + SEND_INSET);
    // The whole number, never a fraction, so the transform never sub-pixel shifts the text.
    expect(sendRise(1_016.6, 1_000.2)).toBe(16 + SEND_INSET);
  });

  test("a row resting above the field still comes out of it", () => {
    // A turn's `Working` line under the message, and a reply already streaming, hold the bubble
    // above the card: it then starts that far below its place and rises into it, rather than
    // sitting still and only fading (which reads as no motion at all).
    expect(sendRise(900, 1_000)).toBe(-(1_000 - 900 - SEND_INSET));
    expect(sendRise(900, 1_000)).toBeLessThan(0);
  });

  test("a long stack over the composer cannot send a row across the screen", () => {
    expect(sendRise(1_400, 1_000)).toBe(MAX_RISE);
    expect(sendRise(200, 1_000)).toBe(-MAX_RISE);
  });

  test("its length is the app's own ease step, not a row's", () => {
    expect(SEND_MS).toBe(220);
  });

  test("motion is on unless the reader asked for less", () => {
    const media = globalThis.matchMedia;
    try {
      globalThis.matchMedia = ((query: string) => ({ matches: query.includes("reduce") })) as never;
      expect(motionAllowed()).toBe(false);
      globalThis.matchMedia = (() => ({ matches: false })) as never;
      expect(motionAllowed()).toBe(true);
    } finally {
      if (media) globalThis.matchMedia = media;
      else Reflect.deleteProperty(globalThis, "matchMedia");
    }
  });
});

describe("one flight per message", () => {
  test("takes a message once, however often its row is drawn", () => {
    const takesFlight = flightLog();
    expect(takesFlight("user-1")).toBe(true);
    // A streaming reply re-renders the row, and the transcript remounts when the thread is reopened.
    expect(takesFlight("user-1")).toBe(false);
    expect(takesFlight("user-1")).toBe(false);
  });

  test("a message with no id, or no message, never flies", () => {
    const takesFlight = flightLog();
    expect(takesFlight(undefined)).toBe(false);
    expect(takesFlight("")).toBe(false);
  });

  test("each send is its own flight", () => {
    const takesFlight = flightLog();
    expect(takesFlight("user-1")).toBe(true);
    expect(takesFlight("user-2")).toBe(true);
    expect(takesFlight("user-2")).toBe(false);
  });
});
