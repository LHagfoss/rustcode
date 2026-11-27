import { expect, test } from "bun:test";
import { doublesTokenSpace } from "./tokenSpace";

test("a space typed right after a picked token is not doubled", () => {
  const before = "Compare with #Ring ";
  expect(doublesTokenSpace(before, `${before} `, before.length)).toBe(true);
  expect(doublesTokenSpace(before, `${before}a`, before.length)).toBe(false);
  expect(doublesTokenSpace(before, `${before} `, null)).toBe(false);
  expect(doublesTokenSpace("#Ring", "#Ring ", 5)).toBe(false);
});
