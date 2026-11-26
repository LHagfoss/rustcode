import type { Json } from "../acp/jsonrpc";

/** One fact an agent saved with `remember`, as `_rustcode/memory/list` answers it. */
export interface MemoryFact {
  category: string;
  key: string;
  value: string;
  source: string;
  /** Low confidence: a guess the engine drops after a week unless it is saved again. */
  tentative: boolean;
  /** Milliseconds since the epoch. */
  updatedAt: number;
}

export interface MemoryFile {
  path: string;
  facts: MemoryFact[];
}

export interface MemoryListing {
  /** Absent when no project is chosen. */
  project: MemoryFile | null;
  global: MemoryFile;
}

const text = (value: Json | undefined) => (typeof value === "string" ? value : "");

function fileOf(value: Json | undefined): MemoryFile | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const facts = Array.isArray(value.facts) ? value.facts : [];
  return {
    path: text(value.path),
    facts: facts.flatMap((row) => {
      if (!row || typeof row !== "object" || Array.isArray(row)) return [];
      const key = text(row.key);
      if (!key) return [];
      return [
        {
          category: text(row.category),
          key,
          value: text(row.value),
          source: text(row.source),
          tentative: row.tentative === true,
          updatedAt: typeof row.updatedAt === "number" ? row.updatedAt * 1000 : 0,
        },
      ];
    }),
  };
}

/** Read `_rustcode/memory/list`'s answer; anything malformed reads as no facts. */
export function parseMemoryListing(answer: Json): MemoryListing {
  const object = answer && typeof answer === "object" && !Array.isArray(answer) ? answer : {};
  return {
    project: fileOf(object.project),
    global: fileOf(object.global) ?? { path: "", facts: [] },
  };
}

/** The listing without one fact, as it reads after Forget succeeds. */
export function withoutFact(listing: MemoryListing, fact: MemoryFact, global: boolean): MemoryListing {
  const drop = (file: MemoryFile): MemoryFile => ({
    ...file,
    facts: file.facts.filter((f) => !(f.category === fact.category && f.key === fact.key)),
  });
  if (global) return { ...listing, global: drop(listing.global) };
  return { ...listing, project: listing.project && drop(listing.project) };
}
