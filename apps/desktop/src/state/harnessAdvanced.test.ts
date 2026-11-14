import { expect, test } from "bun:test";
import { fallbackHarnessSnapshot } from "../acp/types";
import { buildHarnessChange } from "./harnessAdvanced";

test("advanced saves only changed fields and retains masked secrets", () => {
  const harness = {
    ...fallbackHarnessSnapshot().harnesses[0]!,
    id: "custom",
    command: ["agent", "acp"],
    env_names: ["TOKEN", "PROXY"],
  };
  const form = {
    name: "Custom",
    executable: "agent",
    args: ["acp"],
    env: [
      { name: "TOKEN", value: "" },
      { name: "PROXY", value: "" },
    ],
  };
  expect(buildHarnessChange(harness, form, harness.env_names)).toEqual({ id: "custom" });
  expect(buildHarnessChange(harness, { ...form, executable: "/opt/agent" }, harness.env_names)).toEqual({
    id: "custom",
    command: ["/opt/agent", "acp"],
  });
  expect(
    buildHarnessChange(
      harness,
      {
        ...form,
        env: [
          { name: "TOKEN", value: "" },
          { name: "PROXY", value: "new" },
        ],
      },
      harness.env_names,
    ),
  ).toEqual({ id: "custom", env: { PROXY: "new" }, retain_env: ["TOKEN"] });
  expect(buildHarnessChange(harness, { ...form, env: [] }, harness.env_names)).toEqual({
    id: "custom",
    env: {},
    retain_env: [],
  });
});
