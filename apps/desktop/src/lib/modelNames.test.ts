import { describe, expect, test } from "bun:test";
import { catalogModelTitle, modelTitle } from "./modelNames";

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

  test("cleans up known GPT and Gemini ids", () => {
    expect(modelTitle("gpt-6.1-terra")).toBe("GPT 6.1 Terra");
    expect(modelTitle("Gpt-6.2-sol")).toBe("GPT 6.2 Sol");
    expect(modelTitle("gemini-4-neon")).toBe("Gemini 4 Neon");
    expect(modelTitle("gemini-3.8-flash-lite")).toBe("Gemini 3.8 Flash-Lite");
    expect(modelTitle("gpt-6.1-codex-mini")).toBe("GPT 6.1 Codex Mini");
    expect(modelTitle("gemini-4-flash-lite")).toBe("Gemini 4 Flash-Lite");
    expect(modelTitle("claude-opus-5-5-thinking")).toBe("Opus 5.5 Thinking");
  });

  test("prefers a catalog name and strips provider prefixes", () => {
    const harness = {
      id: "opencode",
      models: [
        { id: "default", name: "Default (recommended)" },
        { id: "openai/gpt-6.1-terra", name: "gpt-6.1-terra" },
      ],
    };
    expect(catalogModelTitle("openai/gpt-6.1-terra", harness)).toBe("GPT 6.1 Terra");
    expect(catalogModelTitle("anthropic/claude-opus-5-5", harness)).toBe("Opus 5.5");
    expect(catalogModelTitle(undefined, harness)).toBeUndefined();
  });

  test("names DeepSeek ids with the family's casing", () => {
    expect(modelTitle("deepseek-v4.1-flash")).toBe("DeepSeek V4.1 Flash");
    expect(modelTitle("deepseek-flash")).toBe("DeepSeek Flash");
    expect(modelTitle("deepseek-chat")).toBe("DeepSeek Chat");
  });

  test("keeps other names and meaningful hyphens", () => {
    expect(modelTitle("gpt-oss-120b")).toBe("gpt-oss-120b");
    expect(modelTitle("default")).toBe("default");
  });
});
