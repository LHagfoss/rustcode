import { describe, expect, test } from "bun:test";
import type { Machine } from "../state/machines";
import { threadMachine } from "./composerFooterLayout";

const machine = (patch: Partial<Machine> = {}): Machine => ({
  id: "gw-studio",
  name: "studio.local",
  platform: "macos",
  version: "1.31.4",
  routes: [],
  route: null,
  state: "connected",
  detail: null,
  hint: null,
  retryInSecs: null,
  addedAt: "2026-11-01T00:00:00Z",
  lastConnectedAt: null,
  ...patch,
});

describe("the machine a thread runs on", () => {
  test("a thread on this computer says nothing", () => {
    expect(threadMachine("", [machine()])).toBeNull();
    expect(threadMachine("", [])).toBeNull();
  });

  test("a thread on a paired machine names it, with its glyph and tint", () => {
    const there = threadMachine("gw-studio", [
      machine({ displayName: "Mac Studio", icon: "studio", iconTint: "3" }),
    ]);
    expect(there).toEqual({
      name: "Mac Studio",
      glyph: "studio",
      tint: "3",
      away: false,
      title: "Runs on Mac Studio",
    });
  });

  test("without a chosen glyph the platform's own is drawn", () => {
    expect(threadMachine("gw-studio", [machine({ platform: "windows" })])?.glyph).toBe("tower");
    expect(threadMachine("gw-studio", [machine({ platform: "linux" })])?.glyph).toBe("linux");
  });

  test("a machine that is not connected says so", () => {
    const offline = threadMachine("gw-studio", [machine({ state: "offline" })]);
    expect(offline?.away).toBe(true);
    expect(offline?.title).toBe("Runs on studio.local · offline");
    expect(threadMachine("gw-studio", [machine({ state: "connecting" })])?.title).toBe(
      "Runs on studio.local · connecting",
    );
  });

  test("a machine that is no longer paired is still not this computer", () => {
    const gone = threadMachine("gw-removed", [machine()]);
    expect(gone?.name).toBe("Another machine");
    expect(gone?.away).toBe(true);
  });
});
