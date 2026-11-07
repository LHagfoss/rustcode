import { describe, expect, test } from "bun:test";
import { BUILTIN_CAPABILITIES, type Harness } from "../acp/types";
import { clampEffort, orderEfforts, pickerModels, shownAccounts } from "./ModelPicker";

const harness = (efforts: string[]): Harness => ({
  id: "codex",
  name: "Codex",
  kind: "codex",
  builtin: false,
  enabled: true,
  command: ["codex", "app-server"],
  installed: true,
  auth: "ok",
  capabilities: { ...BUILTIN_CAPABILITIES, effort: true },
  models: [{ id: "gpt-6-luna", name: "gpt-6-luna", efforts, default_effort: "medium" }],
});

/** Two models with their own levels, as Pi and OpenCode list them. */
const twoModels = (): Harness => ({
  ...harness(["low", "medium", "high"]),
  models: [
    { id: "gpt-6-luna", name: "gpt-6-luna", efforts: ["low", "medium", "high"], default_effort: "medium" },
    { id: "gpt-6-astra", name: "gpt-6-astra", efforts: ["off", "high", "max"], default_effort: "high" },
  ],
});

describe("effort order", () => {
  test("off comes first even when the harness lists it last", () => {
    expect(orderEfforts(["low", "medium", "high", "off"])).toEqual(["off", "low", "medium", "high"]);
    expect(orderEfforts(["high", "default", "minimal"])).toEqual(["default", "minimal", "high"]);
  });

  test("max is the top step, above xhigh", () => {
    expect(orderEfforts(["low", "max", "xhigh", "high"])).toEqual(["low", "high", "xhigh", "max"]);
  });

  test("unknown values keep the harness's order after the known ones", () => {
    expect(orderEfforts(["turbo", "high", "zeta", "off"])).toEqual(["off", "high", "turbo", "zeta"]);
  });

  test("a list without off keeps its semantic order", () => {
    expect(orderEfforts(["high", "low", "medium"])).toEqual(["low", "medium", "high"]);
  });

  test("the picker orders the stops and keeps the selected effort", () => {
    const model = harness(["low", "medium", "high", "off"]);
    const picked = pickerModels(
      undefined,
      model,
      { harness: "codex", model: "gpt-6-luna", effort: "high" },
      [],
    );
    expect(picked.efforts).toEqual(["off", "low", "medium", "high"]);
    // The knob sits on the same effort; only its position on the track changed.
    expect(picked.effort).toBe("high");
  });

  test("a model with no off stop keeps its default effort", () => {
    const picked = pickerModels(
      undefined,
      harness(["low", "medium", "high"]),
      { harness: "codex", model: "gpt-6-luna", effort: null },
      [],
    );
    expect(picked.efforts).toEqual(["low", "medium", "high"]);
    expect(picked.effort).toBe("medium");
  });
});

describe("the slider's stops", () => {
  test("each model brings its own levels, ordered, and its own default", () => {
    const codex = twoModels();
    const luna = pickerModels(undefined, codex, { harness: "codex", model: "gpt-6-luna", effort: null }, []);
    const astra = pickerModels(
      undefined,
      codex,
      { harness: "codex", model: "gpt-6-astra", effort: null },
      [],
    );
    expect(luna.efforts).toEqual(["low", "medium", "high"]);
    expect(luna.effort).toBe("medium");
    expect(astra.efforts).toEqual(["off", "high", "max"]);
    expect(astra.effort).toBe("high");
  });

  test("a level the new model has keeps its place on the track", () => {
    const codex = twoModels();
    const astra = pickerModels(
      undefined,
      codex,
      { harness: "codex", model: "gpt-6-astra", effort: "high" },
      [],
    );
    expect(astra.effort).toBe("high");
  });

  test("a level the new model lacks clamps to that model's default", () => {
    const astra = twoModels().models[1]!;
    expect(clampEffort(astra, ["off", "high", "max"], "medium")).toBe("high");
    expect(clampEffort(astra, ["off", "high", "max"], "high")).toBe("high");
    // No level at all leaves nothing to clamp to.
    expect(clampEffort(undefined, [], "high")).toBeNull();
  });

  test("a model that names no default takes the lowest step", () => {
    expect(clampEffort({ id: "m", name: "m", efforts: ["low", "high"] }, ["low", "high"], "max")).toBe("low");
  });
});

describe("account rows", () => {
  const chip = (name: string, selected: boolean) => ({ id: name, name, selected });

  test("one login is the pill's business, not a row to choose", () => {
    expect(shownAccounts([chip("Paral", true)])).toEqual([]);
    expect(shownAccounts([])).toEqual([]);
  });

  test("two logins are rows, and the selected one is kept", () => {
    const chips = [chip("Paral", true), chip("Work", false)];
    expect(shownAccounts(chips)).toEqual(chips);
  });
});
