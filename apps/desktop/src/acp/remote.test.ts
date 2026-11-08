import { describe, expect, test } from "bun:test";
import { parseRemoteStatus, RemoteMethod } from "./remote";

describe("remote access", () => {
  test("the methods are the engine's ACP extensions", () => {
    expect(RemoteMethod).toEqual({
      status: "_rustcode/remote",
      start: "_rustcode/remote/start",
      pair: "_rustcode/remote/pair",
      stop: "_rustcode/remote/stop",
      answer: "_rustcode/remote/answer",
      revoke: "_rustcode/remote/revoke",
    });
  });

  test("a shared session carries the gateway, the offer and the devices", () => {
    const status = parseRemoteStatus({
      available: true,
      enabled: true,
      shared: true,
      sessionId: "session-1",
      gateway: {
        connected: true,
        advertisedAddress: "192.168.1.24:17879",
        loopbackOnly: false,
        attachedDevices: ["Lars's iPhone"],
        connectedDevices: ["Lars's iPhone", "Kitchen iPad"],
      },
      offer: {
        qrPayload: '{"protocol_version":1}',
        code: "1234-5678",
        address: "192.168.1.24:17879",
        expiresInSecs: 87,
        loopbackOnly: false,
        gatewayId: "gateway-1",
        challenge: 7,
        lockedForSecs: 240,
      },
      pairingPending: false,
      pairingExpired: false,
      pairedDevices: 1,
      devices: [{ id: "device-1", name: "Lars's iPhone" }],
    });
    expect(status.shared).toBe(true);
    expect(status.enabled).toBe(true);
    expect(status.gateway?.advertisedAddress).toBe("192.168.1.24:17879");
    expect(status.gateway?.attachedDevices).toEqual(["Lars's iPhone"]);
    expect(status.gateway?.connectedDevices).toHaveLength(2);
    expect(status.offer?.code).toBe("1234-5678");
    expect(status.offer?.expiresInSecs).toBe(87);
    expect(status.offer?.lockedForSecs).toBe(240);
    expect(status.devices).toEqual([{ id: "device-1", name: "Lars's iPhone" }]);
  });

  test("an idle answer has no gateway or offer, and still lists the devices", () => {
    const status = parseRemoteStatus({
      available: true,
      enabled: false,
      shared: false,
      sessionId: "session-1",
      pairedDevices: 1,
      devices: [{ id: "device-1", name: "Phone" }],
    });
    expect(status.shared).toBe(false);
    expect(status.gateway).toBeUndefined();
    expect(status.offer).toBeUndefined();
    expect(status.devices).toHaveLength(1);
  });

  test("an offer without both the QR payload and the code is not an offer", () => {
    expect(parseRemoteStatus({ offer: { code: "1234-5678" } }).offer).toBeUndefined();
    expect(parseRemoteStatus({ offer: { qrPayload: "{}" } }).offer).toBeUndefined();
  });

  test("a refused pairing keeps the engine's reason", () => {
    const status = parseRemoteStatus({
      available: true,
      shared: false,
      error: "this session is not shared",
    });
    expect(status.error).toBe("this session is not shared");
    expect(status.available).toBe(true);
  });

  test("a host without the gateway is explicit, and unknown fields are ignored", () => {
    const status = parseRemoteStatus({ available: false, future: { anything: true } });
    expect(status.available).toBe(false);
    expect(status.devices).toEqual([]);
    expect(parseRemoteStatus(undefined).shared).toBe(false);
  });
});
