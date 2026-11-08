// Remote access over ACP (docs/remote.md): the engine owns the gateway, the
// shares and pairing, and the desktop only asks (`_rustcode/remote*`) and
// renders the answer. Shapes and parsers; the page calls the methods.
//
// Every method takes an optional `sessionId`: with it they describe and drive
// that thread's share, without it the machine's own switch, gateway and
// challenge (Settings › Remote access).

import { arr, bool, type Json, num, obj, str } from "./jsonrpc";

export const RemoteMethod = {
  /** The machine's switch, gateway and challenge, or this thread's share. */
  status: "_rustcode/remote",
  /** Share the session, starting the gateway if none runs; `pair` asks for an offer too. No session switches the machine on. */
  start: "_rustcode/remote/start",
  /** A fresh pairing challenge on a shared session, or for the machine. */
  pair: "_rustcode/remote/pair",
  /** Turn remote access off for the machine, ending every share. */
  stop: "_rustcode/remote/stop",
} as const;

/** A phone or tablet paired with this Mac, as `_rustcode/remote` names it. */
export interface RemoteDevice {
  id: string;
  name: string;
}

/** The pairing challenge the gateway is counting down, as the machine's answer reports it. */
export interface RemotePairing {
  /** Seconds the challenge has left. */
  expiresInSecs: number;
  /** Wrong codes the challenge still tolerates before a lockout. */
  attemptsLeft?: number;
  /** Seconds pairing stays locked after too many wrong codes, when it is. */
  lockedForSecs?: number;
  challenge?: number;
}

/** Where the gateway listens and who is on it. */
export interface RemoteGateway {
  connected: boolean;
  /** The address a phone dials, when the gateway knows it. */
  advertisedAddress?: string;
  /** The address the gateway bound. */
  listenAddress?: string;
  /** Only this machine can reach it (a Simulator or a test client). */
  loopbackOnly: boolean;
  /** The devices attached to this thread, by name. */
  attachedDevices: string[];
  /** The devices connected to the gateway, by name. */
  connectedDevices: string[];
  /** Devices paired with this Mac (the machine's answer). */
  pairedDevices?: number;
  /** A pairing challenge is open right now (the machine's answer). */
  pairingOpen?: boolean;
  /** The open challenge (the machine's answer). */
  pairing?: RemotePairing;
  /** Threads the gateway holds shared (the machine's answer). */
  sessions?: number;
}

/**
 * One pairing challenge: what a device scans or types, and how long it has.
 * `qrRows` is the code itself, one string per row of `0` (light) and `1`
 * (dark) modules with the quiet zone included, so the app draws it without a
 * QR encoder (`engine/src/remote_gateway/qr.rs`); `qrPayload` is what the code
 * encodes, for an answer that carries no rows.
 */
export interface RemoteOffer {
  qrPayload: string;
  qrRows?: string[];
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

/**
 * The engine's answer to `_rustcode/remote` (status), `/start`, `/pair` and
 * `/stop`. With no `sessionId` it describes the machine; with one, that
 * thread's share.
 */
export interface RemoteStatus {
  /** False on a host without the gateway (Windows, or a build without it). */
  available: boolean;
  /** Remote access is switched on for the machine: the gateway was started and not stopped. */
  enabled: boolean;
  /** This thread is shared with paired devices (threads only). */
  shared: boolean;
  sessionId?: string;
  /** The harness a shared thread runs on (`rustcode` for the built-in one). */
  target?: string;
  gateway?: RemoteGateway;
  offer?: RemoteOffer;
  /** A challenge was asked for and the gateway has not answered yet. */
  pairingPending?: boolean;
  /** The last challenge ran out and is gone from the answer. */
  pairingExpired?: boolean;
  pairingError?: string;
  /** Why sharing ended here (the gateway stopped, another client took the session). */
  error?: string;
  /**
   * One field, two answers: how many threads the host holds back (the
   * machine's), and why this thread is not shared (a thread's).
   */
  notShared?: string | number;
  pairedDevices: number;
  devices: RemoteDevice[];
}

/** How many of the app's open threads the host holds back (the machine's answer). */
export function heldBackCount(status: RemoteStatus | null | undefined): number {
  return typeof status?.notShared === "number" ? status.notShared : 0;
}

/** Why this thread is not shared with paired devices (a thread's answer). */
export function heldBackReason(status: RemoteStatus | null | undefined): string | undefined {
  return typeof status?.notShared === "string" ? status.notShared : undefined;
}

/** The names in a list, dropping anything that is not a string. */
function names(value: Json | undefined): string[] {
  return (
    arr(value)
      ?.map((entry) => str(entry))
      .filter((entry): entry is string => !!entry) ?? []
  );
}

function parseOffer(value: Json | undefined): RemoteOffer | undefined {
  const offer = obj(value);
  const qrPayload = str(offer?.qrPayload);
  const code = str(offer?.code);
  if (!offer || !qrPayload || !code) return undefined;
  return {
    qrPayload,
    qrRows: names(offer.qrRows),
    code,
    address: str(offer.address) ?? "",
    expiresInSecs: num(offer.expiresInSecs) ?? 0,
    loopbackOnly: bool(offer.loopbackOnly) ?? false,
    gatewayId: str(offer.gatewayId) ?? "",
    challenge: num(offer.challenge),
    lockedForSecs: num(offer.lockedForSecs),
  };
}

function parsePairing(value: Json | undefined): RemotePairing | undefined {
  const pairing = obj(value);
  if (!pairing) return undefined;
  return {
    expiresInSecs: num(pairing.expiresInSecs) ?? 0,
    attemptsLeft: num(pairing.attemptsLeft),
    lockedForSecs: num(pairing.lockedForSecs),
    challenge: num(pairing.challenge),
  };
}

function parseGateway(value: Json | undefined): RemoteGateway | undefined {
  const gateway = obj(value);
  if (!gateway) return undefined;
  return {
    connected: bool(gateway.connected) ?? false,
    advertisedAddress: str(gateway.advertisedAddress),
    listenAddress: str(gateway.listenAddress),
    loopbackOnly: bool(gateway.loopbackOnly) ?? false,
    attachedDevices: names(gateway.attachedDevices),
    connectedDevices: names(gateway.connectedDevices),
    pairedDevices: num(gateway.pairedDevices),
    pairingOpen: bool(gateway.pairingOpen),
    pairing: parsePairing(gateway.pairing),
    sessions: num(gateway.sessions),
  };
}

export function parseRemoteStatus(value: Json | undefined): RemoteStatus {
  const root = obj(value) ?? {};
  const heldBack = num(root.notShared) ?? str(root.notShared);
  return {
    available: bool(root.available) ?? true,
    enabled: bool(root.enabled) ?? false,
    shared: bool(root.shared) ?? false,
    sessionId: str(root.sessionId),
    target: str(root.target),
    gateway: parseGateway(root.gateway),
    offer: parseOffer(root.offer),
    pairingPending: bool(root.pairingPending) ?? false,
    pairingExpired: bool(root.pairingExpired) ?? false,
    pairingError: str(root.pairingError),
    error: str(root.error),
    notShared: heldBack,
    pairedDevices: num(root.pairedDevices) ?? 0,
    devices: arr(root.devices).flatMap((device): RemoteDevice[] => {
      const entry = obj(device);
      const id = str(entry?.id);
      if (!id) return [];
      return [{ id, name: str(entry?.name) ?? id }];
    }),
  };
}
