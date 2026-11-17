import { beforeEach, describe, expect, test } from "bun:test";
import { parseRemoteStatus } from "../acp/remote";
import { applyRemoteStatus, dismissRemoteChallenge, tickRemotePairing } from "./remoteAccess";
import { type PhonePairing, remainingPairingSeconds, transitionPairing } from "./remotePairing";
import { resetForTests, useApp } from "./store";

const offer = parseRemoteStatus({
  offer: { qrPayload: "qr", code: "12345678", address: "host", expiresInSecs: 120, challenge: 7 },
}).offer!;
const snapshot = (extra: object = {}) =>
  parseRemoteStatus({
    enabled: true,
    gateway: { connected: true, pairingOpen: true, pairing: { challenge: 7 } },
    devices: [],
    ...extra,
  });
const idle: PhonePairing = { phase: "idle" };
const asking: PhonePairing = { phase: "asking", askedAt: 1000, baselineIds: ["old"], inFlight: true };
const showing: PhonePairing = {
  phase: "showing",
  challenge: { offer, expiresAt: 121_000, baselineIds: ["old"] },
};

beforeEach(() => resetForTests());

describe("phone pairing transitions", () => {
  test("asks, holds while a read lands, and shows the answer", () => {
    expect(transitionPairing(idle, { type: "ask", now: 1000, baselineIds: ["old"] })).toEqual(asking);
    expect(transitionPairing(asking, { type: "status", status: snapshot(), now: 2000 })).toEqual(asking);
    expect(
      transitionPairing(asking, { type: "status", status: snapshot({ offer }), now: 1000, answer: true }),
    ).toEqual(showing);
    expect(
      transitionPairing(asking, { type: "status", status: snapshot(), now: 1000, answer: true }),
    ).toEqual(idle);
  });

  test("keeps one deadline across reads and detects a newly paired device", () => {
    useApp.setState({ pairing: showing });
    applyRemoteStatus(snapshot({ devices: [{ id: "old", name: "Phone" }] }), 11_000);
    const state = useApp.getState().pairing;
    expect(state.phase).toBe("showing");
    if (state.phase === "showing")
      expect(remainingPairingSeconds(state.challenge.expiresAt, 11_000)).toBe(110);
    applyRemoteStatus(
      snapshot({
        devices: [
          { id: "old", name: "Phone" },
          { id: "new", name: "Phone" },
        ],
      }),
      21_000,
    );
    expect(useApp.getState().pairing).toEqual({ phase: "paired", name: "Phone" });
  });

  test("expires on the clock or engine status, and turns off or dismisses", () => {
    expect(transitionPairing(showing, { type: "tick", now: 120_999 })).toEqual(showing);
    expect(transitionPairing(showing, { type: "tick", now: 121_000 })).toEqual({ phase: "expired" });
    expect(
      transitionPairing(showing, { type: "status", status: snapshot({ pairingExpired: true }), now: 2000 }),
    ).toEqual({ phase: "expired" });
    expect(transitionPairing(showing, { type: "off" })).toEqual(idle);
    expect(
      transitionPairing(showing, { type: "status", status: snapshot({ enabled: false }), now: 2000 }),
    ).toEqual(idle);
    useApp.setState({ pairing: showing });
    dismissRemoteChallenge();
    expect(useApp.getState().pairing).toEqual(idle);
    useApp.setState({ pairing: showing });
    tickRemotePairing(121_000);
    expect(useApp.getState().pairing.phase).toBe("expired");
  });

  test("records failures and clears invalid challenges", () => {
    expect(transitionPairing(asking, { type: "failure", error: "offline" })).toEqual({
      phase: "failed",
      error: "offline",
    });
    expect(
      transitionPairing(showing, { type: "status", status: snapshot({ pairingError: "failed" }), now: 2000 }),
    ).toEqual({ phase: "failed", error: "failed" });
    for (const status of [
      snapshot({ gateway: { connected: true, pairingOpen: false } }),
      snapshot({ gateway: { connected: true, pairingOpen: true, pairing: { challenge: 8 } } }),
      snapshot({ offer: { ...offer, qrPayload: "another" } }),
      snapshot({ error: "stopped" }),
    ]) {
      expect(transitionPairing(showing, { type: "status", status, now: 2000 })).toEqual(idle);
    }
  });

  test("preserves paired state until off or a new request", () => {
    const paired: PhonePairing = { phase: "paired", name: "Phone" };
    expect(transitionPairing(paired, { type: "status", status: snapshot(), now: 2000 })).toEqual(paired);
    expect(transitionPairing(paired, { type: "off" })).toEqual(idle);
    expect(transitionPairing(paired, { type: "ask", now: 1000, baselineIds: ["old"] })).toEqual(asking);
  });
});
