import { expect, test } from "bun:test";
import { clip } from "./clip";

/** A cut that lands inside a character leaves a lone surrogate half behind. */
const loneSurrogate = (text: string) => {
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = text.charCodeAt(i + 1);
      if (next >= 0xdc00 && next <= 0xdfff) i++;
      else return true;
    } else if (code >= 0xdc00 && code <= 0xdfff) return true;
  }
  return false;
};

test("text that already fits is untouched", () => {
  expect(clip("hello", 50)).toBe("hello");
  // Exactly `max` units is not a cut.
  expect(clip("hello", 5)).toBe("hello");
  expect(clip("", 0)).toBe("");
  expect(clip("hello", 0)).toBe("");
});

test("ASCII is cut where it always was", () => {
  expect(clip("hello world", 5)).toBe("hello");
  expect(clip("x".repeat(70), 59)).toBe("x".repeat(59));
});

test("a family emoji at the cut is dropped whole, never halved", () => {
  const family = "👨‍👩‍👧‍👦";
  expect(family).toHaveLength(11);
  const text = `aa${family}bb`;
  expect(clip(text, 5)).toBe("aa");
  expect(clip(text, 12)).toBe("aa");
  // Just past the family: it stays whole.
  expect(clip(text, 13)).toBe(`aa${family}`);
  expect(loneSurrogate(clip(text, 5))).toBe(false);
});

test("flags and skin tones are cut as one character each", () => {
  const france = "🇫🇷";
  expect(clip(`${france}${france}`, 2)).toBe("");
  expect(clip(`${france}${france}`, 4)).toBe(france);
  expect(clip(`${france}${france}`, 6)).toBe(france);
  const thumb = "👍🏽";
  expect(clip(`ok ${thumb} there`, 4)).toBe("ok ");
  expect(clip(`ok ${thumb} there`, 7)).toBe(`ok ${thumb}`);
});

test("a combining mark stays with the letter it modifies", () => {
  // "café", the accent as its own code point, so the letter is units 3 and the mark 4.
  const cafe = "cafe\u0301";
  expect(clip(cafe, 4)).toBe("caf");
  expect(clip(cafe, 5)).toBe(cafe);
  expect(clip(`${cafe} bar`, 6)).toBe(`${cafe} `);
});

test("a very long string is cut without being segmented in full", () => {
  const huge = `${"a".repeat(1_000_000)}👍`;
  const started = performance.now();
  const cut = clip(huge, 60);
  expect(cut).toBe("a".repeat(60));
  expect(performance.now() - started).toBeLessThan(500);
});

test("a webview without Intl.Segmenter still never splits a surrogate pair", async () => {
  // Read at load, so this needs a fresh copy of the module.
  const { Segmenter } = Intl as { Segmenter?: unknown };
  delete (Intl as { Segmenter?: unknown }).Segmenter;
  try {
    // Not a literal specifier: typecheck has no such module, and the query string is bun's
    // cache-buster, so this is a second, freshly loaded copy of the file.
    const specifier: string = "./clip.ts?no-segmenter";
    const { clip: withoutSegmenter } = (await import(specifier)) as {
      clip: (text: string, max: number) => string;
    };
    expect(withoutSegmenter("aaaa🚀", 5)).toBe("aaaa");
    expect(withoutSegmenter("aaaa🚀", 4)).toBe("aaaa");
    expect(withoutSegmenter("hello world", 5)).toBe("hello");
    expect(withoutSegmenter("x".repeat(70), 59)).toBe("x".repeat(59));
    expect(withoutSegmenter("short", 50)).toBe("short");
  } finally {
    (Intl as { Segmenter?: unknown }).Segmenter = Segmenter;
  }
});
