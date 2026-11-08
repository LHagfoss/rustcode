import { beforeEach, describe, expect, test } from "bun:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { parseRemoteStatus } from "../acp/remote";
import { PairingCard } from "../ui/RemoteAccessPage";
import { remainingPairingSeconds } from "./remotePairing";
import { applyRemoteStatus, resetForTests, useApp } from "./store";

const offer = parseRemoteStatus({
  offer: { qrPayload: "qr", code: "12345678", address: "host", expiresInSecs: 120, challenge: 7 },
}).offer!;
const snapshot = (seconds: number, extra: object = {}) =>
  parseRemoteStatus({
    enabled: true,
    gateway: { connected: true, pairingOpen: true, pairing: { expiresInSecs: seconds, challenge: 7 } },
    devices: [],
    ...extra,
  });

beforeEach(() => resetForTests());
describe("remote pairing", () => {
  test("keeps the challenge across page remount and uses one deadline across refreshes", () => {
    useApp.setState({ remoteChallenge: { offer, expiresAt: 121_000, baselineIds: [] } });
    applyRemoteStatus(snapshot(110), 11_000);
    expect(remainingPairingSeconds(useApp.getState().remoteChallenge!.expiresAt, 11_000)).toBe(110);
    applyRemoteStatus(snapshot(60), 61_000);
    const remountedPageChallenge = useApp.getState().remoteChallenge;
    expect(remountedPageChallenge?.offer.code).toBe("12345678");
    expect(remainingPairingSeconds(remountedPageChallenge!.expiresAt, 61_000)).toBe(60);
    applyRemoteStatus(parseRemoteStatus({ enabled: false }), 62_000);
    expect(useApp.getState().remoteChallenge).toBeNull();
  });
  test("renders the remaining time after status updates and remount, then hides a stopped gateway", () => {
    const originalNow = Date.now;
    try {
      useApp.setState({ status: "ready", remoteChallenge: { offer, expiresAt: 121_000, baselineIds: [] } });
      Date.now = () => 11_000;
      applyRemoteStatus(snapshot(110));
      const render = () => {
        const state = useApp.getState();
        const challenge = state.remoteChallenge;
        return challenge
          ? renderToStaticMarkup(
              createElement(PairingCard, {
                offer: challenge.offer,
                expiresAt: challenge.expiresAt,
                pairing: state.remote?.gateway?.pairing,
                now: Date.now(),
                pending: false,
                overdue: false,
                busy: false,
                onRenew: () => {},
                onStop: () => {},
                onDismiss: () => {},
              }),
            )
          : "";
      };
      expect(render()).toContain("Expires in 1:50");
      Date.now = () => 61_000;
      applyRemoteStatus(snapshot(60));
      expect(render()).toContain("Expires in 1:00");
      applyRemoteStatus(parseRemoteStatus({ enabled: false }));
      expect(render()).not.toContain("12345678");
    } finally {
      Date.now = originalNow;
    }
  });
  test("detects a newly paired device by id after a section change", () => {
    useApp.setState({ remoteChallenge: { offer, expiresAt: 121_000, baselineIds: ["old"] } });
    applyRemoteStatus(snapshot(110, { devices: [{ id: "old", name: "iPhone" }] }), 11_000);
    expect(useApp.getState().remoteChallenge).not.toBeNull();
    applyRemoteStatus(
      snapshot(100, {
        devices: [
          { id: "old", name: "iPhone" },
          { id: "new", name: "iPhone" },
        ],
      }),
      21_000,
    );
    expect(useApp.getState().remoteChallenge).toBeNull();
    expect(useApp.getState().remotePaired?.name).toBe("iPhone");
  });
  test("clears a closed, replaced, expired or failed challenge", () => {
    const invalid = [
      snapshot(110, { gateway: { connected: true, pairingOpen: false } }),
      snapshot(110, {
        gateway: { connected: true, pairingOpen: true, pairing: { expiresInSecs: 110, challenge: 8 } },
      }),
      snapshot(110, {
        gateway: {
          connected: true,
          pairingOpen: true,
          pairing: { expiresInSecs: 110, attemptsLeft: 0, challenge: 7 },
        },
      }),
      snapshot(110, { offer: { qrPayload: "another", code: "87654321", expiresInSecs: 110 } }),
      snapshot(110, { pairingExpired: true }),
      snapshot(110, { pairingError: "failed" }),
      snapshot(110, { error: "stopped" }),
    ];
    for (const status of invalid) {
      useApp.setState({ remoteChallenge: { offer, expiresAt: 121_000, baselineIds: [] } });
      applyRemoteStatus(status, 11_000);
      expect(useApp.getState().remoteChallenge).toBeNull();
    }
  });
});
