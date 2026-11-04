import { describe, expect, test } from "bun:test";
import { fuzzyScore, matchModels } from "./modelSearch";

const models = [
  { id: "claude-sonnet-4-5", name: "Claude Sonnet 4.5" },
  { id: "claude-opus-4-1", name: "Claude Opus 4.1" },
  { id: "gpt-6-astra", name: "Astra" },
  { id: "gpt-6-luna", name: "Luna" },
  { id: "gemini-3-pro", name: "Gemini 3 Pro" },
];

describe("fuzzyScore", () => {
  test("every letter, in order, is needed", () => {
    expect(fuzzyScore("son", "Claude Sonnet 4.5")).not.toBeNull();
    expect(fuzzyScore("nos", "Claude Sonnet 4.5")).toBeNull();
  });

  test("a whole substring beats letters scattered through the name", () => {
    const direct = fuzzyScore("luna", "Luna")!;
    const scattered = fuzzyScore("luna", "Lovely Unnamed Night Atlas")!;
    expect(direct).toBeGreaterThan(scattered);
  });

  test("a hit at the start beats one later in the name", () => {
    expect(fuzzyScore("astra", "Astra")!).toBeGreaterThan(fuzzyScore("astra", "Gpt Astra")!);
  });

  test("case does not matter, nor extra spaces in the query", () => {
    expect(fuzzyScore("CLAUDE  sonnet", "Claude Sonnet 4.5")).not.toBeNull();
  });

  test("an empty query matches everything", () => {
    expect(fuzzyScore("  ", "anything")).toBe(0);
  });
});

describe("matchModels", () => {
  test("no query keeps the list in order and counts it", () => {
    const { shown, total } = matchModels("", models);
    expect(shown).toEqual(models);
    expect(total).toBe(5);
  });

  test("matches a name, not only the id", () => {
    const { shown } = matchModels("gemini", models);
    expect(shown.map((m) => m.id)).toEqual(["gemini-3-pro"]);
  });

  test("matches an id fragment the name does not spell", () => {
    const { shown } = matchModels("astra", models);
    expect(shown.map((m) => m.id)).toEqual(["gpt-6-astra"]);
  });

  test("a squeezed subsequence still finds the model", () => {
    const { shown } = matchModels("cso4", models);
    expect(shown[0]?.id).toBe("claude-sonnet-4-5");
  });

  test("the closest match comes first", () => {
    const { shown } = matchModels("luna", models);
    expect(shown[0]?.id).toBe("gpt-6-luna");
  });

  test("nothing matches nothing", () => {
    const { shown, total } = matchModels("zzz", models);
    expect(shown).toEqual([]);
    expect(total).toBe(0);
  });

  test("the window is capped but the count is not", () => {
    const many = Array.from({ length: 800 }, (_, i) => ({ id: `m-${i}`, name: `Model ${i}` }));
    const { shown, total } = matchModels("", many, 60);
    expect(shown.length).toBe(60);
    expect(total).toBe(800);
    const q = matchModels("model", many, 60);
    expect(q.shown.length).toBe(60);
    expect(q.total).toBe(800);
  });
});
