import { describe, expect, test } from "bun:test";
import { GRID, PATTERNS, splitWorkingLabel, turnFlavor, WORKING_WORDS } from "./workingMotion";

describe("working indicator", () => {
  test("every pattern lights cells inside the grid and keeps moving", () => {
    for (const pattern of Object.values(PATTERNS)) {
      let dark = 0;
      for (let step = 0; step < 200; step++) {
        const lit = pattern(step);
        if (!lit.length) dark++;
        for (const { cell, level } of lit) {
          expect(cell).toBeGreaterThanOrEqual(0);
          expect(cell).toBeLessThan(GRID * GRID);
          expect(level).toBeGreaterThan(0);
          expect(level).toBeLessThanOrEqual(1);
        }
      }
      expect(dark).toBeLessThan(20);
    }
  });

  test("a turn keeps one word and pattern, and turns differ", () => {
    expect(turnFlavor(1700000000000)).toEqual(turnFlavor(1700000000000));
    const words = new Set(Array.from({ length: 40 }, (_, i) => turnFlavor(1700000000000 + i * 977).word));
    expect(words.size).toBeGreaterThan(5);
    for (const word of words) expect(WORKING_WORDS).toContain(word as never);
  });

  test("the label splits into one word and the step's detail", () => {
    expect(splitWorkingLabel("Working", "Churning")).toEqual({ word: "Churning", detail: "" });
    expect(splitWorkingLabel("Thinking · Plan the fix", "Churning")).toEqual({
      word: "Thinking",
      detail: "Plan the fix",
    });
    expect(splitWorkingLabel("Writing the reply", "Churning")).toEqual({ word: "Writing", detail: "" });
    expect(splitWorkingLabel("Read Composer.tsx", "Brewing")).toEqual({
      word: "Brewing",
      detail: "Read Composer.tsx",
    });
  });
});
