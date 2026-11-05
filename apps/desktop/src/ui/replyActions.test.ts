import { describe, expect, test } from "bun:test";
import { replyActions, turnFooterHidden } from "./replyActions";
import { estimatedRowHeight } from "./Transcript";

const at = 1_776_758_100_000;

describe("the action row under a reply", () => {
  test("shows copy, fork and the time", () => {
    expect(replyActions({ where: "reply", text: "Done **it**.", at, threadId: "t1" })).toEqual({
      row: true,
      copy: "Done **it**.",
      fork: true,
      time: true,
    });
  });

  test("copies the raw Markdown, not the rendered text", () => {
    expect(replyActions({ where: "reply", text: "`a` > b", at }).copy).toBe("`a` > b");
  });

  test("an empty reply has no copy, and no row when it has no time either", () => {
    const empty = replyActions({ where: "reply", text: "  \n", at });
    expect(empty.copy).toBeNull();
    expect(empty.row).toBe(true); // it still has a time to show
    expect(replyActions({ where: "reply", text: "" })).toEqual({
      row: false,
      copy: null,
      fork: false,
      time: false,
    });
  });

  test("a streaming reply keeps its row, with copy but no fork", () => {
    expect(replyActions({ where: "reply", text: "Writing…", at, threadId: "t1", streaming: true })).toEqual({
      row: true,
      copy: "Writing…",
      fork: false,
      time: true,
    });
  });

  test("an earlier reply in the turn keeps its row", () => {
    expect(replyActions({ where: "reply", text: "One", at, threadId: "t1", lastInTurn: false }).row).toBe(
      true,
    );
  });

  test("the last reply of a finished turn draws no row: its footer has the one", () => {
    expect(replyActions({ where: "reply", text: "Done.", at, threadId: "t1", lastInTurn: true })).toEqual({
      row: false,
      copy: null,
      fork: false,
      time: false,
    });
  });
});

describe("the action row in a finished turn's footer", () => {
  test("shows copy, fork and the time, folded or not", () => {
    expect(replyActions({ where: "turnEnd", text: "Done.", at, threadId: "t1" })).toEqual({
      row: true,
      copy: "Done.",
      fork: true,
      time: true,
    });
  });

  test("a turn with no reply before it shows only its own time", () => {
    // Nothing to copy, so nothing to fork either: the row is the time alone.
    expect(replyActions({ where: "turnEnd", at, threadId: "t1" })).toEqual({
      row: true,
      copy: null,
      fork: false,
      time: true,
    });
  });

  test("a reply that is only whitespace gets no copy button", () => {
    expect(replyActions({ where: "turnEnd", text: " \n ", at, threadId: "t1" }).copy).toBeNull();
  });

  test("a footer without a thread still shows its text, and no fork", () => {
    expect(replyActions({ where: "turnEnd", text: "Done.", at })).toEqual({
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

  test("an answer, a completion card or a changes chip keeps the footer", () => {
    expect(turnFooterHidden({ folded: true, reply: "Done.", outcome: "completed" })).toBe(false);
    expect(turnFooterHidden({ folded: true, completion: true, outcome: "completed" })).toBe(false);
    expect(turnFooterHidden({ folded: true, changes: true, outcome: "completed" })).toBe(false);
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
