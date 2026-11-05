import { describe, expect, test } from "bun:test";
import {
  DEFAULT_HEADER_BUTTONS,
  HEADER_BUTTON_IDS,
  type HeaderButtonId,
  insertIndex,
  sameHeaderButtons,
  sanitizeHeaderButtons,
} from "./headerButtons";

const ids = (...list: string[]) => list as HeaderButtonId[];

describe("header buttons sanitising", () => {
  test("nothing stored shows every button", () => {
    for (const raw of [null, undefined, "", "{not json", 42, [], {}]) {
      expect(sanitizeHeaderButtons(raw)).toEqual([...DEFAULT_HEADER_BUTTONS]);
    }
  });

  test("a saved order is kept as it is", () => {
    const shown = ids("changes", "status", "terminal");
    expect(sanitizeHeaderButtons({ shown, known: [...HEADER_BUTTON_IDS] })).toEqual(shown);
  });

  test("unknown ids and duplicates are dropped", () => {
    expect(
      sanitizeHeaderButtons({
        shown: ["nope", "plan", "plan", 7, "plan", "changes"],
        known: [...HEADER_BUTTON_IDS],
      }),
    ).toEqual(ids("plan", "changes"));
  });

  test("every button hidden is a valid choice", () => {
    expect(sanitizeHeaderButtons({ shown: [], known: [...HEADER_BUTTON_IDS] })).toEqual([]);
  });

  test("a button added in a later release shows by default, after the arranged ones", () => {
    // Saved before `details` and `changes` existed.
    const stored = { shown: ids("terminal", "plan"), known: ids("status", "plan", "pulls", "terminal") };
    expect(sanitizeHeaderButtons(stored)).toEqual(ids("terminal", "plan", "details", "changes"));
  });

  test("a button the user hid stays hidden", () => {
    const stored = { shown: ids("status"), known: [...HEADER_BUTTON_IDS] };
    expect(sanitizeHeaderButtons(stored)).toEqual(ids("status"));
  });

  test("a value saved without a known list re-adds nothing", () => {
    expect(sanitizeHeaderButtons({ shown: ["status"] })).toEqual(ids("status"));
  });

  test("a known list that is not a list of ids is ignored", () => {
    expect(sanitizeHeaderButtons({ shown: ids("plan"), known: "status" })).toEqual(ids("plan"));
  });
});

describe("comparing orders", () => {
  test("same ids in the same order are equal", () => {
    expect(sameHeaderButtons(ids("status", "plan"), ids("status", "plan"))).toBe(true);
    expect(sameHeaderButtons([], [])).toBe(true);
    expect(sameHeaderButtons([], [...DEFAULT_HEADER_BUTTONS])).toBe(false);
  });

  test("a move makes the order different", () => {
    expect(sameHeaderButtons(ids("plan", "status"), ids("status", "plan"))).toBe(false);
  });
});

describe("the drop position", () => {
  const rows = [
    { top: 100, height: 32 },
    { top: 132, height: 32 },
    { top: 164, height: 32 },
  ];

  test("an empty list takes it at the start", () => {
    expect(insertIndex([], 500)).toBe(0);
  });

  test("above every row is the start, below every row is the end", () => {
    expect(insertIndex(rows, 0)).toBe(0);
    expect(insertIndex(rows, 110)).toBe(0);
    expect(insertIndex(rows, 900)).toBe(3);
  });

  test("past a row's middle is past the row", () => {
    // The middles are 116, 148 and 180.
    expect(insertIndex(rows, 130)).toBe(1);
    expect(insertIndex(rows, 160)).toBe(2);
    expect(insertIndex(rows, 181)).toBe(3);
  });

  test("exactly on a middle stays before that row", () => {
    expect(insertIndex(rows, 148)).toBe(1);
  });
});
