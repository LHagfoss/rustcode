import { expect, test } from "bun:test";
import { formatTokens } from "./threadFormat";

test("token counts past a billion read in billions, not thousands of millions", () => {
  expect(formatTokens(1_165_000_000)).toBe("1.2B");
  expect(formatTokens(12_400_000_000)).toBe("12B");
  expect(formatTokens(950_000_000)).toBe("950M");
  expect(formatTokens(4_200)).toBe("4k");
});
