import { describe, expect, test } from "bun:test";
import type { RemoteRefusedConnection } from "../acp/remote";
import { expiredCodeCopy, refusalAgo, refusalCopy } from "./refusals";

function refused(overrides: Partial<RemoteRefusedConnection> = {}): RemoteRefusedConnection {
  return {
    reason: "unsupported_version",
    attempts: 1,
    deviceKind: "phone",
    platform: "ios",
    supportedVersions: [1],
    remoteAddr: "192.168.1.31",
    refusedSecsAgo: 12,
    ...overrides,
  };
}

describe("a refused connection", () => {
  test("an older app is told to update, naming both versions", () => {
    const copy = refusalCopy(
      refused({ deviceName: "Lars's iPhone", protocolVersion: 0, supportedVersions: [1] }),
    );
    expect(copy.title).toBe("Lars's iPhone tried to connect with an older app version");
    expect(copy.detail).toContain("version 0");
    expect(copy.detail).toContain("version 1");
    expect(copy.detail).toContain("Update RustCode");
  });

  test("a newer app names the newer side, so the Mac is the one to update", () => {
    const copy = refusalCopy(refused({ protocolVersion: 2, supportedVersions: [1] }));
    expect(copy.title).toBe("A phone tried to connect with a newer app version");
    expect(copy.detail).toContain("version 2");
  });

  test("a version mismatch that names no versions still says what to do", () => {
    const copy = refusalCopy(refused({ protocolVersion: undefined, supportedVersions: [] }));
    expect(copy.title).toBe("A phone tried to connect with a different app version");
    expect(copy.detail).toContain("different remote protocol versions");
    expect(copy.detail).not.toContain("another one");
  });

  test("an unknown or revoked pairing says to pair again", () => {
    const copy = refusalCopy(refused({ reason: "unauthorized", deviceName: "Lars's iPhone" }));
    expect(copy.title).toBe("Lars's iPhone tried to connect with a pairing this computer does not accept");
    expect(copy.detail).toContain("Pair it again");
  });

  test("a secure dial to the plain gateway says to drop https/wss", () => {
    const copy = refusalCopy(refused({ reason: "secure_connection" }));
    expect(copy.title).toContain("wss://");
    expect(copy.detail).toContain("plain WebSocket");
  });

  test("a first message nobody understands points at the app version on both sides", () => {
    const copy = refusalCopy(refused({ reason: "unrecognized_request", deviceName: "Lars's iPhone" }));
    expect(copy.title).toBe("Lars's iPhone sent a first message this computer does not understand");
    expect(copy.detail).toContain("Update RustCode on the device and on this computer");
  });

  test("every reason has words of its own, and never repeats the raw code", () => {
    const reasons: RemoteRefusedConnection["reason"][] = [
      "unsupported_version",
      "pairing_failed",
      "rate_limited",
      "unauthorized",
      "token_replayed",
      "unexpected_origin",
      "secure_connection",
      "unrecognized_request",
      "unknown",
    ];
    const titles = reasons.map((reason) => refusalCopy(refused({ reason })).title);
    expect(new Set(titles).size).toBe(reasons.length);
    for (const title of titles) expect(title).not.toContain("_");
  });

  test("times read as a person says them", () => {
    expect(refusalAgo(0)).toBe("just now");
    expect(refusalAgo(44)).toBe("just now");
    expect(refusalAgo(60)).toBe("1 min ago");
    expect(refusalAgo(300)).toBe("5 min ago");
  });
});

describe("an expired pairing code", () => {
  test("a code nobody dialed in for names the network, with the address", () => {
    const copy = expiredCodeCopy(false, "192.168.1.20:7878");
    expect(copy).toContain("no device connected");
    expect(copy).toContain("same network");
    expect(copy).toContain("192.168.1.20:7878");
  });

  test("a code the phone did reach keeps the plain message", () => {
    expect(expiredCodeCopy(true, "192.168.1.20:7878")).toBe(
      "The last code expired. Pair again for a fresh one.",
    );
    expect(expiredCodeCopy(undefined, undefined)).toBe("The last code expired. Pair again for a fresh one.");
  });
});
