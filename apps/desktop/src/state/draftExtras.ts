import { type ComposeExtras, carriesExtras } from "./compose";

/** What a draft carries besides its words (pastes, images, files, links, mentions, the command), per key. */
export type ExtrasBook = Record<string, ComposeExtras>;

export const DRAFT_EXTRAS_KEY = "rustcode.draftExtras";
/** At most this many drafts keep their extras, newest first. */
export const MAX_DRAFT_EXTRAS = 50;
/** Images past this many characters of data, per draft, are not kept: they go back to being attached by hand. */
export const MAX_DRAFT_IMAGE_CHARS = 1024 * 1024;
/** The whole book's ceiling, so pictures cannot crowd the drafts and settings out of app storage. */
export const MAX_EXTRAS_BOOK_CHARS = 3 * 1024 * 1024;

const isExtras = (value: unknown): value is ComposeExtras =>
  !!value &&
  typeof value === "object" &&
  Array.isArray((value as ComposeExtras).links) &&
  Array.isArray((value as ComposeExtras).attachments);

/** A stored book read back: anything that is not a draft's extras is dropped. */
export function parseExtrasBook(raw: string | null): ExtrasBook {
  if (!raw) return {};
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return {};
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  return Object.fromEntries(
    Object.entries(value as Record<string, unknown>).filter(
      (entry): entry is [string, ComposeExtras] => isExtras(entry[1]) && carriesExtras(entry[1]),
    ),
  );
}

function withinImageBudget(extras: ComposeExtras): ComposeExtras {
  let budget = MAX_DRAFT_IMAGE_CHARS;
  const attachments = extras.attachments.filter((a) => {
    if (a.kind !== "image") return true;
    budget -= a.data.length;
    return budget >= 0;
  });
  return attachments.length === extras.attachments.length ? extras : { ...extras, attachments };
}

/** The book as it goes to storage: the newest drafts, each image budget kept, under the book's ceiling. */
export function persistableExtras(book: ExtrasBook): string | null {
  const entries = Object.entries(book)
    .filter(([, extras]) => carriesExtras(extras))
    .slice(-MAX_DRAFT_EXTRAS)
    .map(([key, extras]) => [key, withinImageBudget(extras)] as const);
  while (entries.length) {
    const text = JSON.stringify(Object.fromEntries(entries));
    if (text.length <= MAX_EXTRAS_BOOK_CHARS) return text;
    entries.shift();
  }
  return null;
}
