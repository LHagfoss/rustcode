import { describe, expect, test } from "bun:test";
import { NEW_THREAD_TIPS, tipBinding } from "./newThreadTips";

describe("new thread tips", () => {
  test("has distinct, non-empty tips", () => {
    expect(NEW_THREAD_TIPS.every((tip) => tip.text.trim() && tip.command.trim())).toBe(true);
    expect(new Set(NEW_THREAD_TIPS.map((tip) => tip.text)).size).toBe(NEW_THREAD_TIPS.length);
  });
  test("formats platform modifiers", () => {
    expect(tipBinding("mod+k", "mac")).toBe("⌘K");
    expect(tipBinding("mod+k", "windows")).toBe("Ctrl+K");
    expect(tipBinding("mod+k", "linux")).toBe("Ctrl+K");
  });
});
