// A paired machine's link as one state machine: the phases its channel goes through and the
// pure step between them. `state/machines.ts` keeps one per saved machine and moves it only
// through `transitionLink`.

import type { AcpConnection } from "../acp/jsonrpc";
import type { MachineTransport } from "../acp/transport";
import type { MachineState } from "./machines";

/**
 * Where one saved machine's channel is (issue 3912). Each phase carries only its own data, and
 * `transitionLink` is the one way between them, so a caller never pairs flags by hand:
 *
 * - no entry: never attached, or the last attach failed; the next read attaches it again.
 * - `opening`: `transport.start` is out.
 * - `closed`: the shell's supervisor holds the channel, with no connection right now.
 * - `open`: a connection is up and has not answered `initialize` yet.
 * - `ready`: the connection answered `initialize`; a later listing only asks for sessions.
 * - `removed`: the machine was removed. Anything that arrives for it after that is dropped.
 *
 * An event for a transport or connection that is no longer the current one is dropped too:
 * `transitionLink` then answers the same state object, which is how a caller tells.
 */
export type LinkState<T = MachineTransport, C = AcpConnection> =
  | { phase: "opening"; transport: T }
  | { phase: "closed"; transport: T }
  | { phase: "open"; transport: T; connection: C; initialized: false }
  | { phase: "ready"; transport: T; connection: C }
  | { phase: "removed" };

export type LinkEvent<T = MachineTransport, C = AcpConnection> =
  /** `attach` starts a new channel on `transport`. */
  | { type: "attach"; transport: T }
  /** `transport.start` answered. */
  | { type: "started"; transport: T }
  /** `transport.start` threw. */
  | { type: "failed"; transport: T }
  /** The supervisor opened a connection. */
  | { type: "opened"; transport: T; connection: C }
  /** The supervisor lost it. */
  | { type: "closed"; transport: T }
  /** `connection` answered `initialize`. */
  | { type: "initialized"; connection: C }
  /** Reconnect: the connection is dropped and the supervisor restarted on the same transport. */
  | { type: "reconnect" }
  /** The machine was removed. */
  | { type: "remove" };

/** The pure step between link phases; an event that does not apply answers `state` itself. */
export function transitionLink<T, C>(
  state: LinkState<T, C> | undefined,
  event: LinkEvent<T, C>,
): LinkState<T, C> | undefined {
  switch (event.type) {
    case "attach":
      return !state || state.phase === "removed" ? { phase: "opening", transport: event.transport } : state;
    case "started":
      return state?.phase === "opening" && state.transport === event.transport
        ? { phase: "closed", transport: event.transport }
        : state;
    case "failed":
      return state?.phase === "opening" && state.transport === event.transport ? undefined : state;
    case "opened":
      return state && state.phase !== "removed" && state.transport === event.transport
        ? { phase: "open", transport: event.transport, connection: event.connection, initialized: false }
        : state;
    case "closed":
      // A supervisor can say it is away before `start` answers: the channel is held, closed.
      return state && state.phase !== "removed" && state.transport === event.transport
        ? { phase: "closed", transport: event.transport }
        : state;
    case "initialized":
      return state?.phase === "open" && state.connection === event.connection
        ? { phase: "ready", transport: state.transport, connection: state.connection }
        : state;
    case "reconnect":
      return state?.phase === "open" || state?.phase === "ready" || state?.phase === "closed"
        ? { phase: "closed", transport: state.transport }
        : state;
    case "remove":
      return state?.phase === "removed" ? state : { phase: "removed" };
  }
}

/** The connection a link holds, while it holds one. */
export function linkConnection<C>(state: LinkState<unknown, C> | undefined): C | null {
  return state?.phase === "open" || state?.phase === "ready" ? state.connection : null;
}

/**
 * What the sidebar says of a link, where the link knows: a connection up is connected, a
 * supervisor without one is offline. Anything else is the shell's word (`machines_list`).
 */
export function linkMachineState(state: LinkState<unknown, unknown> | undefined): MachineState | null {
  if (state?.phase === "open" || state?.phase === "ready") return "connected";
  if (state?.phase === "closed") return "offline";
  return null;
}

/** One link per machine id. Only `step` writes it, so every change goes through `transitionLink`. */
export class LinkTable<T = MachineTransport, C = AcpConnection> {
  private readonly links = new Map<string, LinkState<T, C>>();

  get(id: string): LinkState<T, C> | undefined {
    return this.links.get(id);
  }

  /** A copy, so a step taken while iterating does not move the loop. */
  entries(): [string, LinkState<T, C>][] {
    return [...this.links];
  }

  clear(): void {
    this.links.clear();
  }

  /** Apply one event; answers the state before and after (the same object when it was dropped). */
  step(
    id: string,
    event: LinkEvent<T, C>,
  ): { before: LinkState<T, C> | undefined; after: LinkState<T, C> | undefined } {
    const before = this.links.get(id);
    const after = transitionLink(before, event);
    if (after === before) return { before, after };
    if (after) this.links.set(id, after);
    else this.links.delete(id);
    return { before, after };
  }

  /** The connection `transport` holds, while it is still the machine's current channel. */
  connectionOf(id: string, transport: T): C | null {
    const link = this.links.get(id);
    return link && link.phase !== "removed" && link.transport === transport ? linkConnection(link) : null;
  }
}
