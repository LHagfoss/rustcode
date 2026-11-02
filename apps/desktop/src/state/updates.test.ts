import { describe, expect, test } from "bun:test";
import { newerVersion, shouldShowWhatsNew } from "./updates";

describe("updates", () => {
  test("versions compare by number, not text", () => {
    expect(newerVersion("0.116.0", "0.115.2")).toBe(true);
    expect(newerVersion("0.115.10", "0.115.9")).toBe(true);
    expect(newerVersion("v1.0.0", "0.115.0")).toBe(true);
    expect(newerVersion("0.115.0", "0.115.0")).toBe(false);
    expect(newerVersion("0.114.9", "0.115.0")).toBe(false);
  });

  test("What's New shows only after an update, not on a first install or in a dev build", () => {
    expect(shouldShowWhatsNew("0.116.0", "0.115.0", false)).toBe(true);
    expect(shouldShowWhatsNew("0.116.0", null, false)).toBe(false);
    expect(shouldShowWhatsNew("0.116.0", "0.116.0", false)).toBe(false);
    expect(shouldShowWhatsNew("0.116.0", "0.115.0", true)).toBe(false);
  });
});

describe("What's New after an in-app install", () => {
  test("shows when the install recorded where it came from, even with no version seen", () => {
    expect(shouldShowWhatsNew("1.1.0", null, false, "1.0.0")).toBe(true);
    expect(shouldShowWhatsNew("1.0.0", null, false, "1.0.0")).toBe(false);
    expect(shouldShowWhatsNew("1.1.0", null, true, "1.0.0")).toBe(false);
  });
});
