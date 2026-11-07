import { describe, expect, test } from "bun:test";
import { turnActions, turnFooterHidden } from "./replyActions";
import { estimatedRowHeight } from "./Transcript";

const at = 1_776_758_100_000;

describe("a finished turn's action row", () => {
  test("shows copy, fork and the time", () => {
    expect(turnActions({ text: "Done **it**.", at, threadId: "t1" })).toEqual({
      row: true,
      copy: "Done **it**.",
      fork: true,
      time: true,
    });
  });

  test("copies the raw Markdown, not the rendered text", () => {
    expect(turnActions({ text: "`a` > b", at }).copy).toBe("`a` > b");
  });

  test("an empty reply has no copy, no fork, and keeps only its time", () => {
    const empty = turnActions({ text: "  \n", at, threadId: "t1" });
    expect(empty.copy).toBeNull();
    expect(empty.fork).toBe(false);
    expect(empty.time).toBe(true);
    // Nothing to copy and no time either: no row at all, and no room held for one.
    expect(turnActions({})).toEqual({ row: false, copy: null, fork: false, time: false });
    expect(turnActions({ text: "" })).toEqual({ row: false, copy: null, fork: false, time: false });
  });

  test("a reply that is only whitespace gets no copy button", () => {
    expect(turnActions({ text: " \n ", at, threadId: "t1" }).copy).toBeNull();
  });

  test("a row without a thread still shows its text, and no fork", () => {
    expect(turnActions({ text: "Done.", at })).toEqual({
      row: true,
      copy: "Done.",
      fork: false,
      time: true,
    });
  });
});

describe("a finished turn's footer that adds nothing", () => {
  test("a folded turn with no answer draws no footer, so its row takes no room", () => {
    expect(turnFooterHidden({ folded: true, reply: "", outcome: "completed" })).toBe(true);
    expect(turnFooterHidden({ folded: true, reply: "  \n", outcome: "completed" })).toBe(true);
    expect(turnFooterHidden({ folded: true, outcome: "completed" })).toBe(true);
  });

  test("an answer, a completion card, a changes chip or a Trace keeps the footer", () => {
    expect(turnFooterHidden({ folded: true, reply: "Done.", outcome: "completed" })).toBe(false);
    expect(turnFooterHidden({ folded: true, completion: true, outcome: "completed" })).toBe(false);
    expect(turnFooterHidden({ folded: true, changes: true, outcome: "completed" })).toBe(false);
    expect(turnFooterHidden({ folded: true, trace: true, outcome: "completed" })).toBe(false);
  });

  test("a turn that failed or was stopped always says so", () => {
    expect(turnFooterHidden({ folded: true, outcome: "failed" })).toBe(false);
    expect(turnFooterHidden({ folded: true, outcome: "cancelled" })).toBe(false);
  });

  test("a turn with no fold keeps its footer: the time is all it has", () => {
    expect(turnFooterHidden({ outcome: "completed" })).toBe(false);
  });
});

describe("the height a row starts at", () => {
  const at = 1_776_758_100_000;
  const footer = { key: "e1", item: { kind: "turnEnd", id: "e1", at, outcome: "completed" } } as never;

  test("a turn footer estimates its own 26px", () => {
    expect(estimatedRowHeight(footer)).toBe(26);
  });

  test("a footer that draws nothing estimates 0, so nothing below it drifts", () => {
    expect(estimatedRowHeight(footer, true)).toBe(0);
    expect(estimatedRowHeight(undefined)).toBe(24);
  });
});
