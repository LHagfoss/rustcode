import { describe, expect, test } from "bun:test";
import { modelMaker } from "./ModelIcon";

describe("a model's maker", () => {
  test("follows the family in its display name", () => {
    expect(modelMaker("Opus 5.5")).toBe("claude");
    expect(modelMaker("Sonnet 5")).toBe("claude");
    expect(modelMaker("GPT 6.1 Sol")).toBe("codex");
    expect(modelMaker("Gemini 3 Pro")).toBe("gemini");
  });

  test("leaves other models to their own letter", () => {
    expect(modelMaker("DeepSeek V4.1 Flash")).toBe("deepseek v4.1 flash");
  });
});
