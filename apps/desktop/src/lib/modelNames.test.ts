import { describe, expect, test } from "bun:test";
import { modelTitle } from "./modelNames";

describe("modelTitle", () => {
  test("reads Claude ids and aliases as their family and version", () => {
    expect(modelTitle("opus")).toBe("Opus 5.5");
    expect(modelTitle("claude-opus-5-5")).toBe("Opus 5.5");
    expect(modelTitle("claude-sonnet-5.5")).toBe("Sonnet 5.5");
    expect(modelTitle("claude-fable-5-2")).toBe("Fable 5.2");
    expect(modelTitle("claude-opus-4-8-20260101")).toBe("Opus 4.8");
    expect(modelTitle("claude-opus-5")).toBe("Opus 5");
    expect(modelTitle("opus[1m]")).toBe("Opus 5.5 1M");
  });

  test("leaves every other model as it is", () => {
    expect(modelTitle("gpt-6.1-terra")).toBe("gpt-6.1-terra");
    expect(modelTitle("deepseek-v4.1-flash")).toBe("deepseek-v4.1-flash");
    expect(modelTitle("default")).toBe("default");
  });
});
