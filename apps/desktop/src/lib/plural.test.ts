import { describe, expect, test } from "bun:test";
import { count } from "./plural";

describe("a count and its noun", () => {
  test("one is singular, everything else is plural", () => {
    expect(count(0, "turn")).toBe("0 turns");
    expect(count(1, "turn")).toBe("1 turn");
    expect(count(2, "turn")).toBe("2 turns");
  });

  test("an irregular plural is given, not guessed", () => {
    expect(count(1, "day", "days")).toBe("1 day");
    expect(count(3, "delegated thread")).toBe("3 delegated threads");
  });
});
