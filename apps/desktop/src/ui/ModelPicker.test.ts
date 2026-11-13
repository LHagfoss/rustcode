import { describe, expect, test } from "bun:test";
import { BUILTIN_CAPABILITIES, type ConfigOption, type Harness } from "../acp/types";
import type { CachedPlan } from "../state/store";
import {
  accountLeft,
  clampEffort,
  dedupeModels,
  filterModelRows,
  modelDisplayName,
  modelProviderOf,
  modelThreadWrites,
  movePickerIndex,
  orderEfforts,
  panelHeight,
  pickerModels,
  pillLabel,
  repeatedModelNames,
  shownAccounts,
  traitsLabel,
  writeModelPick,
} from "./ModelPicker";

const harness = (efforts: string[]): Harness => ({
  id: "codex",
  name: "Codex",
  kind: "codex",
  builtin: false,
  known: true,
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

  test("a model picked again returns to the effort it was last left on", () => {
    const astra = twoModels().models[1]!;
    // The remembered level wins over the level the picker was showing.
    expect(clampEffort(astra, ["off", "high", "max"], "high", "max")).toBe("max");
    expect(clampEffort(astra, ["off", "high", "max"], "off", "high")).toBe("high");
    // A level the model no longer offers falls through to the current one, else its default.
    expect(clampEffort(astra, ["off", "high", "max"], "high", "medium")).toBe("high");
    expect(clampEffort(astra, ["off", "high", "max"], "medium", "minimal")).toBe("high");
    expect(clampEffort(astra, ["off", "high", "max"], null, null)).toBe("high");
  });

  test("a model that names no default takes the lowest step", () => {
    expect(clampEffort({ id: "m", name: "m", efforts: ["low", "high"] }, ["low", "high"], "max")).toBe("low");
  });
});

describe("a model picked in a thread", () => {
  const [luna, astra] = twoModels().models;

  test("another harness's model brings a level even when none was remembered", () => {
    // The old branch wrote only a level the thread's own effort already listed, so a switch to a
    // model this one does not offer left the thread on an effort the new model lacks.
    expect(modelThreadWrites(astra, "gpt-6-astra", "medium", null, false)).toEqual([
      { configId: "model", value: "gpt-6-astra" },
      { configId: "effort", value: "high" },
    ]);
    // The level that model was last left on, never the other harness's effort.
    expect(modelThreadWrites(astra, "gpt-6-astra", "medium", "max", false)).toEqual([
      { configId: "model", value: "gpt-6-astra" },
      { configId: "effort", value: "max" },
    ]);
  });

  test("inside one harness the level picked here stays while the model lists it", () => {
    expect(modelThreadWrites(luna, "gpt-6-luna", "low", null, true)).toEqual([
      { configId: "model", value: "gpt-6-luna" },
    ]);
    expect(modelThreadWrites(luna, "gpt-6-luna", "off", null, true)).toEqual([
      { configId: "model", value: "gpt-6-luna" },
      { configId: "effort", value: "medium" },
    ]);
  });

  test("a model that lists no levels leaves the effort the thread had", () => {
    expect(modelThreadWrites(undefined, "bare", "high", null, true)).toEqual([
      { configId: "model", value: "bare" },
    ]);
  });

  test("the level is written after the model, so memory files it under the model now run", async () => {
    const writes = modelThreadWrites(astra, "gpt-6-astra", "medium", null, false);
    const seen: string[] = [];
    let model: string | null = null;
    await writeModelPick("thread-1", writes, async (_id, configId, value) => {
      seen.push(`${configId}:${value}`);
      if (configId === "model") model = value;
      // What the harness files the level under: the model the thread has when the write lands.
      else expect(model).toBe("gpt-6-astra");
    });
    expect(seen).toEqual(["model:gpt-6-astra", "effort:high"]);
  });
});

describe("the effort pill", () => {
  const chip = (name: string, selected: boolean) => ({ id: name, name, selected });

  test("names the level the model runs at", () => {
    expect(traitsLabel({ levels: ["low", "high"], effort: "high", accounts: [], fast: false })).toBe("High");
  });

  test("a harness that sets its own effort names the login instead", () => {
    const accounts = [chip("Paral", false), chip("Work", true)];
    expect(
      traitsLabel({
        levels: ["low", "high"],
        effort: "high",
        effortReason: "Codex sets it",
        accounts,
        fast: false,
      }),
    ).toBe("Work");
  });

  test("a model with no levels keeps the pill for its logins or Fast mode", () => {
    expect(
      traitsLabel({ levels: [], effort: null, accounts: [chip("A", false), chip("B", false)], fast: false }),
    ).toBe("Account");
    expect(traitsLabel({ levels: [], effort: null, accounts: [], fast: true })).toBe("Fast mode");
  });

  test("with nothing to choose there is no pill", () => {
    expect(traitsLabel({ levels: [], effort: null, accounts: [], fast: false })).toBeNull();
    expect(traitsLabel({ levels: ["low"], effort: null, accounts: [], fast: false })).toBeNull();
  });
});

describe("the card's height", () => {
  test("leaves the effort and account column room even beside a short list", () => {
    // Two models would be 104px tall; the column's levels and logins need more than that.
    expect(panelHeight([{ models: [1, 2] }])).toBeGreaterThanOrEqual(264);
  });
  test("follows the longest harness's list, so moving along the rail does not resize it", () => {
    const groups = [{ models: [1, 2] }, { models: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10] }];
    expect(panelHeight(groups)).toBe(panelHeight([{ models: groups[1]!.models }, { models: [] }]));
    expect(panelHeight(groups)).toBeGreaterThan(panelHeight([{ models: [1, 2] }, { models: [] }]));
  });

  test("a harness with no models still leaves room for a line, and the rail is never cut", () => {
    expect(panelHeight([{ models: [] }])).toBeGreaterThan(0);
    const rail = Array.from({ length: 12 }, () => ({ models: [] }));
    expect(panelHeight(rail)).toBeGreaterThan(panelHeight([{ models: [] }]));
  });
});

describe("one row per model", () => {
  const model = (value: string, name: string) => ({ value, name });

  test("an id the harness lists twice is one row, where it first listed it", () => {
    const rows = dedupeModels([
      model("fable-5-1", "Fable 5.1"),
      model("sonnet-5-5", "Sonnet 5.5"),
      model("fable-5-1", "Fable 5.1"),
    ]);
    expect(rows.map((row) => row.value)).toEqual(["fable-5-1", "sonnet-5-5"]);
  });

  test("two ids under one name keep the one the harness runs", () => {
    // An alias beside the dated id it names: Claude Code lists both as "Fable 5.1".
    const rows = dedupeModels(
      [
        model("claude-fable-5-1-20260115", "Fable 5.1"),
        model("claude-sonnet-5-5", "Sonnet 5.5"),
        model("claude-fable-5-1", "Fable 5.1"),
      ],
      "claude-fable-5-1",
    );
    expect(rows.map((row) => row.value)).toEqual(["claude-fable-5-1", "claude-sonnet-5-5"]);
  });

  test("with nothing recommended the first listing of a name stays", () => {
    const rows = dedupeModels([
      model("claude-fable-5-1-20260115", "Fable 5.1"),
      model("claude-fable-5-1", "Fable 5.1"),
    ]);
    expect(rows.map((row) => row.value)).toEqual(["claude-fable-5-1-20260115"]);
  });

  test("names that differ in case or spacing are one name", () => {
    const rows = dedupeModels([model("a", "Fable  5.1"), model("b", "fable 5.1 ")]);
    expect(rows.map((row) => row.value)).toEqual(["a"]);
  });

  test("the harness's own order is kept, its first model first", () => {
    const rows = dedupeModels([
      model("fable-5-1", "Fable 5.1"),
      model("haiku-5-5", "Haiku 5.5"),
      model("sonnet-5", "Sonnet 5"),
    ]);
    expect(rows.map((row) => row.value)).toEqual(["fable-5-1", "haiku-5-5", "sonnet-5"]);
  });

  test("a row with no name reads by its id", () => {
    expect(modelDisplayName({ value: "chatgpt/gpt-6-luna", name: "  " })).toBe("chatgpt/gpt-6-luna");
    expect(modelDisplayName({ value: "luna", name: " GPT  6 Luna " })).toBe("GPT 6 Luna");
  });

  test("one name under two providers is two models, both listed", () => {
    // Pi and OpenCode list `provider/model`: the same model through two providers is two picks.
    const rows = dedupeModels([
      model("anthropic/claude-sonnet-4", "Claude Sonnet 4"),
      model("openrouter/anthropic/claude-sonnet-4", "Claude Sonnet 4"),
    ]);
    expect(rows.map((row) => row.value)).toEqual([
      "anthropic/claude-sonnet-4",
      "openrouter/anthropic/claude-sonnet-4",
    ]);
  });

  test("one provider's pair of ids for one model is still one row", () => {
    const rows = dedupeModels(
      [
        model("anthropic/claude-sonnet-4-20260115", "Claude Sonnet 4"),
        model("anthropic/claude-sonnet-4", "Claude Sonnet 4"),
        model("anthropic/claude-haiku-4", "Claude Haiku 4"),
      ],
      "anthropic/claude-sonnet-4",
    );
    expect(rows.map((row) => row.value)).toEqual(["anthropic/claude-sonnet-4", "anthropic/claude-haiku-4"]);
  });

  test("an id carries its provider, and a bare id names none", () => {
    expect(modelProviderOf("anthropic/claude-sonnet-4")).toBe("anthropic");
    expect(modelProviderOf("openrouter/anthropic/claude-sonnet-4")).toBe("openrouter/anthropic");
    expect(modelProviderOf("claude-sonnet-4")).toBeNull();
    expect(modelProviderOf("/leading")).toBeNull();
  });

  test("only the names a list holds twice read by their provider", () => {
    const shared = repeatedModelNames([
      model("anthropic/claude-sonnet-4", "Claude Sonnet 4"),
      model("openrouter/anthropic/claude-sonnet-4", "claude  sonnet 4"),
      model("anthropic/claude-haiku-4", "Claude Haiku 4"),
    ]);
    expect([...shared]).toEqual(["claude sonnet 4"]);
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

describe("what is left of an account", () => {
  const plan = (used: number, minutes: number): CachedPlan => ({
    source: "test",
    windows: [{ label: "Usage", usedPercent: used, minutes }],
    fetchedAt: Date.now(),
  });

  test("reads the shortest window of the login's own plan", () => {
    const cache: Record<string, CachedPlan> = {
      "claude-code#work": {
        source: "test",
        windows: [
          { label: "Weekly", usedPercent: 90, minutes: 10080 },
          { label: "5-hour", usedPercent: 25, minutes: 300 },
        ],
        fetchedAt: Date.now(),
      },
    };
    expect(accountLeft(cache, "claude-code", "work")).toBe(75);
  });

  test("the harness's own login and a named one never share numbers", () => {
    const cache = { "claude-code": plan(10, 300), "claude-code#work": plan(60, 300) };
    expect(accountLeft(cache, "claude-code", null)).toBe(90);
    expect(accountLeft(cache, "claude-code", "work")).toBe(40);
    expect(accountLeft(cache, "claude-code", "other")).toBeNull();
  });

  test("a login that is gone has nothing left to say", () => {
    expect(accountLeft({ "claude-code": plan(10, 300) }, "claude-code", undefined)).toBeNull();
  });
});

describe("a new thread before its harness lists models", () => {
  // The options of whichever thread is open in the app: the built-in harness's own list, whose
  // effort is high and whose models this external harness cannot run.
  const openThread: ConfigOption[] = [
    { id: "model", name: "Model", type: "select", current: "gemini-3.6-flash", choices: [] },
    {
      id: "effort",
      name: "Effort",
      type: "select",
      current: "high",
      choices: [
        { value: "default", name: "Default" },
        { value: "high", name: "High" },
      ],
    },
  ];
  const unlisted: Harness = { ...harness([]), models: [] };

  test("keeps its own model and effort, not the open thread's", () => {
    const picked = pickerModels(
      undefined,
      unlisted,
      { harness: "codex", model: null, effort: "medium" },
      openThread,
    );
    expect(picked.model).toBeNull();
    expect(picked.effort).toBe("medium");
    expect(picked.models).toEqual([]);
    expect(picked.efforts).toEqual([]);
  });

  test("shows no effort when none is remembered", () => {
    const picked = pickerModels(
      undefined,
      unlisted,
      { harness: "codex", model: null, effort: null },
      openThread,
    );
    expect(picked.effort).toBeNull();
  });

  test("the built-in harness still reads the open thread's list", () => {
    const local: Harness = { ...unlisted, id: "rustcode", builtin: true, models: [] };
    const picked = pickerModels(
      undefined,
      local,
      { harness: "rustcode", model: null, effort: null },
      openThread,
    );
    expect(picked.model).toBe("gemini-3.6-flash");
    expect(picked.effort).toBe("high");
  });
});

describe("model picker navigation", () => {
  test("filters across providers or limits the list to the selected provider", () => {
    const first = harness([]);
    const second = { ...harness([]), id: "other", name: "Other" };
    const groups = [
      { harness: first, models: [{ value: "sonnet", name: "Sonnet 5.5" }] },
      { harness: second, models: [{ value: "opus", name: "Opus" }] },
    ];
    expect(filterModelRows(groups, "codex", "").map((r) => r.model.value)).toEqual(["sonnet"]);
    expect(filterModelRows(groups, "codex", "opus").map((r) => r.group.harness.id)).toEqual(["other"]);
    expect(filterModelRows(groups, "codex", "missing")).toEqual([]);
  });
  test("finds a model by its id, so a provider narrows the list", () => {
    const group = {
      harness: harness([]),
      models: [
        { value: "anthropic/claude-sonnet-4", name: "Claude Sonnet 4" },
        { value: "openai/gpt-6", name: "GPT 6" },
      ],
    };
    expect(filterModelRows([group], "codex", "openai/").map((r) => r.model.value)).toEqual(["openai/gpt-6"]);
  });
  test("clamps keyboard movement and displays the catalog label", () => {
    expect(movePickerIndex(0, -1, 3)).toBe(0);
    expect(movePickerIndex(2, 1, 3)).toBe(2);
    expect(pillLabel("Sonnet 5.5")).toBe("Sonnet 5.5");
    expect(pillLabel(null)).toBe("Default model");
    expect(pillLabel("claude-sonnet-5-5-long", true)).toBe("claude-sonnet…");
  });
});
