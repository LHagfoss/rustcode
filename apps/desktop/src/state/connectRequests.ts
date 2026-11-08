// A phone that asks to connect (docs/remote.md › Connection requests): the gateway holds the
// request, this app asks the user anywhere in the window, and after Accept shows the code the
// phone needs until the phone pairs or the code runs out. What to show is decided here, from the
// machine's `_rustcode/remote` answers, so it is testable without the UI.

import type { ConnectRequestEnd, RemoteConnectRequest, RemoteStatus } from "../acp/remote";

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
 * all (it was read before the answer, or the gateway restarted) keeps the code until it runs out.
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

/** A platform as people say it. */
export function platformLabel(platform: string): string {
  switch (platform.toLowerCase()) {
    case "ios":
      return "iPhone or iPad";
    case "ipados":
      return "iPad";
    case "android":
      return "Android";
    case "macos":
      return "Mac";
    case "unknown":
    case "":
      return "Device";
    default:
      return platform;
  }
}

/** `m:ss` of the seconds left, never negative. */
export function clock(secs: number): string {
  const left = Math.max(0, Math.ceil(secs));
  return `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}`;
}
