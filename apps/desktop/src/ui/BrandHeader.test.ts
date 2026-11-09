// The sidebar's brand line: the version it prints under RustCode.

import { describe, expect, test } from "bun:test";
import { versionLabel } from "./BrandHeader";

describe("the brand's version line", () => {
  test("a release says v and its number", () => {
    expect(versionLabel("1.21.0")).toBe("v1.21.0");
    expect(versionLabel("0.1.0")).toBe("v0.1.0");
  });

  test("a version the shell already marked with v is not given two", () => {
    expect(versionLabel("v1.21.0")).toBe("v1.21.0");
  });

  test("a beta keeps its prerelease, so the build is the one the user runs", () => {
    expect(versionLabel("1.22.0-beta.3")).toBe("v1.22.0-beta.3");
  });

  test("nothing known, or nothing in it, prints no line rather than a bare v", () => {
    expect(versionLabel(null)).toBeNull();
    expect(versionLabel(undefined)).toBeNull();
    expect(versionLabel("")).toBeNull();
    expect(versionLabel("  ")).toBeNull();
    expect(versionLabel("v")).toBeNull();
  });
});
