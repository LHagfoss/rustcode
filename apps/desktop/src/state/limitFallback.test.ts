import { describe, expect, it } from "bun:test";
import {
  type FallbackRule,
  movedNotice,
  parseRules,
  pickFallback,
  removeRule,
  ruleError,
  ruleSummary,
  rulesJson,
  saveRule,
  switchText,
} from "./limitFallback";

const opus: FallbackRule = {
  from: { harness: "claude-code", model: "opus" },
  to: { harness: "codex", model: "gpt-5.5", effort: "high" },
};
const anyClaude: FallbackRule = { from: { harness: "claude-code" }, to: { harness: "codex" } };
const codex: FallbackRule = { from: { harness: "codex" }, to: { harness: "claude-code", model: "sonnet" } };

describe("the saved table", () => {
  it("skips rows that name no harness on either side", () => {
    const rules = parseRules([
      { from: { harness: " claude-code ", model: "" }, to: { harness: "codex", model: "gpt-5.5" } },
      { from: { harness: "codex" }, to: {} },
      { from: "x", to: { harness: "codex" } },
      "nope",
    ]);
    expect(rules).toEqual([{ from: { harness: "claude-code" }, to: { harness: "codex", model: "gpt-5.5" } }]);
    expect(parseRules(undefined)).toEqual([]);
  });

  it("round-trips through the stored form without empty fields", () => {
    expect(parseRules(rulesJson([opus, anyClaude]))).toEqual([opus, anyClaude]);
  });

  it("edits rows by what they start from", () => {
    expect(saveRule([opus], anyClaude)).toEqual([opus, anyClaude]);
    const edited = { ...opus, to: { harness: "gemini" } };
    expect(saveRule([opus, codex], edited, opus.from)).toEqual([edited, codex]);
    expect(removeRule([opus, codex], opus.from)).toEqual([codex]);
  });

  it("refuses a row that goes nowhere or repeats one", () => {
    expect(ruleError([], { from: { harness: "codex" }, to: { harness: "codex" } })).toMatch(/same harness/);
    expect(ruleError([], { from: { harness: "" }, to: { harness: "codex" } })).toMatch(/hits the limit/);
    expect(ruleError([opus], { ...opus, to: { harness: "gemini" } })).toMatch(/already a row/);
    expect(ruleError([opus], { ...opus, to: { harness: "gemini" } }, opus.from)).toBeNull();
  });

  it("says a row in a line", () => {
    expect(ruleSummary(opus)).toBe("claude-code · opus → codex · gpt-5.5 · high");
    expect(ruleSummary(anyClaude)).toBe("claude-code · any model → codex");
  });
});

describe("picking where a limited thread goes", () => {
  it("prefers the row for the thread's model, else the harness's own row", () => {
    expect(pickFallback([anyClaude, opus], { harness: "claude-code", model: "claude-opus-5-5" })).toBe(opus);
    expect(pickFallback([anyClaude, opus], { harness: "claude-code", model: "claude-haiku-5-5" })).toBe(
      anyClaude,
    );
    expect(pickFallback([opus], { harness: "claude-code", model: "claude-haiku-5-5" })).toBeNull();
  });

  it("finds nothing for a harness no row starts from", () => {
    expect(pickFallback([opus, codex], { harness: "gemini" })).toBeNull();
    expect(pickFallback([opus], { harness: undefined })).toBeNull();
  });

  it("never sends a thread back to a harness it already left on a limit", () => {
    expect(pickFallback([codex], { harness: "codex" }, new Set(["claude-code"]))).toBeNull();
    expect(pickFallback([codex], { harness: "codex" }, new Set(["gemini"]))).toBe(codex);
  });
});

describe("what the thread says", () => {
  it("names the harness it left and the one it moved to", () => {
    expect(movedNotice("Claude Code", "Codex · gpt-5.5")).toBe(
      "Moved to Codex · gpt-5.5 because Claude Code hit its usage limit.",
    );
    expect(switchText("Claude Code", "Codex")).toContain("Continue where you left off");
  });
});
