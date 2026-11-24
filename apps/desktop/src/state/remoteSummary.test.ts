import { describe, expect, test } from "bun:test";
import type { RemoteGateway, RemoteStatus } from "../acp/remote";
import type { ServerService } from "./machines";
import { remoteSummary } from "./remoteSummary";

const gateway = (patch: Partial<RemoteGateway> = {}): RemoteGateway => ({
  connected: true,
  advertisedAddress: "192.168.1.24:17879",
  loopbackOnly: false,
  routes: [],
  attachedDevices: [],
  connectedDevices: ["Phone"],
  ...patch,
});

const status = (patch: Partial<RemoteStatus> = {}): RemoteStatus => ({
  available: true,
  enabled: true,
  shared: false,
  gateway: gateway(),
  pairedDevices: 2,
  devices: [],
  connectRequests: [],
  endedConnectRequests: [],
  refusedConnections: [],
  ...patch,
});

const server = (running: boolean) => ({ installed: true, running }) as ServerService;

describe("remoteSummary", () => {
  test("reachable names the address, devices and pairings", () => {
    const s = remoteSummary(status(), server(true));
    expect(s.tone).toBe("ok");
    expect(s.label).toBe("On");
    expect(s.title).toBe("Reachable on your network at 192.168.1.24:17879");
    expect(s.detail).toBe("1 device connected · 2 devices paired");
    expect(s.address).toBe("192.168.1.24:17879");
  });

  test("without the server it says the app has to stay open", () => {
    expect(remoteSummary(status(), server(false)).detail).toContain("Reachable while the app is open");
  });

  test("off with the server running says network access is what is off", () => {
    const s = remoteSummary(status({ enabled: false }), server(true));
    expect(s.label).toBe("Off");
    expect(s.detail).toContain("server is running, but network access is off");
  });

  test("on but not connected yet is starting", () => {
    expect(remoteSummary(status({ gateway: gateway({ connected: false }) }), null).label).toBe("Starting");
  });

  test("a loopback gateway is local only", () => {
    expect(remoteSummary(status({ gateway: gateway({ loopbackOnly: true }) }), null).label).toBe(
      "Local only",
    );
  });

  test("an ended share, a missing gateway and a failed read each say so", () => {
    expect(remoteSummary(status({ error: "The gateway stopped." }), null).detail).toBe(
      "The gateway stopped.",
    );
    expect(remoteSummary(status({ available: false }), null).label).toBe("Unavailable");
    expect(remoteSummary(null, null, "timed out").tone).toBe("bad");
    expect(remoteSummary(null, null).label).toBe("Checking");
  });
});
