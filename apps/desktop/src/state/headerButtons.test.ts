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
  test("the hand-driven git button is hidden by default", () => {
    expect(DEFAULT_HEADER_BUTTONS).toEqual(ids("status", "plan", "details"));
    // Still a button the setting lists, so it can be shown.
    expect(HEADER_BUTTON_IDS).toContain("git");
  });

  test("the removed terminal and project actions buttons drop out of a saved layout", () => {
    const shown = ids("status", "terminal", "actions", "details");
    const known = [...HEADER_BUTTON_IDS, ...ids("actions", "terminal")];
    expect(sanitizeHeaderButtons({ shown, known })).toEqual(ids("status", "details"));
  });

  test("Changes and Pull requests left the header for the panel's tabs", () => {
    // A layout saved with them shown keeps the rest of its order.
    const shown = ids("status", "pulls", "git", "changes", "details");
    const known = [...HEADER_BUTTON_IDS, ...ids("pulls", "changes")];
    expect(sanitizeHeaderButtons({ shown, known })).toEqual(ids("status", "git", "details"));
    // Saved before `known` existed: nothing is re-added.
    expect(sanitizeHeaderButtons({ shown: ids("pulls", "plan") })).toEqual(ids("plan"));
  });

  test("a layout saved with every button keeps them, and a reset hides the git ones", () => {
    const old = [...HEADER_BUTTON_IDS];
    expect(sanitizeHeaderButtons({ shown: old, known: old })).toEqual(old);
    expect(sameHeaderButtons(old, DEFAULT_HEADER_BUTTONS)).toBe(false);
  });

  test("nothing stored shows the default buttons", () => {
    for (const raw of [null, undefined, "", "{not json", 42, [], {}]) {
      expect(sanitizeHeaderButtons(raw)).toEqual([...DEFAULT_HEADER_BUTTONS]);
    }
  });

  test("a saved order is kept as it is", () => {
    const shown = ids("details", "status", "plan");
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
    expect(sanitizeHeaderButtons(stored)).toEqual(ids("plan", "status", "git", "details"));
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
