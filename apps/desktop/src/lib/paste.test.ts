import { describe, expect, test } from "bun:test";
import { isLongPaste, PASTE_CHARS, PASTE_LINES, pastedImages, pasteLabel } from "./paste";

const lines = (n: number) => Array.from({ length: n }, (_, i) => `line ${i + 1}`).join("\n");

const file = (name: string, type: string) => new File([new Uint8Array([1, 2, 3])], name, { type });
const item = (type: string, held: File | null, kind = "file") => ({
  kind,
  type,
  getAsFile: () => held,
});

describe("a long paste", () => {
  test("is a chip past a dozen lines, text before that", () => {
    expect(isLongPaste(lines(PASTE_LINES - 1))).toBe(false);
    expect(isLongPaste(lines(PASTE_LINES))).toBe(true);
    expect(isLongPaste("a log line\nanother")).toBe(false);
  });

  test("is a chip past 1200 characters even with one line", () => {
    expect(isLongPaste("x".repeat(PASTE_CHARS - 1))).toBe(false);
    expect(isLongPaste("x".repeat(PASTE_CHARS))).toBe(true);
    expect(isLongPaste(`a\n${"x".repeat(PASTE_CHARS)}`)).toBe(true);
  });

  test("says how many lines it holds", () => {
    expect(pasteLabel(lines(84))).toBe("Pasted text · 84 lines");
    expect(pasteLabel(lines(PASTE_LINES))).toBe("Pasted text · 12 lines");
  });

  test("says how many characters a few long lines hold", () => {
    expect(pasteLabel("x".repeat(3200))).toBe("Pasted text · 3.2k chars");
    expect(pasteLabel("x".repeat(PASTE_CHARS))).toBe("Pasted text · 1.2k chars");
    expect(pasteLabel(`a\n${"x".repeat(12000)}`)).toBe("Pasted text · 12k chars");
  });
});

describe("the images a paste carries", () => {
  test("are the files the web view hands over", () => {
    const shot = file("shot.png", "image/png");
    expect(pastedImages({ files: [shot] })).toEqual([shot]);
    expect(pastedImages({ files: [], items: undefined })).toEqual([]);
  });

  test("are the image items when `files` is empty, as WebKitGTK leaves them", () => {
    const shot = file("shot.png", "image/png");
    const items = [item("text/plain", null, "string"), item("image/png", shot)];
    expect(pastedImages({ files: [], items })).toEqual([shot]);
  });

  test("take the picture and leave the text of a mixed paste to the field", () => {
    const shot = file("shot.png", "image/png");
    const items = [
      item("text/plain", null, "string"),
      item("image/png", shot),
      item("text/html", null, "string"),
    ];
    expect(pastedImages({ files: [file("notes.txt", "text/plain")], items })).toEqual([shot]);
  });

  test("are none for text alone", () => {
    const items = [item("text/plain", null, "string")];
    expect(pastedImages({ files: [file("notes.txt", "text/plain")], items })).toEqual([]);
    expect(pastedImages({ files: [] })).toEqual([]);
  });

  test("attach a picture both lists name once", () => {
    const shot = file("shot.png", "image/png");
    const items = [item("image/png", file("shot.png", "image/png"))];
    expect(pastedImages({ files: [shot], items })).toHaveLength(1);
  });

  test("skip an item the web view cannot turn into a file", () => {
    expect(pastedImages({ files: [], items: [{ kind: "file", type: "image/png" }] })).toEqual([]);
    expect(pastedImages({ files: [], items: [item("image/png", null)] })).toEqual([]);
  });
});
