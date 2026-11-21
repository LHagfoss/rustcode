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
  test("a fresh install shows the plan and the panel's toggle", () => {
    expect(DEFAULT_HEADER_BUTTONS).toEqual(ids("plan", "details"));
    expect([...HEADER_BUTTON_IDS]).toEqual(ids("plan", "details"));
  });

  test("the removed status and git buttons drop out of a saved layout", () => {
    const shown = ids("status", "plan", "git", "details");
    const known = ids("status", "plan", "git", "details");
    expect(sanitizeHeaderButtons({ shown, known })).toEqual(ids("plan", "details"));
  });

  test("the removed terminal, project actions, Changes and Pull requests buttons drop out", () => {
    const shown = ids("terminal", "actions", "pulls", "changes", "details");
    const known = [...HEADER_BUTTON_IDS, ...ids("actions", "terminal", "pulls", "changes")];
    expect(sanitizeHeaderButtons({ shown, known })).toEqual(ids("details"));
    // Saved before `known` existed: nothing is re-added.
    expect(sanitizeHeaderButtons({ shown: ids("pulls", "plan") })).toEqual(ids("plan"));
  });

  test("nothing stored shows the default buttons", () => {
    for (const raw of [null, undefined, "", "{not json", 42, [], {}]) {
      expect(sanitizeHeaderButtons(raw)).toEqual([...DEFAULT_HEADER_BUTTONS]);
    }
  });

  test("a saved order is kept as it is", () => {
    const shown = ids("details", "plan");
    expect(sanitizeHeaderButtons({ shown, known: [...HEADER_BUTTON_IDS] })).toEqual(shown);
  });

  test("unknown ids and duplicates are dropped", () => {
    expect(
      sanitizeHeaderButtons({
        shown: ["nope", "plan", "plan", 7, "plan", "changes", "details"],
        known: [...HEADER_BUTTON_IDS],
      }),
    ).toEqual(ids("plan", "details"));
  });

  test("every button hidden is a valid choice", () => {
    expect(sanitizeHeaderButtons({ shown: [], known: [...HEADER_BUTTON_IDS] })).toEqual([]);
  });

  test("a button added in a later release shows by default, after the arranged ones", () => {
    // Saved before `details` existed.
    const stored = { shown: ids("plan", "status"), known: ids("status", "plan", "pulls", "terminal") };
    expect(sanitizeHeaderButtons(stored)).toEqual(ids("plan", "details"));
  });

  test("a button the user hid stays hidden", () => {
    const stored = { shown: ids("plan"), known: [...HEADER_BUTTON_IDS] };
    expect(sanitizeHeaderButtons(stored)).toEqual(ids("plan"));
  });

  test("a value saved without a known list re-adds nothing", () => {
    expect(sanitizeHeaderButtons({ shown: ["plan"] })).toEqual(ids("plan"));
  });

  test("a known list that is not a list of ids is ignored", () => {
    expect(sanitizeHeaderButtons({ shown: ids("plan"), known: "status" })).toEqual(ids("plan"));
  });
});

describe("comparing orders", () => {
  test("same ids in the same order are equal", () => {
    expect(sameHeaderButtons(ids("details", "plan"), ids("details", "plan"))).toBe(true);
    expect(sameHeaderButtons([], [])).toBe(true);
    expect(sameHeaderButtons([], [...DEFAULT_HEADER_BUTTONS])).toBe(false);
  });

  test("a move makes the order different", () => {
    expect(sameHeaderButtons(ids("details", "plan"), ids("plan", "details"))).toBe(false);
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
