import { describe, expect, test } from "bun:test";
import {
  type AccountsSignIn,
  type AccountsSignInEvent,
  addRowOpen,
  anyWaiting,
  initialAccountsSignIn,
  SIGN_IN_WAIT_MS,
  transitionAccountsSignIn,
} from "./accountSignIn";

const run = (events: AccountsSignInEvent[], from: AccountsSignIn = initialAccountsSignIn) =>
  events.reduce(transitionAccountsSignIn, from);

describe("account sign-in", () => {
  test("Sign in waits until five minutes from now", () => {
    const state = run([{ type: "startSignIn", id: "work", now: 1000 }]);
    expect(state.accounts.work).toEqual({ phase: "waiting", deadline: 1000 + SIGN_IN_WAIT_MS });
    expect(anyWaiting(state)).toBe(true);
  });

  test("two accounts wait at once, and one signing in leaves the other waiting", () => {
    const both = run([
      { type: "startSignIn", id: "a", now: 0 },
      { type: "startSignIn", id: "b", now: 10 },
    ]);
    expect(both.accounts.a?.phase).toBe("waiting");
    expect(both.accounts.b?.phase).toBe("waiting");
    const aDone = transitionAccountsSignIn(both, { type: "signedIn", ids: ["a"] });
    expect(aDone.accounts.a).toBeUndefined();
    expect(aDone.accounts.b).toEqual({ phase: "waiting", deadline: 10 + SIGN_IN_WAIT_MS });
    expect(anyWaiting(aDone)).toBe(true);
    const bDone = transitionAccountsSignIn(aDone, { type: "signedIn", ids: ["b", "other"] });
    expect(bDone.accounts).toEqual({});
    expect(anyWaiting(bDone)).toBe(false);
  });

  test("each wait times out on its own deadline", () => {
    const both = run([
      { type: "startSignIn", id: "a", now: 0 },
      { type: "startSignIn", id: "b", now: 60_000 },
    ]);
    const atDeadline = transitionAccountsSignIn(both, { type: "timedOut", now: SIGN_IN_WAIT_MS });
    expect(atDeadline).toBe(both);
    const aOut = transitionAccountsSignIn(both, { type: "timedOut", now: SIGN_IN_WAIT_MS + 1 });
    expect(aOut.accounts.a).toEqual({ phase: "gaveUp" });
    expect(aOut.accounts.b?.phase).toBe("waiting");
    const bOut = transitionAccountsSignIn(aOut, { type: "timedOut", now: 60_000 + SIGN_IN_WAIT_MS + 1 });
    expect(bOut.accounts.b).toEqual({ phase: "gaveUp" });
    expect(anyWaiting(bOut)).toBe(false);
  });

  test("Sign in again after giving up waits again", () => {
    const gaveUp = run([
      { type: "startSignIn", id: "a", now: 0 },
      { type: "timedOut", now: SIGN_IN_WAIT_MS + 1 },
      { type: "startSignIn", id: "a", now: SIGN_IN_WAIT_MS + 2 },
    ]);
    expect(gaveUp.accounts.a).toEqual({ phase: "waiting", deadline: 2 * SIGN_IN_WAIT_MS + 2 });
  });

  test("a signed-in read clears a gave-up account, and ignores accounts nobody waits for", () => {
    const gaveUp = run([{ type: "noTerminal", id: "a" }]);
    expect(gaveUp.accounts.a).toEqual({ phase: "gaveUp" });
    expect(transitionAccountsSignIn(gaveUp, { type: "signedIn", ids: ["a"] }).accounts).toEqual({});
    expect(transitionAccountsSignIn(gaveUp, { type: "signedIn", ids: ["b"] })).toBe(gaveUp);
  });
});

describe("add row", () => {
  test("opens for a name and closes on Escape or Cancel", () => {
    const naming = run([{ type: "openAdd" }]);
    expect(naming.add).toEqual({ phase: "naming" });
    expect(addRowOpen(naming.add)).toBe(true);
    const closed = transitionAccountsSignIn(naming, { type: "closeAdd" });
    expect(closed.add).toEqual({ phase: "idle" });
    expect(addRowOpen(closed.add)).toBe(false);
  });

  test("a named add keeps the row open while it runs and closes it when added", () => {
    const adding = run([{ type: "openAdd" }, { type: "startAdd", kind: "named" }]);
    expect(adding.add).toEqual({ phase: "adding", kind: "named", returnTo: "naming" });
    expect(addRowOpen(adding.add)).toBe(true);
    expect(transitionAccountsSignIn(adding, { type: "added" }).add).toEqual({ phase: "idle" });
  });

  test("a failed named add returns to the name field", () => {
    const failed = run([{ type: "openAdd" }, { type: "startAdd", kind: "named" }, { type: "addFailed" }]);
    expect(failed.add).toEqual({ phase: "naming" });
  });

  test("Cancel during an add hides the row, and the add still ends idle", () => {
    const cancelled = run([{ type: "openAdd" }, { type: "startAdd", kind: "named" }, { type: "closeAdd" }]);
    expect(addRowOpen(cancelled.add)).toBe(false);
    expect(cancelled.add.phase).toBe("adding");
    expect(transitionAccountsSignIn(cancelled, { type: "addFailed" }).add).toEqual({ phase: "idle" });
  });

  test("a found add leaves the row as it was", () => {
    const fromIdle = run([{ type: "startAdd", kind: "found" }, { type: "added" }]);
    expect(fromIdle.add).toEqual({ phase: "idle" });
    const fromNaming = run([{ type: "openAdd" }, { type: "startAdd", kind: "found" }]);
    expect(addRowOpen(fromNaming.add)).toBe(true);
    expect(transitionAccountsSignIn(fromNaming, { type: "added" }).add).toEqual({ phase: "naming" });
  });

  test("one add at a time, and a named add needs the name field open", () => {
    const adding = run([{ type: "startAdd", kind: "found" }]);
    expect(transitionAccountsSignIn(adding, { type: "startAdd", kind: "found" })).toBe(adding);
    expect(run([{ type: "startAdd", kind: "named" }])).toBe(initialAccountsSignIn);
    expect(run([{ type: "added" }])).toBe(initialAccountsSignIn);
    expect(run([{ type: "addFailed" }])).toBe(initialAccountsSignIn);
  });

  test("the add row and the sign-ins change independently", () => {
    const state = run([
      { type: "startSignIn", id: "a", now: 0 },
      { type: "openAdd" },
      { type: "startAdd", kind: "named" },
      { type: "added" },
      { type: "startSignIn", id: "b", now: 5 },
    ]);
    expect(state.add).toEqual({ phase: "idle" });
    expect(Object.keys(state.accounts).sort()).toEqual(["a", "b"]);
  });
});
