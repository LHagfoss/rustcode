import { describe, expect, test } from "bun:test";
import { parseRemoteStatus } from "../acp/remote";
import { activeChallenge, makeChallenge, newPairedDevice } from "./pairingChallenge";

describe("pairing challenge", () => {
  test("survives a status refresh without an offer until expiry", () => {
    const response = parseRemoteStatus({
      offer: { qrPayload: "qr", code: "12345678", address: "host", expiresInSecs: 120 },
    });
    const challenge = makeChallenge(response.offer!, 1000);
    const refresh = parseRemoteStatus({ enabled: true, devices: [] });
    expect(refresh.offer).toBeUndefined();
    expect(activeChallenge(challenge, 120_999)?.offer.code).toBe("12345678");
    expect(activeChallenge(challenge, 121_000)).toBeNull();
  });

  test("recognizes a new device by id even when names match", () => {
    const devices = [
      { id: "old", name: "iPhone" },
      { id: "new", name: "iPhone" },
    ];
    expect(newPairedDevice(["old"], devices)?.id).toBe("new");
    expect(newPairedDevice(["old", "new"], devices)).toBeUndefined();
  });
});
