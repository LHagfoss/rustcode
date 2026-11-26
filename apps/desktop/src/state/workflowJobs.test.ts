import { describe, expect, test } from "bun:test";
import { parseWorkflowArgs, workflowArgsProblem, workflowParams } from "./workflowJobs";

describe("workflow jobs", () => {
  test("blank args save as an empty object", () => {
    expect(parseWorkflowArgs("  ")).toEqual({});
    expect(parseWorkflowArgs(undefined)).toEqual({});
  });

  test("args must be a JSON object", () => {
    expect(parseWorkflowArgs('{"label": "bug"}')).toEqual({ label: "bug" });
    expect(parseWorkflowArgs("[1]")).toBeNull();
    expect(parseWorkflowArgs("{oops")).toBeNull();
    expect(workflowArgsProblem("[1]")).toContain("JSON object");
    expect(workflowArgsProblem('{"a": 1}')).toBeNull();
  });

  test("a workflow job's params drop the prompt-only fields", () => {
    const params = workflowParams(
      {
        name: "Nightly triage",
        workspace: "/repo",
        prompt: "ignored",
        script: "gh run list",
        threadId: "t-1",
        model: "gpt",
        effort: "high",
        harness: "codex",
        readOnly: true,
        schedule: { kind: "manual" },
      },
      " triage ",
      '{"label": "bug"}',
    );
    expect(params).toEqual({
      name: "Nightly triage",
      workspace: "/repo",
      prompt: "",
      harness: "codex",
      readOnly: true,
      schedule: { kind: "manual" },
      workflow: "triage",
      args: { label: "bug" },
    });
  });
});
