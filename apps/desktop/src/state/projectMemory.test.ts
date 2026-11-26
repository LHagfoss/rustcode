import { describe, expect, test } from "bun:test";
import { MockMemory } from "../acp/mockMemory";
import { parseMemoryListing, withoutFact } from "./projectMemory";

describe("project memory", () => {
  test("the listing reads project and global facts, with tentative ones marked", () => {
    const mock = new MockMemory();
    const listing = parseMemoryListing(mock.answer("_rustcode/memory/list", { cwd: "/repo" }));
    expect(listing.project?.facts.map((f) => f.key)).toEqual(["test-runner", "commits", "vps-builds"]);
    expect(listing.project?.facts.find((f) => f.key === "vps-builds")?.tentative).toBe(true);
    expect(listing.global.facts).toHaveLength(1);
    expect(listing.project?.facts[0]?.updatedAt).toBeGreaterThan(1e12);
    expect(parseMemoryListing(mock.answer("_rustcode/memory/list", {})).project).toBeNull();
  });

  test("forget removes one fact, and forget all empties only the project", () => {
    const mock = new MockMemory();
    const listing = parseMemoryListing(mock.answer("_rustcode/memory/list", { cwd: "/repo" }));
    const fact = listing.project!.facts[0]!;
    expect(
      mock.answer("_rustcode/memory/forget", { cwd: "/repo", category: fact.category, key: fact.key }),
    ).toEqual({
      removed: 1,
    });
    expect(withoutFact(listing, fact, false).project?.facts).toHaveLength(2);
    mock.answer("_rustcode/memory/forget", { cwd: "/repo", all: true });
    const after = parseMemoryListing(mock.answer("_rustcode/memory/list", { cwd: "/repo" }));
    expect(after.project?.facts).toEqual([]);
    expect(after.global.facts).toHaveLength(1);
  });

  test("a malformed answer reads as no facts", () => {
    expect(parseMemoryListing(null)).toEqual({ project: null, global: { path: "", facts: [] } });
    expect(parseMemoryListing({ global: { facts: [{ value: "no key" }, 3] } }).global.facts).toEqual([]);
  });
});
