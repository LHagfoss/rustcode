// What the Devices view says, without a window: the glyph a platform gets, the
// chip and the line under a machine's name, the last error in words, and the
// version sentence when the two ends differ.

import { describe, expect, test } from "bun:test";
import type { SessionSummary } from "../acp/types";
import {
  deviceGlyph,
  deviceMeta,
  devicePlatform,
  deviceRoute,
  deviceRouteName,
  deviceStatus,
  localDeviceStatus,
  machineThreadCounts,
  offlineReason,
  routePort,
  threadCountLabel,
  versionLine,
} from "./deviceRows";
import { DEV_APP_VERSION } from "./machines";
import { LOCAL_HOST, threadKey } from "./threadKeys";

const LINUX = "gw-7c1d9e4f2a8b6d30";
const NOW = Date.parse("2026-11-08T12:00:00Z");

function thread(id: string, host: string | undefined, status?: SessionSummary["status"]): SessionSummary {
  return { id: threadKey(host ?? LOCAL_HOST, id), host, cwd: "/code", title: id, status };
}

describe("a device's glyph", () => {
  test("is the platform's own, and a question mark when it is unknown", () => {
    expect(deviceGlyph("macos")).toBe("laptop");
    expect(deviceGlyph("darwin")).toBe("laptop");
    expect(deviceGlyph("windows")).toBe("tower");
    expect(deviceGlyph("linux")).toBe("linux");
    expect(deviceGlyph("unknown")).toBe("unknown");
    expect(deviceGlyph(null)).toBe("unknown");
  });

  test("is named the way people say it", () => {
    expect(devicePlatform("macos")).toBe("Mac");
    expect(devicePlatform("windows")).toBe("Windows");
    expect(devicePlatform("linux")).toBe("Linux");
    expect(devicePlatform("beos")).toBe("Beos");
    expect(devicePlatform("unknown")).toBe("Unknown platform");
  });
});

describe("a device's chip", () => {
  test("says the state, and the tone follows it", () => {
    expect(deviceStatus({ state: "connected", retryInSecs: null })).toEqual({
      label: "Connected",
      tone: "ok",
    });
    expect(deviceStatus({ state: "connecting", retryInSecs: null })).toEqual({
      label: "Connecting",
      tone: "idle",
    });
    expect(deviceStatus({ state: "offline", retryInSecs: null })).toEqual({
      label: "Offline",
      tone: "warn",
    });
    // The shell retries it on its own: reconnecting, not a failure to act on.
    expect(deviceStatus({ state: "offline", retryInSecs: 4 })).toEqual({
      label: "Reconnecting…",
      tone: "idle",
    });
    expect(deviceStatus({ state: "needsPairing", retryInSecs: null })).toEqual({
      label: "Needs pairing",
      tone: "bad",
    });
    expect(deviceStatus({ state: "stopped", retryInSecs: null })).toEqual({
      label: "Not connected",
      tone: "idle",
    });
  });

  test("this computer's follows the app's own engine", () => {
    expect(localDeviceStatus("ready").label).toBe("Connected");
    expect(localDeviceStatus("connecting").label).toBe("Connecting");
    expect(localDeviceStatus("closed").tone).toBe("warn");
  });
});

describe("a device's route", () => {
  test("is named once in a row, and once with its address on the card", () => {
    const route = { kind: "lan", address: "192.168.1.20:17879", encrypted: false };
    expect(deviceRouteName({ route })).toBe("LAN");
    expect(deviceRoute({ route })).toBe("LAN · 192.168.1.20:17879");
    expect(deviceRouteName({ route: null })).toBe("No route saved");
    expect(deviceRoute({ route: null })).toBe("No route saved");
  });

  test("names the port it will fail on", () => {
    expect(routePort("192.168.1.20:17879")).toBe("17879");
    expect(routePort("[fdfd::1]:17879")).toBe("17879");
    expect(routePort("studio.local")).toBeUndefined();
    expect(routePort(null)).toBeUndefined();
  });
});

describe("a device's thread count", () => {
  const sessions = [
    thread("local-1", undefined, "running"),
    thread("local-2", undefined, "idle"),
    thread("linux-1", LINUX, "idle"),
    thread("linux-2", LINUX, "idle"),
    thread("linux-3", LINUX, "idle"),
  ];

  test("counts this computer's own threads", () => {
    expect(machineThreadCounts(sessions, LOCAL_HOST)).toEqual({ running: 1, total: 2 });
  });

  test("counts a machine's, and the ones running there now", () => {
    const live = new Set([threadKey(LINUX, "linux-2")]);
    expect(machineThreadCounts(sessions, LINUX, live)).toEqual({ running: 1, total: 3 });
    expect(machineThreadCounts(sessions, "gw-other")).toEqual({ running: 0, total: 0 });
  });

  test("reads as a number of threads, and how many of them work", () => {
    expect(threadCountLabel({ running: 0, total: 5 })).toBe("5 threads");
    expect(threadCountLabel({ running: 0, total: 1 })).toBe("1 thread");
    expect(threadCountLabel({ running: 2, total: 5 })).toBe("2 of 5 threads running");
    expect(threadCountLabel({ running: 0, total: 0 })).toBe("No threads");
  });
});

describe("the line under a device's name", () => {
  const route = { kind: "lan", address: "192.168.1.20:17879", encrypted: false };

  test("carries the route and the count while it is connected", () => {
    expect(
      deviceMeta(
        { state: "connected", route, lastConnectedAt: "2026-11-08T09:00:00Z" },
        {
          running: 1,
          total: 3,
        },
      ),
    ).toBe("LAN · 1 of 3 threads running");
  });

  test("carries when it last answered while it is away", () => {
    expect(
      deviceMeta(
        { state: "offline", route, lastConnectedAt: "2026-11-08T09:00:00Z" },
        { running: 0, total: 3 },
        NOW,
      ),
    ).toBe("LAN · last seen 3 hours ago");
  });

  test("says only the route when nothing was counted", () => {
    expect(deviceMeta({ state: "connecting", route, lastConnectedAt: null }, { running: 0, total: 0 })).toBe(
      "LAN",
    );
  });
});

describe("why a device is away, in words", () => {
  const route = { kind: "lan", address: "192.168.1.31:17879", encrypted: false };

  test("turns a timeout into the thing to check", () => {
    expect(
      offlineReason({
        state: "offline",
        route,
        detail: "failed to connect: Connection timed out (os error 60)",
      }),
    ).toBe("No answer on port 17879, a firewall may block it.");
  });

  test("says when the machine refused the connection", () => {
    expect(offlineReason({ state: "offline", route, detail: "Connection refused (os error 61)" })).toBe(
      "That computer refused the connection on port 17879. Is RustCode running there?",
    );
  });

  test("a pairing the machine ended is not retried", () => {
    expect(
      offlineReason({
        state: "needsPairing",
        route,
        detail: "Lars's Linux box ended this computer's pairing.",
      }),
    ).toBe("That computer no longer accepts this pairing. Pair it again.");
  });

  test("drops the retry countdown, which the chip and the clock already say", () => {
    expect(offlineReason({ state: "offline", route, detail: "Not connected. Retrying in 4 s." })).toBe(
      "Not reachable. The app keeps trying.",
    );
  });

  test("keeps an error nobody recognised", () => {
    expect(offlineReason({ state: "offline", route, detail: "The machine closed the connection." })).toBe(
      "The machine closed the connection.",
    );
  });

  test("says nothing for a machine that is here", () => {
    expect(offlineReason({ state: "connected", route, detail: "stale" })).toBeNull();
    expect(offlineReason({ state: "connecting", route, detail: null })).toBeNull();
  });
});

describe("two versions", () => {
  test("say which machine runs which", () => {
    expect(versionLine("Studio", "1.29.0", "1.29.1", "This Mac")).toBe("Studio runs 1.29.0, this Mac 1.29.1");
    expect(versionLine("Studio", "1.29.2", "1.29.1", "This PC")).toBe("Studio runs 1.29.2, this PC 1.29.1");
  });

  test("are quiet when they agree, are unknown, or the app is a dev build", () => {
    expect(versionLine("Studio", "1.29.1", "1.29.1", "This Mac")).toBeNull();
    expect(versionLine("Studio", null, "1.29.1", "This Mac")).toBeNull();
    expect(versionLine("Studio", "1.29.0", null, "This Mac")).toBeNull();
    expect(versionLine("Studio", "1.29.0", DEV_APP_VERSION, "This Mac")).toBeNull();
  });
});
