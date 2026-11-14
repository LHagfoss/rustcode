// Settings a project overrides (docs/desktop.md › Settings a project overrides): the resolution
// order, which rows a project may set, the machines a project write reaches, and removing an
// override through the mock transport the pages run on without an engine.

import { describe, expect, test } from "bun:test";
import type { JsonObject } from "../acp/jsonrpc";
import { mockSettingsAnswer, mockSettingsSet } from "../acp/mock";
import {
  PROJECT_SETTING_KEYS,
  projectOverridable,
  projectReadOnlyReason,
  projectTargets,
  resolveSetting,
  type ScopedMachine,
  SERVER_SETTING_KEYS,
} from "./settingsRegistry";

const machine = (host: string, online = true): ScopedMachine => ({
  host,
  name: host,
  online,
  local: host === "",
});

describe("resolveSetting", () => {
  test("default, then the machine, then the project", () => {
    expect(resolveSetting({ default: "ask" })).toEqual({ value: "ask", source: "default" });
    expect(resolveSetting({ default: "ask", machine: "edits" })).toEqual({
      value: "edits",
      source: "machine",
    });
    expect(resolveSetting({ default: "ask", machine: "edits", project: "full" })).toEqual({
      value: "full",
      source: "project",
    });
    // A project that set `false` still wins: only a missing layer falls through.
    expect(resolveSetting({ default: true, machine: true, project: false })).toEqual({
      value: false,
      source: "project",
    });
  });
});

describe("the registry's overridable flag", () => {
  test("per-thread behaviour and new-thread defaults, not machine-wide limits", () => {
    for (const key of [
      "default-harness",
      "default-permission-mode",
      "resume-after-restart",
      "continue-after-limit",
      "auto-compact-mode",
      "auto-compact-threshold",
    ])
      expect(projectOverridable(key)).toBe(true);
    for (const key of [
      "delegates-total",
      "delegates-per-thread",
      "orchestration",
      "trust-delegate-accounts",
      "limit-fallback",
    ])
      expect(projectOverridable(key)).toBe(false);
  });

  test("every overridable server key is one the registry knows", () => {
    for (const key of PROJECT_SETTING_KEYS)
      if (key !== "default-harness") expect(SERVER_SETTING_KEYS).toContain(key);
  });

  test("rows a project cannot set say why", () => {
    expect(projectReadOnlyReason("server", "default-permission-mode")).toBeNull();
    expect(projectReadOnlyReason("server", "delegates-total")).toContain("every project");
    expect(projectReadOnlyReason("machine")).toContain("machine");
    expect(projectReadOnlyReason("app")).toContain("app preference");
  });
});

describe("projectTargets", () => {
  test("a machine without the project is skipped and listed", () => {
    const paths: Record<string, string> = { "": "/work/app", studio: "/Users/me/app" };
    const result = projectTargets([machine(""), machine("studio"), machine("linux")], (m) => paths[m.host]);
    expect(result.reached.map((t) => [t.machine.host, t.path])).toEqual([
      ["", "/work/app"],
      ["studio", "/Users/me/app"],
    ]);
    expect(result.skipped.map((m) => m.host)).toEqual(["linux"]);
  });
});

describe("the mock transport", () => {
  test("an override resolves over the machine and is removed by clear", () => {
    const projects: Record<string, JsonObject> = {};
    let values: JsonObject = { "default-permission-mode": "edits", "delegates-total": 8 };
    values = mockSettingsSet(values, projects, {
      project: "/work/app/",
      values: { "default-permission-mode": "full" },
    });
    const answer = mockSettingsAnswer(values, projects, [], "/work/app");
    expect((answer.values as JsonObject)["default-permission-mode"]).toBe("full");
    expect((answer.sources as JsonObject)["default-permission-mode"]).toBe("project");
    // The machine and another project keep the machine's value.
    expect(
      (mockSettingsAnswer(values, projects, [], undefined).values as JsonObject)["default-permission-mode"],
    ).toBe("edits");
    expect(
      (mockSettingsAnswer(values, projects, [], "/work/other").values as JsonObject)[
        "default-permission-mode"
      ],
    ).toBe("edits");
    values = mockSettingsSet(values, projects, { project: "/work/app", clear: ["default-permission-mode"] });
    const after = mockSettingsAnswer(values, projects, [], "/work/app");
    expect((after.values as JsonObject)["default-permission-mode"]).toBe("edits");
    expect((after.sources as JsonObject)["default-permission-mode"]).toBe("machine");
    expect(projects).toEqual({});
  });

  test("a machine-wide setting is refused for a project", () => {
    expect(() =>
      mockSettingsSet({}, {}, { project: "/work/app", values: { "delegates-total": 2 } }),
    ).toThrow();
  });
});
