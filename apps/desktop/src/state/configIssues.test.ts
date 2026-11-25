import { describe, expect, test } from "bun:test";
import { parseConfigIssues, rememberConfigIssues, useConfigIssues } from "./configIssues";

describe("config issues", () => {
  test("reads the engine's unreadable config files", () => {
    const issues = parseConfigIssues({
      values: {},
      configIssues: [
        { path: "/u/config.toml", scope: "user", message: "Failed to parse /u/config.toml: line 2" },
        { path: "/p/.rustcode/config.toml", scope: "project", message: "bad" },
        { path: 3, message: "skipped" },
      ],
    });
    expect(issues).toEqual([
      { path: "/u/config.toml", scope: "user", message: "Failed to parse /u/config.toml: line 2" },
      { path: "/p/.rustcode/config.toml", scope: "project", message: "bad" },
    ]);
  });

  test("an older engine without the field reports none, and clears an earlier answer", () => {
    rememberConfigIssues({ configIssues: [{ path: "/u/config.toml", scope: "user", message: "x" }] });
    expect(useConfigIssues.getState().issues).toHaveLength(1);
    rememberConfigIssues({ values: {} });
    expect(useConfigIssues.getState().issues).toEqual([]);
  });
});
