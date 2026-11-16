import { describe, expect, test } from "bun:test";
import { problemCommand, problemKey, settingsAttention, visibleProblems } from "./settingsAttention";

const PATH = {
  code: "path-version-mismatch",
  message: "`rustcode` on PATH is 1.39.0, but this engine is 1.39.1.",
};
const RESTART = { code: "restart-pending", message: "A restart is waiting for running turns to finish." };

describe("settings attention", () => {
  test("points a machine that is away at Machines and a server warning at Remote access", () => {
    expect(settingsAttention(2, [PATH], [])).toEqual({
      machines: "2 machines are not connected",
      remote: "the RustCode server has a warning",
    });
  });

  test("a state that is not a fault puts no dot", () => {
    expect(settingsAttention(0, [RESTART], [])).toEqual({});
  });

  test("a dismissed warning stays hidden until its words change", () => {
    const dismissed = [problemKey(PATH)];
    expect(settingsAttention(0, [PATH], dismissed)).toEqual({});
    const newer = { ...PATH, message: "`rustcode` on PATH is 1.39.1, but this engine is 1.40.0." };
    expect(visibleProblems([newer], dismissed)).toEqual([newer]);
  });

  test("finds the command a warning names", () => {
    expect(problemCommand("Run `rustcode server restart --when-idle` now.")).toBe(
      "rustcode server restart --when-idle",
    );
    expect(problemCommand(PATH.message)).toBeNull();
  });
});
