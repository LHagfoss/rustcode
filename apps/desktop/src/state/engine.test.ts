import { describe, expect, test } from "bun:test";
import {
  BOOT_PROGRESS,
  type EngineEvent,
  type EngineState,
  engineDetail,
  HANDSHAKE_PROGRESS,
  initialEngine,
  sessionsListed,
  startingText,
  transitionEngine,
} from "./engine";

const run = (events: EngineEvent[], from: EngineState = initialEngine) =>
  events.reduce(transitionEngine, from);

const READY: EngineState = { phase: "ready", sessions: "listed", restarted: false };

describe("engine connection", () => {
  test("first start: connecting, not a restart, the bar at its start", () => {
    const state = run([{ type: "start" }]);
    expect(state).toEqual({ phase: "connecting", restart: false, progress: BOOT_PROGRESS });
    expect(startingText(state)).toBe("Starting the server…");
    expect(sessionsListed(state)).toBe(false);
  });

  test("slow start: the handshake moves the bar on and keeps it a first start", () => {
    const state = run([{ type: "start" }, { type: "handshake" }]);
    expect(state).toEqual({ phase: "connecting", restart: false, progress: HANDSHAKE_PROGRESS });
    expect(startingText(state)).toBe("Starting the server…");
  });

  test("ready after a first start lists threads and is no restart", () => {
    const state = run([{ type: "start" }, { type: "handshake" }, { type: "ready" }]);
    expect(state).toEqual({ phase: "ready", sessions: "listing", restarted: false });
    expect(sessionsListed(state)).toBe(false);
    expect(engineDetail(state)).toBeUndefined();
  });

  test("the list loaded", () => {
    const state = run([{ type: "start" }, { type: "handshake" }, { type: "ready" }, { type: "listed" }]);
    expect(state).toEqual(READY);
    expect(sessionsListed(state)).toBe(true);
    // A second read changes nothing.
    expect(transitionEngine(state, { type: "listed" })).toBe(state);
  });

  test("a list read before the engine is ready is not the list", () => {
    const state = run([{ type: "start" }, { type: "listed" }]);
    expect(state.phase).toBe("connecting");
    expect(sessionsListed(state)).toBe(false);
  });

  test("crash: a ready engine that closes crashed, with its reason", () => {
    const state = transitionEngine(READY, { type: "closed", detail: "The server stopped." });
    expect(state).toEqual({ phase: "closed", detail: "The server stopped.", crashed: true });
    expect(engineDetail(state)).toBe("The server stopped.");
    expect(sessionsListed(state)).toBe(false);
  });

  test("a first start that fails did not crash", () => {
    const state = run([{ type: "start" }, { type: "closed", detail: "spawn failed" }]);
    expect(state).toEqual({ phase: "closed", detail: "spawn failed", crashed: false });
  });

  test("after a crash the shell's own restart is a restart, and ready says so", () => {
    const crashed = transitionEngine(READY, { type: "closed", detail: "gone" });
    const starting = transitionEngine(crashed, { type: "handshake" });
    expect(starting).toEqual({ phase: "connecting", restart: true, progress: HANDSHAKE_PROGRESS });
    expect(startingText(starting)).toBe("Restarting the server…");
    expect(transitionEngine(starting, { type: "ready" })).toEqual({
      phase: "ready",
      sessions: "listing",
      restarted: true,
    });
  });

  test("Restart: a restart with its own line, then the server restarted", () => {
    const restarting = transitionEngine(READY, { type: "restart" });
    expect(restarting).toEqual({
      phase: "connecting",
      restart: true,
      progress: BOOT_PROGRESS,
      detail: "Restarting the server…",
    });
    expect(startingText(restarting)).toBe("Restarting the server…");
    const state = run([{ type: "handshake" }, { type: "ready" }], restarting);
    expect(state).toEqual({ phase: "ready", sessions: "listing", restarted: true });
  });

  test("a failed restart closes as a crash, and the next try is a restart again", () => {
    const failed = run([{ type: "restart" }, { type: "closed", detail: "no binary" }], READY);
    expect(failed).toEqual({ phase: "closed", detail: "no binary", crashed: true });
    const again = transitionEngine(failed, { type: "restart" });
    expect(again.phase === "connecting" && again.restart).toBe(true);
  });

  test("a Restart after a failed first start is a restart", () => {
    const state = run([
      { type: "start" },
      { type: "closed", detail: "x" },
      { type: "restart" },
      { type: "ready" },
    ]);
    expect(state).toEqual({ phase: "ready", sessions: "listing", restarted: true });
  });

  test("closed straight to ready in one batch is still a restart", () => {
    const closed = transitionEngine(READY, { type: "closed", detail: "gone" });
    expect(transitionEngine(closed, { type: "ready" })).toEqual({
      phase: "ready",
      sessions: "listing",
      restarted: true,
    });
  });

  test("ready again while ready keeps the list", () => {
    expect(transitionEngine(READY, { type: "ready" })).toBe(READY);
  });

  test("a handshake on a ready engine starts again as a first start, like before", () => {
    const state = transitionEngine(READY, { type: "handshake" });
    expect(state).toEqual({ phase: "connecting", restart: false, progress: HANDSHAKE_PROGRESS });
  });
});
