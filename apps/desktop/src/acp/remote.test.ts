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
        instanceId: "gateway-instance-1",
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
    expect(status.gateway?.instanceId).toBe("gateway-instance-1");
    expect(status.gateway?.attachedDevices).toEqual(["Lars's iPhone"]);
    expect(status.gateway?.connectedDevices).toHaveLength(2);
    expect(status.offer?.code).toBe("1234-5678");
    expect(status.offer?.expiresInSecs).toBe(87);
    expect(status.offer?.lockedForSecs).toBe(240);
    // An engine from before devices said what they are: a phone on an unknown platform.
    expect(status.devices).toEqual([
      { id: "device-1", name: "Lars's iPhone", kind: "phone", platform: "unknown" },
    ]);
  });

  test("paired devices carry their kind, platform, host and last seen, as acp/remote.rs sends them", () => {
    const status = parseRemoteStatus({
      available: true,
      enabled: true,
      shared: false,
      pairedDevices: 3,
      devices: [
        {
          id: "device-1",
          name: "Lars's PC",
          kind: "desktop",
          platform: "windows",
          hostId: "host-0123456789abcdef",
          lastSeenAt: "2026-11-08T09:30:00Z",
          connected: false,
        },
        { id: "device-2", name: "box", kind: "cli", platform: "linux", lastSeenAt: null, connected: true },
        { id: "device-3", name: "?", kind: "toaster", platform: "ios", hostId: null, lastSeenAt: "garbage" },
      ],
      connectRequests: [
        {
          id: "r1",
          deviceName: "Lars's PC",
          deviceKind: "desktop",
          platform: "windows",
          remoteAddr: "192.168.1.40",
          ageSecs: 1,
          expiresInSecs: 119,
          state: "pending",
        },
      ],
    });
    expect(status.devices).toEqual([
      {
        id: "device-1",
        name: "Lars's PC",
        kind: "desktop",
        platform: "windows",
        hostId: "host-0123456789abcdef",
        lastSeenAt: Date.parse("2026-11-08T09:30:00Z"),
        connected: false,
      },
      { id: "device-2", name: "box", kind: "cli", platform: "linux", connected: true },
      { id: "device-3", name: "?", kind: "unknown", platform: "ios" },
    ]);
    expect(status.connectRequests[0]?.deviceKind).toBe("desktop");
  });

  test("refused connections keep their reason, the device and the versions", () => {
    const status = parseRemoteStatus({
      available: true,
      enabled: true,
      shared: false,
      refusedConnections: [
        {
          reason: "unsupported_version",
          attempts: 3,
          deviceName: "Lars's iPhone",
          deviceKind: "phone",
          platform: "ios",
          protocolVersion: 2,
          supportedVersions: [1],
          remoteAddr: "192.168.1.31",
          refusedSecsAgo: 40,
        },
        { reason: "from_the_future", platform: "android", remoteAddr: "192.168.1.44" },
      ],
    });
    expect(status.refusedConnections[0]).toEqual({
      reason: "unsupported_version",
      attempts: 3,
      deviceName: "Lars's iPhone",
      deviceKind: "phone",
      platform: "ios",
      protocolVersion: 2,
      supportedVersions: [1],
      remoteAddr: "192.168.1.31",
      refusedSecsAgo: 40,
    });
    // A reason from a newer engine still reads as a refusal, with one try, a
    // phone and no version, so the page can still say something.
    expect(status.refusedConnections[1]).toEqual({
      reason: "unknown",
      attempts: 1,
      deviceName: undefined,
      deviceKind: "phone",
      platform: "android",
      protocolVersion: undefined,
      supportedVersions: [],
      remoteAddr: "192.168.1.44",
      refusedSecsAgo: 0,
    });
    // An engine that does not keep refusals answers without the field.
    expect(parseRemoteStatus({ available: true }).refusedConnections).toEqual([]);
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
