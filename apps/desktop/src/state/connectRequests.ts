// A phone that asks to connect (docs/remote.md › Connection requests): the gateway holds the
// request, this app asks the user anywhere in the window, and after Accept shows the code the
// phone needs until the phone pairs or the code runs out. What to show is decided here, from the
// machine's `_rustcode/remote` answers, so it is testable without the UI.

import type {
  ConnectRequestEnd,
  DeviceKind,
  RemoteConnectRequest,
  RemoteDevice,
  RemoteStatus,
} from "../acp/remote";

/** While the gateway runs, a request should reach the user within a few seconds. */
export const CONNECT_POLL_MS = 2500;
/** With remote access off nothing can ask; a slow read notices it being switched on elsewhere. */
export const IDLE_POLL_MS = 15_000;

/** The code this Mac shows after Accept: the only copy, never stored. */
export interface ConnectCode {
  requestId: string;
  deviceName: string;
  code: string;
  /** Gateway process that issued the code. */
  instanceId?: string;
  /** Epoch milliseconds at which the code stops working. */
  expiresAt: number;
  /** Pairing is locked after too many wrong codes; the code works once this runs out. */
  lockedForSecs?: number;
}

/** How often to read the machine's remote status now. */
export function connectPollMs(status: RemoteStatus | null | undefined): number {
  return status?.enabled && status.gateway?.connected ? CONNECT_POLL_MS : IDLE_POLL_MS;
}

/** The requests to ask the user about: waiting for an answer, oldest first. */
export function promptedRequests(status: RemoteStatus | null | undefined): RemoteConnectRequest[] {
  return (status?.connectRequests ?? []).filter((request) => request.state === "pending");
}

export type CodeSettlement =
  | { kind: "open" }
  | { kind: "paired"; deviceName: string }
  | { kind: "ended"; end: ConnectRequestEnd | "stopped"; deviceName: string };

/**
 * Where the code on screen stands, from the latest status: still for the phone to enter, used
 * (the phone paired), or over for another reason. A status that no longer knows the request at
 * all (for example, a read that raced the answer) keeps the code until it runs out. A different
 * gateway instance ends it immediately.
 */
export function settleCode(
  code: ConnectCode,
  status: RemoteStatus | null | undefined,
  now: number,
): CodeSettlement {
  const deviceName = code.deviceName;
  if (status?.gateway?.instanceId && code.instanceId && status.gateway.instanceId !== code.instanceId)
    return { kind: "ended", end: "stopped", deviceName };
  if (status && (!status.enabled || status.gateway?.connected === false || !status.gateway))
    return { kind: "ended", end: "stopped", deviceName };
  const ended = status?.endedConnectRequests.find((request) => request.id === code.requestId);
  if (ended)
    return ended.end === "paired"
      ? { kind: "paired", deviceName }
      : { kind: "ended", end: ended.end, deviceName };
  if (now >= code.expiresAt) return { kind: "ended", end: "expired", deviceName };
  return { kind: "open" };
}

/** What to tell the user when a code they were showing is over without a pairing. */
export function endedMessage(end: ConnectRequestEnd | "stopped", deviceName: string): string {
  switch (end) {
    case "paired":
      return `${deviceName} is paired.`;
    case "denied":
      return `${deviceName} was denied.`;
    case "expired":
      return `The code for ${deviceName} ran out before it was entered.`;
    case "replaced":
      return `The code for ${deviceName} was replaced by a newer pairing code.`;
    case "failed":
      return `${deviceName} entered a wrong code. Ask again from the phone.`;
    case "withdrawn":
      return `${deviceName} stopped connecting.`;
    case "stopped":
      return `Remote access stopped before ${deviceName} paired.`;
  }
}

/** An operating system as people say it, or `undefined` when the device did not say. */
function systemName(platform: string): string | undefined {
  switch (platform.toLowerCase()) {
    case "macos":
    case "darwin":
      return "Mac";
    case "windows":
      return "Windows";
    case "linux":
      return "Linux";
    case "unknown":
    case "":
      return undefined;
    default:
      return platform.charAt(0).toUpperCase() + platform.slice(1);
  }
}

/** What a device is, as people say it: "iPhone", "Android phone", "Windows desktop", "Linux command line". */
export function deviceLabel(kind: DeviceKind, platform: string): string {
  switch (kind) {
    case "phone":
      switch (platform.toLowerCase()) {
        case "ios":
          return "iPhone";
        case "ipados":
          return "iPad";
        case "android":
          return "Android phone";
        default:
          return "Phone";
      }
    case "desktop": {
      const system = systemName(platform);
      return system ? `${system} desktop` : "Desktop app";
    }
    case "cli": {
      const system = systemName(platform);
      return system ? `${system} command line` : "Command line";
    }
    case "unknown":
      return "Unknown device";
  }
}

/** When a paired device was last there: "Connected now", "Last seen 3 h ago", or unknown for an old pairing. */
export function lastSeenLabel(device: Pick<RemoteDevice, "lastSeenAt" | "connected">, now: number): string {
  if (device.connected) return "Connected now";
  const at = device.lastSeenAt;
  if (at === undefined) return "Last seen: unknown";
  const minutes = Math.floor(Math.max(0, now - at) / 60_000);
  if (minutes < 1) return "Last seen just now";
  if (minutes < 60) return `Last seen ${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `Last seen ${hours} h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `Last seen ${days} ${days === 1 ? "day" : "days"} ago`;
  return `Last seen ${new Date(at).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" })}`;
}

/** `m:ss` of the seconds left, never negative. */
export function clock(secs: number): string {
  const left = Math.max(0, Math.ceil(secs));
  return `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}`;
}
