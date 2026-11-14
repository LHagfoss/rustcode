import type { RemoteDevice } from "../acp/remote";
import type { Machine } from "../state/machines";

/** Gateway devices are inbound; shell machine supervisors dial outbound. Join only by host ID,
 * never by display name or address (both can be shared or changed). */
export function connectionDirection(outbound: boolean, inbound: boolean, peer: string): string {
  if (outbound && inbound) return `Both ways: this computer connects to ${peer}; ${peer} connects here`;
  if (outbound) return `Outbound: this computer connects to ${peer}`;
  if (inbound) return `Inbound: ${peer} connects to this computer`;
  return `No active connection between this computer and ${peer}`;
}

export function peerInbound(id: string, devices: readonly RemoteDevice[]): boolean {
  return devices.some((device) => device.hostId === id && device.connected === true);
}

export function peerOutbound(
  hostId: string | undefined,
  machines: readonly Pick<Machine, "id" | "state">[],
): boolean {
  return !!hostId && machines.some((machine) => machine.id === hostId && machine.state === "connected");
}
