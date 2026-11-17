// This computer's engine connection as one state and one pure transition (issue 3911).
//
// Whether a start is the first one or a restart, how far it has got, whether the
// thread list is in, and why it stopped are one union: the store keeps it as
// `engine`, the engine toast reads it (`onEngineChange` in `store.ts`), and a
// caller asks `engine.phase === "ready"` instead of pairing flags by hand.

/** The engine's connection. Each phase carries only its own facts. */
export type EngineState =
  | {
      phase: "connecting";
      /** The engine stopped (or Restart was pressed) since it was last ready: "Restarting the server…". */
      restart: boolean;
      /** How far the start has got, 0–1, for the slow-start toast's bar. */
      progress: number;
      /** What the start is doing, when the app says ("Restarting the server…"). */
      detail?: string;
    }
  | {
      phase: "ready";
      /** Whether this computer's thread list has been read since the engine became ready. */
      sessions: "listing" | "listed";
      /** It came back from a stop or a Restart: the toast says "The server restarted". */
      restarted: boolean;
    }
  | {
      phase: "closed";
      /** Why it stopped, for the error toast and Diagnostics. */
      detail: string;
      /** It stopped after it was ready (or during a restart), rather than failing its first start. */
      crashed: boolean;
    };

/** Where a start's progress bar begins, and where the handshake puts it. */
export const BOOT_PROGRESS = 0.15;
export const HANDSHAKE_PROGRESS = 0.5;

export const initialEngine: EngineState = { phase: "connecting", restart: false, progress: 0 };

export type EngineEvent =
  /** The app started its transport (`connect`): the first start. */
  | { type: "start" }
  /** Restart was pressed (`restartEngine`). */
  | { type: "restart" }
  /** The shell (re)started the engine and the app is initializing a fresh connection. */
  | { type: "handshake" }
  /** The handshake answered. */
  | { type: "ready" }
  /** This computer's thread list was read (or could not be: either way it is no longer pending). */
  | { type: "listed" }
  /** The connection closed, or a start failed. */
  | { type: "closed"; detail: string };

/** Whether the engine is on its way back from a stop: the next ready is a restart. */
function restarting(state: EngineState): boolean {
  if (state.phase === "connecting") return state.restart;
  return state.phase === "closed";
}

export function transitionEngine(state: EngineState, event: EngineEvent): EngineState {
  switch (event.type) {
    case "start":
      return { phase: "connecting", restart: restarting(state), progress: BOOT_PROGRESS };
    case "restart":
      return {
        phase: "connecting",
        restart: true,
        progress: BOOT_PROGRESS,
        detail: "Restarting the server…",
      };
    case "handshake":
      return {
        phase: "connecting",
        restart: restarting(state),
        progress: Math.max(HANDSHAKE_PROGRESS, state.phase === "connecting" ? state.progress : 0),
      };
    case "ready":
      if (state.phase === "ready") return state;
      // Batched updates can go from closed straight to ready: that is a restart too.
      return { phase: "ready", sessions: "listing", restarted: restarting(state) };
    case "listed":
      if (state.phase !== "ready" || state.sessions === "listed") return state;
      return { ...state, sessions: "listed" };
    case "closed":
      return {
        phase: "closed",
        detail: event.detail,
        crashed: state.phase === "ready" || restarting(state),
      };
  }
}

/** Whether this computer's thread list is in: an empty list then reads "no threads", not loading. */
export function sessionsListed(engine: EngineState): boolean {
  return engine.phase === "ready" && engine.sessions === "listed";
}

/** What the connection says, when anything: why it stopped, or what a start is doing. */
export function engineDetail(engine: EngineState): string | undefined {
  return engine.phase === "ready" ? undefined : engine.detail;
}

/** The engine toast's starting line, once a start takes long enough to show one. */
export function startingText(engine: EngineState): string {
  return engine.phase === "connecting" && engine.restart ? "Restarting the server…" : "Starting the server…";
}
