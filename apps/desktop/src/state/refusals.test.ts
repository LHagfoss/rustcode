import { describe, expect, test } from "bun:test";
import type { RemoteRefusedConnection } from "../acp/remote";
import { refusalAgo, refusalCopy } from "./refusals";

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

  test("an unknown or revoked pairing says to pair again", () => {
    const copy = refusalCopy(refused({ reason: "unauthorized", deviceName: "Lars's iPhone" }));
    expect(copy.title).toBe("Lars's iPhone tried to connect with a pairing this computer does not accept");
    expect(copy.detail).toContain("Pair it again");
  });

  test("every reason has words of its own, and never repeats the raw code", () => {
    const reasons: RemoteRefusedConnection["reason"][] = [
      "unsupported_version",
      "pairing_failed",
      "rate_limited",
      "unauthorized",
      "token_replayed",
      "unexpected_origin",
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
