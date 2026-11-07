import { describe, expect, test } from "bun:test";
import {
  AUTO_COMPACT_CACHE_IDLE_MS,
  AUTO_COMPACT_CACHE_THRESHOLD,
  AUTO_COMPACT_DEFAULT_THRESHOLD,
  AUTO_COMPACT_FOLLOW_UP_MS,
  type AutoCompactInput,
  autoCompactDecision,
  autoCompactMark,
  autoCompactPolicy,
  automaticCompactionText,
  clampAutoCompactThreshold,
  DEFAULT_AUTO_COMPACT,
  parseAutoCompactSettings,
} from "./autoCompact";

const NOW = 1_800_000_000_000;
const input = (over: Partial<AutoCompactInput> = {}): AutoCompactInput => ({
  settings: DEFAULT_AUTO_COMPACT,
  policy: autoCompactPolicy("claude-code", "acp"),
  canCompact: true,
  usedTokens: 20_000,
  now: NOW,
  idleSince: NOW - 1_000,
  status: "idle",
  running: false,
  queued: 0,
  approvals: 0,
  typing: false,
  compactedThisIdle: false,
  delegated: false,
  orchestratorTalking: false,
  ...over,
});

describe("auto-compact settings", () => {
  test("a fresh install is on for Claude Code's two rules and 150k", () => {
    expect(DEFAULT_AUTO_COMPACT).toEqual({ mode: "cache", threshold: 150_000 });
    expect(AUTO_COMPACT_DEFAULT_THRESHOLD).toBe(150_000);
  });

  test("the stored mode and threshold are read back, an older string too", () => {
    expect(parseAutoCompactSettings(null)).toEqual(DEFAULT_AUTO_COMPACT);
    expect(parseAutoCompactSettings('{"mode":"size","threshold":90000}')).toEqual({
      mode: "size",
      threshold: 90_000,
    });
    expect(parseAutoCompactSettings('"off"')).toEqual({ ...DEFAULT_AUTO_COMPACT, mode: "off" });
    expect(parseAutoCompactSettings("nonsense")).toEqual(DEFAULT_AUTO_COMPACT);
  });

  test("the threshold stays in its range", () => {
    expect(clampAutoCompactThreshold(1)).toBe(20_000);
    expect(clampAutoCompactThreshold(9_000_000)).toBe(500_000);
    expect(clampAutoCompactThreshold(Number.NaN)).toBe(150_000);
  });

  test("per-harness defaults: Claude both rules, Codex only the cache rule, the built-in none", () => {
    expect(autoCompactPolicy("claude-code", "acp")).toEqual({ size: true, cache: true });
    expect(autoCompactPolicy("codex", "codex")).toEqual({ size: false, cache: true });
    expect(autoCompactPolicy("rustcode", "builtin")).toEqual({ size: false, cache: false });
    expect(autoCompactPolicy("gemini", "acp")).toEqual({ size: true, cache: false });
  });
});

describe("rule 1: the context size after a turn", () => {
  test("compacts at once at the threshold and never below it", () => {
    expect(autoCompactDecision(input({ usedTokens: 149_999 })).action).toBe("wait");
    expect(autoCompactDecision(input({ usedTokens: 150_000 }))).toEqual({
      action: "compact",
      rule: "size",
      before: 150_000,
    });
  });

  test("does not fire while a turn runs, a message waits or an approval is open", () => {
    expect(autoCompactDecision(input({ running: true }))).toMatchObject({
      action: "none",
      reason: "turn running",
    });
    expect(autoCompactDecision(input({ status: "running" }))).toMatchObject({
      action: "none",
      reason: "turn running",
    });
    expect(autoCompactDecision(input({ queued: 1, usedTokens: 200_000 }))).toMatchObject({
      action: "none",
      reason: "message queued",
    });
    expect(autoCompactDecision(input({ approvals: 1, usedTokens: 200_000 }))).toMatchObject({
      action: "none",
      reason: "approval pending",
    });
    expect(autoCompactDecision(input({ status: "needs_input", usedTokens: 200_000 }))).toMatchObject({
      action: "none",
      reason: "approval pending",
    });
  });

  test("stops for the user, a settled thread, a delegate nobody talks to, and a second time", () => {
    expect(autoCompactDecision(input({ typing: true, usedTokens: 200_000 }))).toMatchObject({
      action: "none",
      reason: "user typing",
    });
    expect(autoCompactDecision(input({ status: "done", usedTokens: 200_000 }))).toMatchObject({
      action: "none",
      reason: "thread settled",
    });
    expect(autoCompactDecision(input({ delegated: true, usedTokens: 200_000 }))).toMatchObject({
      action: "none",
      reason: "orchestrator is not talking to it",
    });
    expect(
      autoCompactDecision(input({ delegated: true, orchestratorTalking: true, usedTokens: 200_000 })).action,
    ).toBe("compact");
    expect(autoCompactDecision(input({ compactedThisIdle: true, usedTokens: 200_000 }))).toMatchObject({
      action: "none",
      reason: "already compacted this idle period",
    });
  });

  test("a harness without compaction is skipped", () => {
    expect(autoCompactDecision(input({ canCompact: false, usedTokens: 500_000 }))).toMatchObject({
      action: "none",
      reason: "harness cannot compact",
    });
    expect(autoCompactDecision(input({ policy: { size: false, cache: false } }))).toMatchObject({
      action: "none",
      reason: "nothing to do on this harness",
    });
  });
});

describe("rule 2: before the prompt cache expires", () => {
  test("waits until 4.5 minutes after the turn and then compacts above 60k", () => {
    const idleSince = NOW;
    const early = autoCompactDecision(input({ idleSince, now: idleSince + 60_000, usedTokens: 80_000 }));
    expect(early).toEqual({
      action: "wait",
      rule: "cache",
      at: idleSince + AUTO_COMPACT_CACHE_IDLE_MS,
      before: 80_000,
    });
    expect(
      autoCompactDecision(
        input({ idleSince, now: idleSince + AUTO_COMPACT_CACHE_IDLE_MS, usedTokens: 80_000 }),
      ),
    ).toEqual({ action: "compact", rule: "cache", before: 80_000 });
  });

  test("the cache threshold is the smaller 60k even when the size threshold is larger", () => {
    const idleSince = NOW;
    expect(
      autoCompactDecision(
        input({ idleSince, now: idleSince + AUTO_COMPACT_CACHE_IDLE_MS, usedTokens: 59_999 }),
      ).action,
    ).toBe("none");
    expect(AUTO_COMPACT_CACHE_THRESHOLD).toBe(60_000);
  });

  test("a thread nobody followed up for an hour does not pay for the summary", () => {
    const idleSince = NOW - AUTO_COMPACT_FOLLOW_UP_MS - 1;
    expect(autoCompactDecision(input({ idleSince, now: NOW, usedTokens: 80_000 }))).toMatchObject({
      action: "none",
      reason: "no follow-up within the hour",
    });
  });

  test("the size-only mode never waits for the cache", () => {
    expect(
      autoCompactDecision(
        input({
          settings: { mode: "size", threshold: 150_000 },
          usedTokens: 80_000,
          idleSince: NOW,
          now: NOW + 60_000,
        }),
      ).action,
    ).toBe("none");
  });

  test("Codex gets the cache rule but not the size rule", () => {
    const policy = autoCompactPolicy("codex", "codex");
    expect(
      autoCompactDecision(input({ policy, usedTokens: 200_000, idleSince: NOW, now: NOW + 60_000 })).action,
    ).toBe("wait");
    expect(
      autoCompactDecision(input({ policy, usedTokens: 80_000, idleSince: NOW, now: NOW + 60_000 })).action,
    ).toBe("wait");
  });
});

describe("what the thread shows", () => {
  test("an automatic compaction names itself", () => {
    expect(automaticCompactionText("🧹 History compacted: reduced context from 161495 to 4200 tokens.")).toBe(
      "Compacted automatically: reduced context from 161495 to 4200 tokens.",
    );
    expect(
      automaticCompactionText(
        "Context compacted: Codex summarized the conversation to free the context window.",
      ),
    ).toBe("Compacted automatically: Codex summarized the conversation to free the context window.");
    expect(automaticCompactionText("Compacted automatically.")).toBe("Compacted automatically.");
  });

  test("the context bar marks the threshold only while rule 1 applies", () => {
    const claude = autoCompactPolicy("claude-code", "acp");
    expect(autoCompactMark({ mode: "cache", threshold: 150_000 }, claude)).toBe(150_000);
    expect(autoCompactMark({ mode: "off", threshold: 150_000 }, claude)).toBeUndefined();
    expect(
      autoCompactMark({ mode: "cache", threshold: 150_000 }, autoCompactPolicy("codex", "codex")),
    ).toBeUndefined();
  });
});
