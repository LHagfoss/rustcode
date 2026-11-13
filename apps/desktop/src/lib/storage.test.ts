// App storage through one pair of calls (lib/storage.ts): what it reads and keeps, and that a
// storage that is missing or refuses a write never throws into the caller.

import { afterEach, describe, expect, test } from "bun:test";
import { readFlag, readStorage, writeFlag, writeStorage } from "./storage";

const scope = globalThis as unknown as Record<string, unknown>;

const withStorage = (storage: unknown) => {
  scope.localStorage = storage;
};

afterEach(() => {
  delete scope.localStorage;
});

describe("app storage", () => {
  test("keeps, reads back and removes a value", () => {
    const data = new Map<string, string>();
    withStorage({
      getItem: (key: string) => data.get(key) ?? null,
      setItem: (key: string, value: string) => void data.set(key, value),
      removeItem: (key: string) => void data.delete(key),
    });
    expect(readStorage("k")).toBeNull();
    expect(writeStorage("k", "v")).toBe(true);
    expect(readStorage("k")).toBe("v");
    expect(writeStorage("k", null)).toBe(true);
    expect(readStorage("k")).toBeNull();
    expect(readFlag("f", true)).toBe(true);
    writeFlag("f", false);
    expect(data.get("f")).toBe("0");
    expect(readFlag("f", true)).toBe(false);
  });

  test("no storage, or one that refuses, reads as unset and says the write was not kept", () => {
    // No DOM at all: the global is missing.
    expect(readStorage("k")).toBeNull();
    expect(writeStorage("k", "v")).toBe(false);
    const refuse = () => {
      throw new Error("QuotaExceededError");
    };
    withStorage({ getItem: refuse, setItem: refuse, removeItem: refuse });
    expect(readStorage("k")).toBeNull();
    expect(readFlag("k", true)).toBe(true);
    expect(writeStorage("k", "v")).toBe(false);
    expect(writeStorage("k", null)).toBe(false);
    expect(writeFlag("k", true)).toBe(false);
  });
});
