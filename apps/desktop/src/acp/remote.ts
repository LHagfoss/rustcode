// Remote access over ACP (docs/remote.md): the engine owns the gateway, the
// shares and pairing, and the desktop only asks (`_rustcode/remote*`) and
// renders the answer. Shapes and parsers; the page calls the methods.

import { arr, bool, type Json, num, obj, str } from "./jsonrpc";

export const RemoteMethod = {
  /** The gateway and this session's share, with the devices and any pairing offer. */
  status: "_rustcode/remote",
  /** Share the session, starting the gateway if none runs; `pair` asks for an offer too. */
  start: "_rustcode/remote/start",
  /** A fresh pairing challenge on a shared session. */
  pair: "_rustcode/remote/pair",
  /** Turn remote access off for the machine, ending every share. */
  stop: "_rustcode/remote/stop",
} as const;

export interface RemoteDevice {
  id: string;
  name: string;
}

export interface RemoteGateway {
  connected: boolean;
  /** The address a phone dials, when the gateway knows it. */
  advertisedAddress?: string;
  /** Only this machine can reach it (a Simulator or a test client). */
  loopbackOnly: boolean;
  /** Devices attached to this session. */
  attachedDevices: number;
  /** Devices connected to the gateway. */
  connectedDevices: number;
}

export interface RemoteOffer {
  /** The compact JSON a phone scans; it carries the pairing credential. */
  qrPayload: string;
  /** The eight-digit code for typing by hand. */
  code: string;
  address: string;
  /** Seconds the challenge has left when the engine answered. */
  expiresInSecs: number;
  loopbackOnly: boolean;
  gatewayId: string;
  challenge?: number;
  /** Seconds pairing stays locked after too many wrong codes, when it is. */
  lockedForSecs?: number;
}

export interface RemoteStatus {
  /** False on a host without the gateway (Windows, or a build without it). */
  available: boolean;
  /** Remote access is switched on for the machine: the gateway was started and not stopped. */
  enabled: boolean;
  /** This session is shared with paired devices. */
  shared: boolean;
  sessionId: string;
  gateway?: RemoteGateway;
  offer?: RemoteOffer;
  /** A challenge was asked for and the gateway has not answered yet. */
  pairingPending: boolean;
  /** The last challenge ran out and is gone from the answer. */
  pairingExpired: boolean;
  pairingError?: string;
  /** Why sharing ended here (the gateway stopped, another client took the session). */
  error?: string;
  pairedDevices: number;
  devices: RemoteDevice[];
}

export function parseRemoteStatus(value: Json | undefined): RemoteStatus {
  const root = obj(value) ?? {};
  const gateway = obj(root.gateway);
  const offer = obj(root.offer);
  return {
    available: bool(root.available) ?? true,
    enabled: bool(root.enabled) ?? false,
    shared: bool(root.shared) ?? false,
    sessionId: str(root.sessionId) ?? "",
    gateway: gateway
      ? {
          connected: bool(gateway.connected) ?? false,
          advertisedAddress: str(gateway.advertisedAddress),
          loopbackOnly: bool(gateway.loopbackOnly) ?? false,
          attachedDevices: num(gateway.attachedDevices) ?? 0,
          connectedDevices: num(gateway.connectedDevices) ?? 0,
        }
      : undefined,
    offer: parseOffer(offer),
    pairingPending: bool(root.pairingPending) ?? false,
    pairingExpired: bool(root.pairingExpired) ?? false,
    pairingError: str(root.pairingError),
    error: str(root.error),
    pairedDevices: num(root.pairedDevices) ?? 0,
    devices: arr(root.devices).flatMap((device): RemoteDevice[] => {
      const d = obj(device);
      const id = str(d?.id);
      if (!id) return [];
      return [{ id, name: str(d?.name) ?? id }];
    }),
  };
}

function parseOffer(value: Json | undefined): RemoteOffer | undefined {
  const offer = obj(value);
  const qrPayload = str(offer?.qrPayload);
  const code = str(offer?.code);
  if (!offer || !qrPayload || !code) return undefined;
  return {
    qrPayload,
    code,
    address: str(offer.address) ?? "",
    expiresInSecs: num(offer.expiresInSecs) ?? 0,
    loopbackOnly: bool(offer.loopbackOnly) ?? false,
    gatewayId: str(offer.gatewayId) ?? "",
    challenge: num(offer.challenge),
    lockedForSecs: num(offer.lockedForSecs),
  };
}
