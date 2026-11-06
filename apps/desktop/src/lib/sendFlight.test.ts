import { describe, expect, test } from "bun:test";
import { MAX_RISE, motionAllowed, SEND_INSET, SEND_MS, sendRise } from "./sendFlight";

describe("a sent message's flight out of the composer", () => {
  test("travels the gap between the row and the field", () => {
    // The composer's card 16px below the row's bottom, as the lane at the foot draws it.
    expect(sendRise(1_016, 1_000)).toBe(16 + SEND_INSET);
    // A row already above the composer stays put rather than flying up out of nowhere.
    expect(sendRise(900, 1_000)).toBe(0);
    // The whole number, never a fraction, so the transform never sub-pixel shifts the text.
    expect(sendRise(1_016.6, 1_000.2)).toBe(16 + SEND_INSET);
  });

  test("a long stack over the composer cannot send a row across the screen", () => {
    expect(sendRise(1_400, 1_000)).toBe(MAX_RISE);
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
