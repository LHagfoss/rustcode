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
  /** Accept or deny a device asking to connect (`{ id, accept }`); accepting answers with the code to show. */
  answer: "_rustcode/remote/answer",
  /** Forget a paired device and close its connections (`{ deviceId }`). */
  revoke: "_rustcode/remote/revoke",
} as const;

/**
 * What a paired device is, as it said when it paired: the phone app, the desktop app on another
 * machine, or the `rustcode` command line there. A label, not a permission. Devices paired
 * before the engine kept it are phones; a kind from a newer engine stays unknown.
 */
export type DeviceKind = "phone" | "desktop" | "cli" | "unknown";

const DEVICE_KINDS: readonly DeviceKind[] = ["phone", "desktop", "cli"];

/** A kind as the engine sends it; an older engine's missing kind is a phone. */
export function parseDeviceKind(value: Json | undefined): DeviceKind {
  const received = str(value);
  if (received === undefined) return "phone";
  return DEVICE_KINDS.find((kind) => kind === received) ?? "unknown";
}

/**
 * A device on the network asking to pair (the machine's answer, `connectRequests`). It carries no
 * secret: accepting it makes the host show a code that only this device's connection can use.
 */
export interface RemoteConnectRequest {
  id: string;
  /** The label the device sent. Not an identity. */
  deviceName: string;
  /** What the device says it is: a phone, desktop app or command line. */
  deviceKind: DeviceKind;
  /** `ios`, `android`, `macos`, `windows`, `linux`, … as the device says. */
  platform: string;
  /** The network address the request came from. */
  remoteAddr: string;
  ageSecs: number;
  /** Seconds left to answer, or once accepted for the device to enter the code. */
  expiresInSecs: number;
  /** `pending` waits for an answer; `approved` has its code on this Mac. */
  state: "pending" | "approved";
}

/** How a connection request ended: `paired`, `denied`, `expired`, `replaced`, `failed` or `withdrawn`. */
export type ConnectRequestEnd = "paired" | "denied" | "expired" | "replaced" | "failed" | "withdrawn";

/** A connection request that ended a moment ago (the machine's answer, `endedConnectRequests`). */
export interface RemoteEndedConnectRequest {
  id: string;
  deviceName: string;
  end: ConnectRequestEnd;
  /** The device it paired, when it did. */
  deviceId?: string;
  endedSecsAgo: number;
}

/** The answer to `_rustcode/remote/answer`. On acceptance `code` is the only copy, for this Mac to show. */
export interface RemoteConnectAnswer {
  id: string;
  deviceName: string;
  /** Gateway process that issued the answer and its code. */
  instanceId?: string;
  accepted: boolean;
  code?: string;
  expiresInSecs?: number;
  /** Pairing is locked after too many wrong codes: the code works once this runs out. */
  lockedForSecs?: number;
}

/** A device paired with this Mac (a phone, desktop app or command line), as `_rustcode/remote` names it. */
export interface RemoteDevice {
  id: string;
  name: string;
  kind: DeviceKind;
  /** `ios`, `windows`, …; `unknown` for a device paired before the engine kept it. */
  platform: string;
  /** The paired machine's own identifier, for a desktop app or command line. */
  hostId?: string;
  /** Epoch milliseconds the device last paired or was connected, to within minutes; absent when unknown. */
  lastSeenAt?: number;
  /** Connected right now (the machine's answer only). */
  connected?: boolean;
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

/** How a device reaches this Mac over one route. */
export type RemoteRouteKind = "lan" | "vpn" | "hostname" | "public" | "unknown";

const ROUTE_KINDS: readonly RemoteRouteKind[] = ["lan", "vpn", "hostname", "public"];

/** One address the phone can dial this Mac at, as the gateway reports it. */
export interface RemoteRoute {
  /** `host:port`. */
  address: string;
  kind: RemoteRouteKind;
}

/** Where the gateway listens and who is on it. */
export interface RemoteGateway {
  connected: boolean;
  /** Changes every time the gateway process starts. */
  instanceId?: string;
  /** The address a phone dials, when the gateway knows it. */
  advertisedAddress?: string;
  /** The address the gateway bound. */
  listenAddress?: string;
  /** Only this machine can reach it (a Simulator or a test client). */
  loopbackOnly: boolean;
  /**
   * The addresses a phone can dial this Mac at, best first: its LAN
   * addresses, a mesh VPN address and its `.local` name. Empty from an older
   * engine and while the gateway listens on loopback only.
   */
  routes: RemoteRoute[];
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
  /** Other addresses the gateway answers on (also in the QR payload), best first. */
  alternates?: string[];
  /** Seconds the challenge has left when the engine answered. */
  expiresInSecs: number;
  loopbackOnly: boolean;
  gatewayId: string;
  instanceId?: string;
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
  /** False from an engine without the gateway (Windows builds up to 1.21). */
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
  /** Devices asking to connect right now (the machine's answer). */
  connectRequests: RemoteConnectRequest[];
  /** Connection requests that ended recently, oldest first (the machine's answer). */
  endedConnectRequests: RemoteEndedConnectRequest[];
  /** The answer to `_rustcode/remote/answer`. */
  connectAnswer?: RemoteConnectAnswer;
  /** Why `_rustcode/remote/answer` did nothing (the request ended meanwhile). */
  connectError?: string;
  /** The device `_rustcode/remote/revoke` forgot. */
  revoked?: Pick<RemoteDevice, "id" | "name">;
  revokeError?: string;
}

const REQUEST_ENDS: readonly ConnectRequestEnd[] = [
  "paired",
  "denied",
  "expired",
  "replaced",
  "failed",
  "withdrawn",
];

function parseConnectRequest(value: Json | undefined): RemoteConnectRequest[] {
  const entry = obj(value);
  const id = str(entry?.id);
  if (!entry || !id) return [];
  return [
    {
      id,
      deviceName: str(entry.deviceName) ?? "A device",
      deviceKind: parseDeviceKind(entry.deviceKind),
      platform: str(entry.platform) ?? "unknown",
      remoteAddr: str(entry.remoteAddr) ?? "",
      ageSecs: num(entry.ageSecs) ?? 0,
      expiresInSecs: num(entry.expiresInSecs) ?? 0,
      state: str(entry.state) === "approved" ? "approved" : "pending",
    },
  ];
}

function parseDevice(value: Json | undefined): RemoteDevice[] {
  const entry = obj(value);
  const id = str(entry?.id);
  if (!entry || !id) return [];
  const device: RemoteDevice = {
    id,
    name: str(entry.name) ?? id,
    kind: parseDeviceKind(entry.kind),
    platform: str(entry.platform) ?? "unknown",
  };
  const hostId = str(entry.hostId);
  if (hostId) device.hostId = hostId;
  const seen = Date.parse(str(entry.lastSeenAt) ?? "");
  if (Number.isFinite(seen)) device.lastSeenAt = seen;
  const connected = bool(entry.connected);
  if (connected !== undefined) device.connected = connected;
  return [device];
}

function parseEndedConnectRequest(value: Json | undefined): RemoteEndedConnectRequest[] {
  const entry = obj(value);
  const id = str(entry?.id);
  const end = REQUEST_ENDS.find((known) => known === str(entry?.end));
  if (!entry || !id || !end) return [];
  return [
    {
      id,
      deviceName: str(entry.deviceName) ?? "A device",
      end,
      deviceId: str(entry.deviceId),
      endedSecsAgo: num(entry.endedSecsAgo) ?? 0,
    },
  ];
}

function parseConnectAnswer(value: Json | undefined): RemoteConnectAnswer | undefined {
  const answer = obj(value);
  const id = str(answer?.id);
  if (!answer || !id) return undefined;
  return {
    id,
    deviceName: str(answer.deviceName) ?? "A device",
    instanceId: str(answer.instanceId),
    accepted: bool(answer.accepted) ?? false,
    code: str(answer.code),
    expiresInSecs: num(answer.expiresInSecs),
    lockedForSecs: num(answer.lockedForSecs),
  };
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
    alternates: names(offer.alternates),
    expiresInSecs: num(offer.expiresInSecs) ?? 0,
    loopbackOnly: bool(offer.loopbackOnly) ?? false,
    gatewayId: str(offer.gatewayId) ?? "",
    instanceId: str(offer.instanceId),
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

/** The routes the gateway answers on, dropping anything malformed. */
function parseRoutes(value: Json | undefined): RemoteRoute[] {
  return (
    arr(value)
      ?.map((entry) => {
        const route = obj(entry);
        const address = str(route?.address);
        if (!route || !address) return undefined;
        const received = str(route.kind);
        const kind = ROUTE_KINDS.find((known) => known === received) ?? "unknown";
        return { address, kind };
      })
      .filter((route): route is RemoteRoute => route !== undefined) ?? []
  );
}

function parseGateway(value: Json | undefined): RemoteGateway | undefined {
  const gateway = obj(value);
  if (!gateway) return undefined;
  return {
    connected: bool(gateway.connected) ?? false,
    instanceId: str(gateway.instanceId),
    advertisedAddress: str(gateway.advertisedAddress),
    listenAddress: str(gateway.listenAddress),
    loopbackOnly: bool(gateway.loopbackOnly) ?? false,
    routes: parseRoutes(gateway.routes),
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
    devices: arr(root.devices).flatMap(parseDevice),
    connectRequests: arr(root.connectRequests).flatMap(parseConnectRequest),
    endedConnectRequests: arr(root.endedConnectRequests).flatMap(parseEndedConnectRequest),
    connectAnswer: parseConnectAnswer(root.connectAnswer),
    connectError: str(root.connectError),
    revoked: (() => {
      const revoked = obj(root.revoked);
      const id = str(revoked?.deviceId);
      return id ? { id, name: str(revoked?.deviceName) ?? id } : undefined;
    })(),
    revokeError: str(root.revokeError),
  };
}
