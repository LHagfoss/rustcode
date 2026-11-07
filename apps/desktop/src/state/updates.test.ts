import { describe, expect, test } from "bun:test";
import {
  compareVersions,
  installProgress,
  isPrerelease,
  newerVersion,
  readChannel,
  shouldShowWhatsNew,
} from "./updates";

describe("updates", () => {
  test("versions compare by number, not text", () => {
    expect(newerVersion("0.116.0", "0.115.2")).toBe(true);
    expect(newerVersion("0.115.10", "0.115.9")).toBe(true);
    expect(newerVersion("v1.0.0", "0.115.0")).toBe(true);
    expect(newerVersion("0.115.0", "0.115.0")).toBe(false);
    expect(newerVersion("0.114.9", "0.115.0")).toBe(false);
  });

  test("a beta is older than its release, and later betas are newer", () => {
    expect(newerVersion("1.21.0-beta.2", "1.21.0-beta.1")).toBe(true);
    expect(newerVersion("1.21.0-beta.10", "1.21.0-beta.9")).toBe(true);
    expect(newerVersion("1.21.0", "1.21.0-beta.2")).toBe(true);
    expect(newerVersion("1.21.0-beta.2", "1.21.0")).toBe(false);
    expect(newerVersion("1.21.0-beta.1", "1.20.9")).toBe(true);
    expect(compareVersions("1.21.0-beta.2", "1.21.0-beta.2")).toBe(0);
    expect(isPrerelease("1.21.0-beta.2")).toBe(true);
    expect(isPrerelease("1.21.0")).toBe(false);
  });

  test("a stored channel reads back, and anything unknown is Stable", () => {
    expect(readChannel("beta")).toBe("beta");
    expect(readChannel("stable")).toBe("stable");
    expect(readChannel(null)).toBe("stable");
    expect(readChannel("prerelease")).toBe("stable");
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

describe("install progress", () => {
  test("a download with a known size reports a fraction and a percentage", () => {
    const { fraction, step } = installProgress({
      phase: "downloading",
      received: 1_048_576,
      total: 4_194_304,
    });
    expect(fraction).toBe(0.25);
    expect(step).toBe("Downloading · 25% of 4.0 MB");
  });

  test("a download of unknown size counts megabytes and shows no fraction", () => {
    const { fraction, step } = installProgress({ phase: "downloading", received: 2_097_152, total: null });
    expect(fraction).toBeUndefined();
    expect(step).toBe("Downloading · 2.0 MB");
  });

  test("verifying and installing are full bars with their own words", () => {
    expect(installProgress({ phase: "verifying" })).toEqual({
      fraction: 1,
      step: "Verifying the download…",
    });
    expect(installProgress({ phase: "installing" }).step).toMatch(/reopens/);
    expect(installProgress(null)).toEqual({ fraction: 0, step: "Starting the download…" });
  });
});
