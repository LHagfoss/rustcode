// `transitionLink`, the one way a paired machine's link moves (#3912): each transition, and the
// events it drops for a machine since removed or a connection since replaced.

import { describe, expect, test } from "bun:test";
import { type LinkState, linkConnection, linkMachineState, transitionLink } from "./machineLink";

type State = LinkState<string, string> | undefined;

const T = "transport-1";
const T2 = "transport-2";
const C = "connection-1";
const C2 = "connection-2";

const opening: State = { phase: "opening", transport: T };
const closed: State = { phase: "closed", transport: T };
const open: State = { phase: "open", transport: T, connection: C, initialized: false };
const ready: State = { phase: "ready", transport: T, connection: C };
const removed: State = { phase: "removed" };

describe("transitionLink", () => {
  test("attach opens a channel for a machine with none, or one removed and saved again", () => {
    expect(transitionLink(undefined, { type: "attach", transport: T })).toEqual(opening);
    expect(transitionLink(removed, { type: "attach", transport: T2 })).toEqual({
      phase: "opening",
      transport: T2,
    });
  });

  test("attach leaves a channel that is opening or held alone", () => {
    for (const state of [opening, closed, open, ready]) {
      expect(transitionLink(state, { type: "attach", transport: T2 })).toBe(state);
    }
  });

  test("start answering keeps the channel without a connection", () => {
    expect(transitionLink(opening, { type: "started", transport: T })).toEqual(closed);
  });

  test("a connection that opened during start stays", () => {
    expect(transitionLink(open, { type: "started", transport: T })).toBe(open);
  });

  test("a failed start forgets the link, so the next read attaches again", () => {
    expect(transitionLink(opening, { type: "failed", transport: T })).toBeUndefined();
  });

  test("open: a connection that has not answered initialize", () => {
    expect(transitionLink(opening, { type: "opened", transport: T, connection: C })).toEqual(open);
    expect(transitionLink(closed, { type: "opened", transport: T, connection: C })).toEqual(open);
    expect(transitionLink(ready, { type: "opened", transport: T, connection: C2 })).toEqual({
      phase: "open",
      transport: T,
      connection: C2,
      initialized: false,
    });
  });

  test("initialize answered on the current connection makes it ready", () => {
    expect(transitionLink(open, { type: "initialized", connection: C })).toEqual(ready);
  });

  test("an initialize answer from a previous connection is dropped", () => {
    const reopened: State = { phase: "open", transport: T, connection: C2, initialized: false };
    expect(transitionLink(reopened, { type: "initialized", connection: C })).toBe(reopened);
    expect(transitionLink(closed, { type: "initialized", connection: C })).toBe(closed);
  });

  test("close drops the connection and keeps the channel", () => {
    expect(transitionLink(open, { type: "closed", transport: T })).toEqual(closed);
    expect(transitionLink(ready, { type: "closed", transport: T })).toEqual(closed);
    expect(transitionLink(closed, { type: "closed", transport: T })).toEqual(closed);
    // Away before `start` answered: held and closed, and the answer leaves it so.
    const early = transitionLink(opening, { type: "closed", transport: T });
    expect(early).toEqual(closed);
    expect(transitionLink(early, { type: "started", transport: T })).toBe(early);
  });

  test("reconnect drops the connection; the next open is asked initialize again", () => {
    const reconnecting = transitionLink(ready, { type: "reconnect" });
    expect(reconnecting).toEqual(closed);
    const reopened = transitionLink(reconnecting, { type: "opened", transport: T, connection: C2 });
    expect(reopened?.phase).toBe("open");
  });

  test("remove ends every phase, during attach too", () => {
    for (const state of [undefined, opening, closed, open, ready]) {
      expect(transitionLink(state, { type: "remove" })).toEqual(removed);
    }
    expect(transitionLink(removed, { type: "remove" })).toBe(removed);
  });

  test("nothing arrives for a removed machine", () => {
    expect(transitionLink(removed, { type: "started", transport: T })).toBe(removed);
    expect(transitionLink(removed, { type: "opened", transport: T, connection: C })).toBe(removed);
    expect(transitionLink(removed, { type: "closed", transport: T })).toBe(removed);
    expect(transitionLink(removed, { type: "initialized", connection: C })).toBe(removed);
    expect(transitionLink(removed, { type: "failed", transport: T })).toBe(removed);
    expect(transitionLink(removed, { type: "reconnect" })).toBe(removed);
  });

  test("events from a transport a newer attach replaced are dropped", () => {
    const newer: State = { phase: "opening", transport: T2 };
    expect(transitionLink(newer, { type: "started", transport: T })).toBe(newer);
    expect(transitionLink(newer, { type: "failed", transport: T })).toBe(newer);
    expect(transitionLink(newer, { type: "opened", transport: T, connection: C })).toBe(newer);
    expect(transitionLink(newer, { type: "closed", transport: T })).toBe(newer);
  });
});

describe("what a link says", () => {
  test("its connection, while it holds one", () => {
    expect(linkConnection(open)).toBe(C);
    expect(linkConnection(ready)).toBe(C);
    for (const state of [undefined, opening, closed, removed]) expect(linkConnection(state)).toBeNull();
  });

  test("the sidebar's state, where the link knows", () => {
    expect(linkMachineState(open)).toBe("connected");
    expect(linkMachineState(ready)).toBe("connected");
    expect(linkMachineState(closed)).toBe("offline");
    for (const state of [undefined, opening, removed]) expect(linkMachineState(state)).toBeNull();
  });
});
