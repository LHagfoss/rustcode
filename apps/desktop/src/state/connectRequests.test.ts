import { describe, expect, test } from "bun:test";
import { parseRemoteStatus, type RemoteStatus } from "../acp/remote";
import {
  CONNECT_POLL_MS,
  type ConnectCode,
  clock,
  connectPollMs,
  deviceLabel,
  endedMessage,
  IDLE_POLL_MS,
  lastSeenLabel,
  promptedRequests,
  settleCode,
} from "./connectRequests";

function status(extra: Record<string, unknown> = {}): RemoteStatus {
  return parseRemoteStatus({
    available: true,
    enabled: true,
    shared: false,
    gateway: { connected: true, loopbackOnly: false, attachedDevices: [], connectedDevices: [] },
    pairedDevices: 0,
    devices: [],
    ...extra,
  });
}

const request = (id: string, state: "pending" | "approved") => ({
  id,
  deviceName: "Lars's iPhone",
  platform: "ios",
  remoteAddr: "192.168.1.31",
  ageSecs: 4,
  expiresInSecs: 116,
  state,
});

const code: ConnectCode = {
  requestId: "r1",
  deviceName: "Lars's iPhone",
  code: "4821-9034",
  expiresAt: 120_000,
};

describe("connection requests", () => {
  test("the status carries open and ended requests", () => {
    const read = status({
      connectRequests: [request("r1", "pending"), { id: "" }, request("r2", "approved")],
      endedConnectRequests: [
        { id: "r0", deviceName: "Tablet", end: "paired", deviceId: "dev-1", endedSecsAgo: 3 },
        { id: "rx", deviceName: "Tablet", end: "exploded", endedSecsAgo: 3 },
      ],
    });
    expect(read.connectRequests.map((r) => [r.id, r.state])).toEqual([
      ["r1", "pending"],
      ["r2", "approved"],
    ]);
    expect(read.endedConnectRequests).toEqual([
      { id: "r0", deviceName: "Tablet", end: "paired", deviceId: "dev-1", endedSecsAgo: 3 },
    ]);
    // An older engine says nothing about requests.
    expect(parseRemoteStatus({}).connectRequests).toEqual([]);
  });

  test("the answer carries the code to show", () => {
    const read = status({
      connectAnswer: {
        id: "r1",
        deviceName: "Lars's iPhone",
        instanceId: "gateway-instance-1",
        accepted: true,
        code: "4821-9034",
        expiresInSecs: 120,
      },
    });
    expect(read.connectAnswer).toEqual({
      id: "r1",
      deviceName: "Lars's iPhone",
      instanceId: "gateway-instance-1",
      accepted: true,
      code: "4821-9034",
      expiresInSecs: 120,
      lockedForSecs: undefined,
    });
    expect(status({ connectError: "that connection request is no longer open" }).connectError).toBe(
      "that connection request is no longer open",
    );
  });

  test("only requests waiting for an answer are asked about", () => {
    const read = status({ connectRequests: [request("r1", "pending"), request("r2", "approved")] });
    expect(promptedRequests(read).map((r) => r.id)).toEqual(["r1"]);
    expect(promptedRequests(null)).toEqual([]);
  });

  test("the app reads often while the gateway runs, and rarely otherwise", () => {
    expect(connectPollMs(status())).toBe(CONNECT_POLL_MS);
    expect(connectPollMs(status({ enabled: false }))).toBe(IDLE_POLL_MS);
    expect(connectPollMs(status({ gateway: { connected: false } }))).toBe(IDLE_POLL_MS);
    expect(connectPollMs(null)).toBe(IDLE_POLL_MS);
  });

  test("the code stays while its request is open and the clock runs", () => {
    const read = status({ connectRequests: [request("r1", "approved")] });
    expect(settleCode(code, read, 10_000)).toEqual({ kind: "open" });
    // A read from before the answer does not know the request yet.
    expect(settleCode(code, status(), 10_000)).toEqual({ kind: "open" });
    expect(settleCode(code, null, 10_000)).toEqual({ kind: "open" });
    expect(settleCode(code, read, 120_000)).toEqual({
      kind: "ended",
      end: "expired",
      deviceName: "Lars's iPhone",
    });
  });

  test("the code goes when the phone pairs with it", () => {
    const read = status({
      endedConnectRequests: [
        { id: "r1", deviceName: "Lars's iPhone", end: "paired", deviceId: "d", endedSecsAgo: 1 },
      ],
    });
    expect(settleCode(code, read, 10_000)).toEqual({ kind: "paired", deviceName: "Lars's iPhone" });
  });

  test("the code goes when the gateway restarts", () => {
    const restarted = status({
      gateway: {
        connected: true,
        instanceId: "gateway-instance-2",
        loopbackOnly: false,
        attachedDevices: [],
        connectedDevices: [],
      },
    });
    expect(restarted.gateway?.instanceId).toBe("gateway-instance-2");

    const oldCode = { ...code, instanceId: "gateway-instance-1" };
    expect(settleCode(oldCode, restarted, 10_000)).toEqual({
      kind: "ended",
      end: "stopped",
      deviceName: "Lars's iPhone",
    });
  });

  test("the code goes, and says why, when its request ends otherwise", () => {
    for (const end of ["failed", "withdrawn", "replaced", "expired"] as const) {
      const read = status({
        endedConnectRequests: [{ id: "r1", deviceName: "Lars's iPhone", end, endedSecsAgo: 1 }],
      });
      expect(settleCode(code, read, 10_000)).toEqual({ kind: "ended", end, deviceName: "Lars's iPhone" });
      expect(endedMessage(end, "Lars's iPhone")).toContain("Lars's iPhone");
    }
    expect(settleCode(code, status({ enabled: false }), 10_000)).toEqual({
      kind: "ended",
      end: "stopped",
      deviceName: "Lars's iPhone",
    });
  });

  test("labels and clocks read as people say them", () => {
    expect(deviceLabel("phone", "ios")).toBe("iPhone");
    expect(deviceLabel("phone", "android")).toBe("Android phone");
    expect(deviceLabel("phone", "unknown")).toBe("Phone");
    expect(deviceLabel("desktop", "windows")).toBe("Windows desktop");
    expect(deviceLabel("desktop", "macos")).toBe("Mac desktop");
    expect(deviceLabel("desktop", "linux")).toBe("Linux desktop");
    expect(deviceLabel("desktop", "freebsd")).toBe("Freebsd desktop");
    expect(deviceLabel("desktop", "unknown")).toBe("Desktop app");
    expect(deviceLabel("cli", "linux")).toBe("Linux command line");
    expect(deviceLabel("cli", "")).toBe("Command line");
    expect(clock(119.2)).toBe("2:00");
    expect(clock(61)).toBe("1:01");
    expect(clock(-3)).toBe("0:00");
  });

  test("a desktop asking to connect is labelled as one, and an older engine's request as a phone", () => {
    const asked = status({
      connectRequests: [
        { ...request("r1", "pending"), deviceName: "Lars's PC", deviceKind: "desktop", platform: "windows" },
        request("r2", "pending"),
      ],
    });
    const [desktop, phone] = promptedRequests(asked);
    expect(desktop && deviceLabel(desktop.deviceKind, desktop.platform)).toBe("Windows desktop");
    expect(phone?.deviceKind).toBe("phone");
    expect(phone && deviceLabel(phone.deviceKind, phone.platform)).toBe("iPhone");
  });

  test("last seen reads as a relative time, and a connected device as connected", () => {
    const now = Date.parse("2026-11-08T12:00:00Z");
    const ago = (ms: number) => ({ lastSeenAt: now - ms });
    expect(lastSeenLabel({ connected: true, lastSeenAt: now - 86_400_000 }, now)).toBe("Connected now");
    expect(lastSeenLabel({}, now)).toBe("Last seen: unknown");
    expect(lastSeenLabel(ago(20_000), now)).toBe("Last seen just now");
    expect(lastSeenLabel(ago(12 * 60_000), now)).toBe("Last seen 12 min ago");
    expect(lastSeenLabel(ago(3 * 3_600_000), now)).toBe("Last seen 3 h ago");
    expect(lastSeenLabel(ago(26 * 3_600_000), now)).toBe("Last seen 1 day ago");
    expect(lastSeenLabel(ago(9 * 86_400_000), now)).toBe("Last seen 9 days ago");
    expect(lastSeenLabel(ago(90 * 86_400_000), now)).toMatch(/^Last seen \S/);
    // A clock that ran behind the engine's never reads as the future.
    expect(lastSeenLabel({ lastSeenAt: now + 60_000 }, now)).toBe("Last seen just now");
  });
});
