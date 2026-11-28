import type { RemoteStatus } from "../acp/remote";
import type { ServerService } from "./machines";

export type RemoteSummaryTone = "ok" | "warn" | "bad" | "idle";

/** Settings › Remote access in one line: where this computer can be reached from, if anywhere. */
export interface RemoteSummary {
  tone: RemoteSummaryTone;
  /** The short state, on the banner's chip: On, Off, Starting, Local only, Stopped. */
  label: string;
  title: string;
  detail: string;
  /** What a phone dials, when there is one to copy. */
  address?: string;
}

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

/**
 * Reads the gateway (`remote`), the background server and a failed read of the gateway into one
 * state, so the page says once whether a phone can reach this computer.
 */
export function remoteSummary(
  remote: RemoteStatus | null,
  server: ServerService | null,
  readError?: string | null,
): RemoteSummary {
  if (readError)
    return { tone: "bad", label: "Unknown", title: "Could not read remote access", detail: readError };
  if (remote === null)
    return { tone: "idle", label: "Checking", title: "Checking remote access…", detail: "" };
  if (!remote.available)
    return {
      tone: "idle",
      label: "Unavailable",
      title: "Remote access is not available here",
      detail: "This build of RustCode has no remote gateway.",
    };
  if (remote.error)
    return { tone: "warn", label: "Stopped", title: "Remote access stopped", detail: remote.error };
  if (!remote.enabled)
    return {
      tone: "idle",
      label: "Off",
      title: "No device can reach this computer",
      detail: server?.running
        ? "The RustCode server is running, but remote access is off. Paired devices stay paired."
        : "Pair a device to open this computer's threads from it.",
    };
  const gateway = remote.gateway;
  if (!gateway?.connected)
    return {
      tone: "warn",
      label: "Starting",
      title: "Starting the gateway…",
      detail: "Waiting for it to announce an address.",
    };
  const connected = gateway.connectedDevices.length;
  const parts = [
    connected ? `${plural(connected, "device")} connected` : "No device connected",
    remote.pairedDevices ? `${plural(remote.pairedDevices, "device")} paired` : "",
    server?.running ? "" : "Reachable while the app is open",
  ].filter(Boolean);
  if (gateway.loopbackOnly)
    return {
      tone: "warn",
      label: "Local only",
      title: "Reachable from this computer only",
      detail: "The gateway listens on loopback, so a phone on the network cannot reach it.",
      address: gateway.advertisedAddress,
    };
  return {
    tone: "ok",
    label: "On",
    title: gateway.advertisedAddress
      ? `Reachable on your network at ${gateway.advertisedAddress}`
      : "Reachable on your network",
    detail: parts.join(" · "),
    address: gateway.advertisedAddress,
  };
}
