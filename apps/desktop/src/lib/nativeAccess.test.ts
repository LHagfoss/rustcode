import { describe, expect, test } from "bun:test";
import {
  micBlocked,
  micBlockedNotice,
  parseMicAccess,
  parseSchemeStatus,
  schemeWarning,
} from "./nativeAccess";

describe("microphone access", () => {
  test("reads the shell's statuses, and anything else as unsupported", () => {
    expect(parseMicAccess("denied")).toBe("denied");
    expect(parseMicAccess("not-determined")).toBe("not-determined");
    expect(parseMicAccess("authorized")).toBe("unsupported");
    expect(parseMicAccess(undefined)).toBe("unsupported");
  });

  test("only a refusal blocks recording", () => {
    expect(micBlocked("denied")).toBe(true);
    expect(micBlocked("restricted")).toBe(true);
    for (const access of ["granted", "not-determined", "unsupported"] as const)
      expect(micBlocked(access)).toBe(false);
  });

  test("a denial points to the setting; a policy does not offer it", () => {
    const denied = micBlockedNotice("denied");
    expect(denied.canOpenSettings).toBe(true);
    expect(denied.text).toContain("Privacy & Security › Microphone");
    expect(micBlockedNotice("restricted").canOpenSettings).toBe(false);
  });
});

describe("link scheme ownership", () => {
  test("reads the shell's status defensively", () => {
    expect(parseSchemeStatus({ owner: "other", handler: "/x/RustCode.app", canFix: true })).toEqual({
      owner: "other",
      handler: "/x/RustCode.app",
      canFix: true,
    });
    expect(parseSchemeStatus(null)).toEqual({ owner: "unknown", handler: null, canFix: false });
    expect(parseSchemeStatus({ owner: "mine", handler: "" })).toEqual({
      owner: "unknown",
      handler: null,
      canFix: false,
    });
  });

  test("says nothing when links open here or it cannot tell", () => {
    expect(schemeWarning({ owner: "this", handler: null, canFix: true })).toBeNull();
    expect(schemeWarning({ owner: "unknown", handler: null, canFix: false })).toBeNull();
  });

  test("names the other copy and offers the fix", () => {
    const warning = schemeWarning({
      owner: "other",
      handler: "/Users/me/Downloads/RustCode.app",
      canFix: true,
    });
    expect(warning?.text).toContain("/Users/me/Downloads/RustCode.app");
    expect(warning?.fix).toBe("Open Links Here");
  });

  test("explains what to do when there is no fix", () => {
    const warning = schemeWarning({ owner: "other", handler: null, canFix: false });
    expect(warning?.fix).toBeNull();
    expect(warning?.text).toContain("Quit the other copy");
    expect(schemeWarning({ owner: "none", handler: null, canFix: true })?.title).toContain("do not open");
  });
});
