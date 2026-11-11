// The Devices view in the sidebar's foot: this computer first, then every
// paired one, and above a machine's threads a card that says what that machine
// is and how to reach it. What a row says is decided here, so the words can be
// tested without a window (docs/desktop.md › Several machines).
//
// A row is two lines: the platform's glyph and the name, with the state as a
// chip; then the route once, the threads that run on it out of its own, and
// how long ago a machine that is away last answered. An offline row takes a
// third line: the last error in words a reader can act on, and the one action
// that can bring it back. The machine's own card carries the version and
// whether it differs from this app's, because an old RustCode on the other end
// is a sentence, not a silent connection failure.

import type { SessionSummary } from "../acp/types";
import { DEV_APP_VERSION, lastSeen, type Machine, platformName, routeName } from "./machines";
import { LOCAL_HOST } from "./threadKeys";

/** The glyph a machine's platform gets; `unknown` when it has not said. */
export type DeviceGlyph = "laptop" | "monitor" | "terminal" | "unknown";

/** One platform, one glyph: a laptop for macOS, a monitor for Windows, a terminal for Linux. */
export function deviceGlyph(platform: string | null | undefined): DeviceGlyph {
  switch (platform?.toLowerCase()) {
    case "macos":
    case "darwin":
      return "laptop";
    case "windows":
      return "monitor";
    case "linux":
      return "terminal";
    default:
      return "unknown";
  }
}

export interface DeviceStatus {
  /** The chip's words: short, because the row is narrow. */
  label: string;
  tone: "ok" | "warn" | "bad" | "idle";
}

/** The state as the row's chip: `Connected`, `Connecting`, `Offline`, `Needs pairing`. */
export function deviceStatus(machine: Pick<Machine, "state">): DeviceStatus {
  switch (machine.state) {
    case "connected":
      return { label: "Connected", tone: "ok" };
    case "connecting":
      return { label: "Connecting", tone: "idle" };
    case "offline":
      return { label: "Offline", tone: "warn" };
    case "needsPairing":
      return { label: "Needs pairing", tone: "bad" };
    case "stopped":
      return { label: "Not connected", tone: "idle" };
  }
}

/** This computer's chip, from the app's own engine rather than a machine's supervisor. */
export function localDeviceStatus(engine: "connecting" | "ready" | "closed"): DeviceStatus {
  switch (engine) {
    case "ready":
      return { label: "Connected", tone: "ok" };
    case "connecting":
      return { label: "Connecting", tone: "idle" };
    case "closed":
      return { label: "Not connected", tone: "warn" };
  }
}

/** The network once, as a narrow row has room for it: `LAN`, `NetBird`, `VPN`. */
export function deviceRouteName(machine: Pick<Machine, "route">): string {
  return machine.route ? routeName(machine.route) : "No route saved";
}

/** The route and the address it dials, for the machine's own card. */
export function deviceRoute(machine: Pick<Machine, "route">): string {
  if (!machine.route) return "No route saved";
  return `${routeName(machine.route)} · ${machine.route.address}`;
}

/** The threads that run on one computer: how many of them are working now, and how many there are. */
const NO_RUNNING: ReadonlySet<string> = new Set();
export function machineThreadCounts(
  sessions: readonly Pick<SessionSummary, "id" | "host" | "status">[],
  host: string,
  running: ReadonlySet<string> = NO_RUNNING,
): { running: number; total: number } {
  let total = 0;
  let working = 0;
  for (const session of sessions) {
    if ((session.host ?? LOCAL_HOST) !== host) continue;
    total += 1;
    // A session's id is the key the store keeps it under: bare here, `‹host›/‹id›` there.
    if (session.status === "running" || running.has(session.id)) working += 1;
  }
  return { running: working, total };
}

/** `5 threads`, `1 thread`, `2 of 5 threads running`, or `No threads`. */
export function threadCountLabel(counts: { running: number; total: number }): string {
  if (counts.total === 0) return "No threads";
  const threads = `${counts.total} ${counts.total === 1 ? "thread" : "threads"}`;
  return counts.running > 0 ? `${counts.running} of ${threads} running` : threads;
}

/**
 * What a row says under its name: the route once, then how much of it works,
 * and for a machine that is away, when it last answered. A row that never
 * counted its threads says only what it knows.
 */
export function deviceMeta(
  machine: Pick<Machine, "state" | "route" | "lastConnectedAt">,
  counts: { running: number; total: number } | undefined,
  now = Date.now(),
): string {
  const parts = [deviceRouteName(machine)];
  if (machine.state === "connected" && counts) {
    parts.push(threadCountLabel(counts));
  } else if (machine.state !== "connecting" && machine.lastConnectedAt) {
    parts.push(`last seen ${lastSeen(machine.lastConnectedAt, now)}`);
  }
  return parts.join(" · ");
}

/** The port in a `host:port` or `[::1]:port` route, so a reason can name it. */
export function routePort(address: string | null | undefined): string | undefined {
  if (!address) return undefined;
  const match = /:(\d+)\]?$/.exec(address.trim());
  return match?.[1];
}

/**
 * Why a machine is not reachable, in the words a reader can act on. The
 * shell's error is usually a socket string ("Connection timed out"); it is
 * turned into what that means for the reader, and a pairing the machine ended
 * says so instead of inviting another dial. Nothing for a machine that is
 * connected or still trying.
 */
export function offlineReason(machine: Pick<Machine, "state" | "detail" | "route">): string | null {
  if (machine.state === "connected" || machine.state === "connecting") return null;
  if (machine.state === "needsPairing") {
    return "That computer no longer accepts this pairing. Pair it again.";
  }
  const port = routePort(machine.route?.address) ?? "17879";
  const detail = machine.detail?.trim() ?? "";
  // The supervisor's own words are in the detail; a retry countdown is the
  // chip's and the row's own clock, so it is not repeated here.
  const reason = detail
    .replace(/^not connected\.\s*/i, "")
    .replace(/\s*retrying in .*$/i, "")
    .trim();
  if (/timed? ?out|no answer|unreachable|no route|deadline/i.test(reason)) {
    return `No answer on port ${port}, a firewall may block it.`;
  }
  if (/refus/i.test(reason)) {
    return `That computer refused the connection on port ${port}. Is RustCode running there?`;
  }
  if (/pair|unauthor|unauthori[sz]ed|forbidden|\b(?:401|403)\b/i.test(reason)) {
    return "That computer no longer accepts this pairing. Pair it again.";
  }
  return reason || "Not reachable. The app keeps trying.";
}

/**
 * The version sentence when the two ends differ: `Studio runs 1.29.0, this Mac
 * 1.29.1`. Nothing when they agree, when either is unknown, or in a dev build,
 * which is stamped `0.1.0` and would make every machine look newer.
 */
export function versionLine(
  name: string,
  remote: string | null | undefined,
  local: string | null | undefined,
  self: string,
): string | null {
  if (!remote || !local || remote === local || local === DEV_APP_VERSION) return null;
  const thisComputer = self.charAt(0).toLowerCase() + self.slice(1);
  return `${name} runs ${remote}, ${thisComputer} ${local}`;
}

/** The platform as the card says it, with a fallback that does not leave a gap. */
export function devicePlatform(platform: string): string {
  return platformName(platform) ?? "Unknown platform";
}
