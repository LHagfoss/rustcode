import { expect, test } from "bun:test";
import { connectionDirection, peerInbound, peerOutbound } from "./connectionDirection";

test("direction is relative to this computer, including both and neither", () => {
  expect(connectionDirection(true, false, "Arch")).toBe("Outbound: this computer connects to Arch");
  expect(connectionDirection(false, true, "Arch")).toBe("Inbound: Arch connects to this computer");
  expect(connectionDirection(true, true, "Arch")).toBe(
    "Both ways: this computer connects to Arch; Arch connects here",
  );
  expect(connectionDirection(false, false, "Arch")).toBe(
    "No active connection between this computer and Arch",
  );
});
test("join by host identity, never names or pairing token IDs", () => {
  const devices = [
    {
      id: "token",
      hostId: "arch",
      name: "MacBook",
      kind: "desktop" as const,
      platform: "linux",
      connected: true,
    },
  ];
  expect(peerInbound("arch", devices)).toBe(true);
  expect(peerInbound("token", devices)).toBe(false);
  expect(peerInbound("MacBook", devices)).toBe(false);
  expect(peerInbound("arch", [{ ...devices[0]!, connected: false }])).toBe(false);
  expect(peerInbound("arch", [{ ...devices[0]!, hostId: undefined }])).toBe(false);
  expect(peerOutbound(undefined, [])).toBe(false);
  expect(peerOutbound("arch", [{ id: "arch", state: "connected" }])).toBe(true);
  expect(peerOutbound("arch", [{ id: "arch", state: "offline" }])).toBe(false);
  expect(peerOutbound("other", [{ id: "arch", state: "connected" }])).toBe(false);
});
