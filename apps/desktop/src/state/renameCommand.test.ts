import { describe, expect, test } from "bun:test";
import { allowAgentRename, parseRename, takeAgentRename } from "./renameCommand";

describe("/rename", () => {
  test("a bare /rename asks the agent; one with a name renames at once", () => {
    expect(parseRename("/rename")).toEqual({});
    expect(parseRename("  /rename  ")).toEqual({});
    expect(parseRename("/rename Fix the login page")).toEqual({ title: "Fix the login page" });
    expect(parseRename("/renamed")).toBeNull();
    expect(parseRename("please /rename")).toBeNull();
  });

  test("the agent's title answers a /rename once, and only for a while", () => {
    expect(takeAgentRename("t1")).toBe(false);
    allowAgentRename("t1", 0);
    expect(takeAgentRename("t1", 1_000)).toBe(true);
    expect(takeAgentRename("t1", 2_000)).toBe(false);
    allowAgentRename("t2", 0);
    expect(takeAgentRename("t2", 60 * 60_000)).toBe(false);
  });
});
