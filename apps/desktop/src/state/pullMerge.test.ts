import { describe, expect, test } from "bun:test";
import { advisoryCheck, checkName, checksHeadline, mergeControl } from "./pullMerge";

const checks = (passed = 0, failed = 0, pending = 0) => ({ passed, failed, pending, skipped: 0 });

describe("merge control", () => {
  test("follows where the pull stands", () => {
    expect(mergeControl({ state: "open", draft: false, checks: checks(6) })).toEqual({ kind: "merge" });
    expect(mergeControl({ state: "open", draft: false, checks: checks() })).toEqual({ kind: "merge" });
    expect(mergeControl({ state: "open", draft: false, checks: checks(6, 0, 3) })).toEqual({
      kind: "merge_when_green",
      pending: 3,
    });
    expect(mergeControl({ state: "open", draft: false, checks: checks(6, 2, 1) })).toEqual({
      kind: "blocked",
      failed: 2,
    });
  });

  test("offers nothing for a draft or a finished pull", () => {
    expect(mergeControl({ state: "open", draft: true, checks: checks(6) })).toEqual({ kind: "none" });
    expect(mergeControl({ state: "merged", draft: false, checks: checks(6) })).toEqual({ kind: "none" });
    expect(mergeControl({ state: "closed", draft: false, checks: checks(6) })).toEqual({ kind: "none" });
  });
});

describe("check names", () => {
  // GitHub's expression syntax, built so it does not read as a template literal.
  const expr = (name: string) => ["$", "{{ ", name, " }}"].join("");
  test("drop an unexpanded matrix template", () => {
    expect(checkName(`Desktop shell (${expr("matrix.name")})`)).toBe("Desktop shell");
    expect(checkName(`${expr("matrix.name")} portability coverage`)).toBe("portability coverage");
    expect(checkName(expr("matrix.os"))).toBe("Job");
    expect(checkName("lint")).toBe("lint");
  });

  test("headline says what needs attention first", () => {
    const run = (status: string) => ({ name: "x", status });
    expect(checksHeadline([run("passed"), run("failed"), run("pending")])).toBe("1 check failed");
    expect(checksHeadline([run("passed"), run("pending"), run("pending")])).toBe("2 checks running");
    expect(checksHeadline([run("passed"), run("skipped")])).toBe("Checks passed");
    expect(checksHeadline([])).toBe("No checks");
    expect(checksHeadline([run("passed"), { name: "portability coverage", status: "failed" }])).toBe(
      "Checks passed",
    );
  });
});

describe("advisory checks", () => {
  const run = (name: string, status: string) => ({ name, status });
  const open = { state: "open" as const, draft: false, checks: checks(1, 1) };

  test("are named as the engine names them", () => {
    expect(advisoryCheck("Windows and macOS portability coverage")).toBe(true);
    expect(advisoryCheck("portability coverage (windows)")).toBe(true);
    expect(advisoryCheck("cargo test --workspace")).toBe(false);
  });

  test("never hold a merge, failing or running", () => {
    expect(mergeControl(open, [run("lint", "passed"), run("portability coverage", "failed")])).toEqual({
      kind: "merge",
    });
    expect(mergeControl(open, [run("lint", "passed"), run("portability coverage", "pending")])).toEqual({
      kind: "merge",
    });
    expect(mergeControl(open, [run("lint", "failed"), run("portability coverage", "failed")])).toEqual({
      kind: "blocked",
      failed: 1,
    });
  });

  test("the list's counts stand in until the runs are read", () => {
    expect(mergeControl(open)).toEqual({ kind: "blocked", failed: 1 });
  });
});
