import { describe, expect, test } from "bun:test";
import {
  distinctModels,
  modelKey,
  readModelHistory,
  recentModels,
  rememberModel,
  toggleFavourite,
  writeModelHistory,
} from "./modelHistory";

const a = { harness: "codex", model: "same" };
const b = { harness: "claude", model: "same" };
describe("model picker history", () => {
  test("favourites prepend, distinguish harnesses and toggle off", () => {
    expect(toggleFavourite([a], b)).toEqual([b, a]);
    expect(toggleFavourite([b, a], a)).toEqual([b]);
    expect(distinctModels([a, b, a])).toEqual([a, b]);
    expect(modelKey({ harness: "a:b", model: "c" })).not.toBe(modelKey({ harness: "a", model: "b:c" }));
  });
  test("recents move repeats first, cap at five and exclude favourites", () => {
    const choices = Array.from({ length: 6 }, (_, i) => ({ harness: "codex", model: String(i) }));
    expect(choices.reduce(rememberModel, [] as typeof choices)).toEqual(choices.slice(1).reverse());
    expect(rememberModel([a, b], b)).toEqual([b, a]);
    expect(recentModels([a, b, a], [a])).toEqual([b]);
  });
  test("storage round trips and rejects malformed entries", () => {
    const original = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
    const data = new Map<string, string>();
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      value: {
        getItem: (key: string) => data.get(key) ?? null,
        setItem: (key: string, value: string) => data.set(key, value),
      },
    });
    try {
      writeModelHistory("favourites", [b, a]);
      expect(readModelHistory("favourites")).toEqual([b, a]);
      writeModelHistory("recents", [a, b, a]);
      expect(readModelHistory("recents")).toEqual([a, b]);
      data.set("rustcode.model-picker.favourites", "broken");
      expect(readModelHistory("favourites")).toEqual([]);
      data.set("rustcode.model-picker.favourites", '[null,42,{}, {"harness":"codex","model":"same"}]');
      expect(readModelHistory("favourites")).toEqual([a]);
    } finally {
      if (original) Object.defineProperty(globalThis, "localStorage", original);
      else Reflect.deleteProperty(globalThis, "localStorage");
    }
  });
});
