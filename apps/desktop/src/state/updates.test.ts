import { describe, expect, test } from "bun:test";
import type { UpdateInfo } from "../lib/updater";
import {
  type AutoInstallFacts,
  anyThreadBusy,
  autoInstallDecision,
  compareVersions,
  installProgress,
  isPrerelease,
  newerVersion,
  readAutoInstall,
  readChannel,
  shouldShowWhatsNew,
  updateHint,
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

  test("a longer prerelease suffix is newer, in both directions", () => {
    expect(compareVersions("1.0.0-alpha", "1.0.0-alpha.1")).toBe(-1);
    expect(compareVersions("1.0.0-alpha.1", "1.0.0-alpha")).toBe(1);
    expect(compareVersions("1.0.0-alpha.1", "1.0.0-alpha.2")).toBe(-1);
    expect(compareVersions("1.0.0-alpha.2", "1.0.0-alpha.1")).toBe(1);
    expect(compareVersions("1.0.0-1", "1.0.0-alpha")).toBe(-1);
    expect(compareVersions("1.0.0-alpha", "1.0.0-1")).toBe(1);
  });

  test("build metadata takes no part in ordering", () => {
    expect(compareVersions("1.21.0+build.7", "1.21.0")).toBe(0);
    expect(newerVersion("v1.21.0+build.7", "1.20.0")).toBe(true);
    expect(newerVersion("1.21.0-beta.2+sha.abc", "1.21.0-beta.1")).toBe(true);
    expect(isPrerelease("1.21.0+build.7")).toBe(false);
    expect(isPrerelease("1.21.0-beta.2+build.7")).toBe(true);
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

describe("the update row's hint", () => {
  const info = {
    current: "1.21.0",
    latest: "1.22.0",
    available: true,
    canInstall: true,
    installNote: null,
    development: false,
    page: "https://github.com/LHagfoss/rustcode/releases/tag/v1.22.0",
    channels: ["stable"],
    channel: "stable",
  } as const;

  test("an installable copy says it installs and reopens; any other opens the release page", () => {
    expect(updateHint({ ...info, channels: ["stable"] })).toBe("Installs and reopens RustCode");
    expect(updateHint({ ...info, canInstall: false, channels: ["stable"] })).toBe("Opens the release page");
  });

  test("a note from the shell replaces the default (the .deb asks for a password)", () => {
    const note = "Asks for your password to install the package";
    expect(updateHint({ ...info, installNote: note, channels: ["stable"] })).toBe(note);
  });
});

describe("automatic install", () => {
  const info = (over: Partial<UpdateInfo> = {}): UpdateInfo => ({
    current: "1.31.3",
    latest: "1.32.0",
    available: true,
    canInstall: true,
    installNote: null,
    development: false,
    page: null,
    channels: ["stable", "beta"],
    channel: "stable",
    ...over,
  });
  const facts = (over: Partial<AutoInstallFacts> = {}): AutoInstallFacts => ({
    enabled: true,
    development: false,
    info: info(),
    channel: "stable",
    skipped: null,
    installing: false,
    busy: false,
    ...over,
  });

  test("installs a newer release when enabled and no thread is working", () => {
    expect(autoInstallDecision(facts())).toBe("install");
  });

  test("waits while a thread works, and installs once it is idle", () => {
    expect(autoInstallDecision(facts({ busy: true }))).toBe("wait");
    expect(autoInstallDecision(facts({ busy: false }))).toBe("install");
  });

  test("never installs in a development build", () => {
    expect(autoInstallDecision(facts({ development: true }))).toBe("none");
    expect(autoInstallDecision(facts({ info: info({ development: true }) }))).toBe("none");
  });

  test("does nothing when the setting is off", () => {
    expect(autoInstallDecision(facts({ enabled: false }))).toBe("none");
    expect(autoInstallDecision(facts({ enabled: false, busy: true }))).toBe("none");
  });

  test("a skipped beta stays skipped", () => {
    const beta = info({ latest: "1.32.0-beta.2", channel: "beta" });
    expect(autoInstallDecision(facts({ info: beta, channel: "beta" }))).toBe("install");
    expect(autoInstallDecision(facts({ info: beta, channel: "beta", skipped: "1.32.0-beta.2" }))).toBe(
      "none",
    );
    expect(
      autoInstallDecision(
        facts({ info: info({ available: false }), channel: "beta", skipped: "1.32.0-beta.2" }),
      ),
    ).toBe("none");
  });

  test("needs an offer from the selected channel that this copy can install", () => {
    expect(autoInstallDecision(facts({ info: null }))).toBe("none");
    expect(autoInstallDecision(facts({ info: info({ available: false }) }))).toBe("none");
    expect(autoInstallDecision(facts({ info: info({ channel: "beta" }) }))).toBe("none");
    expect(autoInstallDecision(facts({ info: info({ canInstall: false }) }))).toBe("none");
    expect(autoInstallDecision(facts({ installing: true }))).toBe("none");
  });

  test("a running turn, background commands, an approval or a question count as work", () => {
    const idle = { running: false, permissions: [] };
    expect(anyThreadBusy({})).toBe(false);
    expect(anyThreadBusy({ a: idle })).toBe(false);
    expect(anyThreadBusy({ a: idle, b: { ...idle, running: true } })).toBe(true);
    expect(anyThreadBusy({ a: { ...idle, waiting: 2 } })).toBe(true);
    expect(anyThreadBusy({ a: { ...idle, permissions: [{}] } })).toBe(true);
  });

  test("the setting is on unless it was turned off", () => {
    expect(readAutoInstall(null)).toBe(true);
    expect(readAutoInstall("1")).toBe(true);
    expect(readAutoInstall("0")).toBe(false);
  });
});
