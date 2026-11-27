import { describe, expect, test } from "bun:test";
import { emptyExtras } from "./compose";
import { MAX_DRAFT_IMAGE_CHARS, parseExtrasBook, persistableExtras } from "./draftExtras";

const paste = { id: "p1", kind: "text" as const, text: "long paste", at: 3 };
const image = (id: string, size: number) => ({
  id,
  kind: "image" as const,
  name: `${id}.png`,
  mimeType: "image/png",
  data: "x".repeat(size),
});

describe("draft extras", () => {
  test("a paste and a mention survive a reload", () => {
    const book = {
      t1: {
        ...emptyExtras,
        attachments: [paste],
        mentions: [{ kind: "thread" as const, id: "a", title: "A" }],
      },
    };
    expect(parseExtrasBook(persistableExtras(book))).toEqual(book);
  });

  test("empty extras are not written", () => {
    expect(persistableExtras({ t1: emptyExtras })).toBeNull();
  });

  test("images past the per-draft budget are dropped", () => {
    const kept = image("a", MAX_DRAFT_IMAGE_CHARS - 10);
    const book = { t1: { ...emptyExtras, attachments: [kept, image("b", 20), paste] } };
    expect(parseExtrasBook(persistableExtras(book)).t1?.attachments).toEqual([kept, paste]);
  });

  test("a book that is not JSON reads as empty", () => {
    expect(parseExtrasBook("{nope")).toEqual({});
    expect(parseExtrasBook(JSON.stringify({ t1: { links: "x" } }))).toEqual({});
  });
});
