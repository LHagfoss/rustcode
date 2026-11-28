// Which values a machine that follows this computer's settings is sent
// (docs/desktop.md › Settings on one machine or all).

import { describe, expect, test } from "bun:test";
import { compareListing, follows, followValues } from "./settingsFollow";

describe("followValues", () => {
  test("sends only the shared values that differ or are missing", () => {
    const local = {
      "default-permission-mode": "full",
      "resume-after-restart": true,
      "wake-batch-seconds": 60,
    };
    const remote = { "default-permission-mode": "ask", "resume-after-restart": true };
    expect(followValues(local, remote)).toEqual({
      "default-permission-mode": "full",
      "wake-batch-seconds": 60,
    });
  });

  test("compares lists by value", () => {
    const map = [{ from: "a", to: "b" }];
    expect(
      followValues({ "limit-fallback-map": map }, { "limit-fallback-map": [{ from: "a", to: "b" }] }),
    ).toEqual({});
  });

  test("never sends a key that is not a shared server setting", () => {
    expect(followValues({ "default-project-folder": "/Users/me/code", theme: "dark" }, {})).toEqual({});
  });

  test("an unreadable follower gets every shared value this computer holds", () => {
    expect(followValues({ orchestration: true }, null)).toEqual({ orchestration: true });
  });
});

describe("compareListing", () => {
  test("the first readable listing is the baseline, not a change", () => {
    expect(compareListing(null, "a")).toEqual({ baseline: "a", changed: false });
    expect(compareListing("a", "a")).toEqual({ baseline: "a", changed: false });
    expect(compareListing("a", "b")).toEqual({ baseline: "b", changed: true });
  });

  test("an unreadable listing keeps the baseline and reports nothing", () => {
    expect(compareListing("a", null)).toEqual({ baseline: "a", changed: false });
    expect(compareListing(null, null)).toEqual({ baseline: null, changed: false });
    expect(compareListing(compareListing("a", null).baseline, "a").changed).toBe(false);
  });
});

describe("follows", () => {
  test("a listed machine follows, an unlisted one does not", () => {
    expect(follows({ ids: ["studio"], all: false }, "studio")).toBe(true);
    expect(follows({ ids: ["studio"], all: false }, "vps")).toBe(false);
  });

  test("every machine follows while the fleet switch is on", () => {
    expect(follows({ ids: [], all: true }, "vps")).toBe(true);
  });

  test("this computer never follows itself", () => {
    expect(follows({ ids: [""], all: true }, "")).toBe(false);
    expect(follows({ ids: ["local"], all: true }, "local")).toBe(false);
  });
});
