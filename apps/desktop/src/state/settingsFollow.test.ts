// Which values a machine that follows this computer's settings is sent
// (docs/desktop.md › Settings on one machine or all).

import { describe, expect, test } from "bun:test";
import { followValues } from "./settingsFollow";

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
