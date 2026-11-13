// The other computers this app is paired with (Settings › Machines). One
// saved machine is one entry, reached over an ordered list of routes; the
// app's Rust shell runs a supervisor per machine and relays its ACP here, and
// this module holds what the page shows: the machines, where each connection
// stands, and one ACP client per machine (the same class the local engine
// uses), so a connected machine's `session/list` gives its thread count.
//
// The store routes every thread operation over the same connection this module
// opens (`state/store.ts` `connectionFor`), and the sidebar lists each machine's
// threads in one group (docs/desktop.md › Several machines).
//
// A machine is a computer, a route is one way to reach it, and the server is
// what runs on it (docs/terminology.md).

import { create } from "zustand";
import { AcpConnection, bool, type JsonObject, num, obj, str } from "../acp/jsonrpc";
import { RemoteMethod } from "../acp/remote";
import { isTauri, MachineTransport } from "../acp/transport";
import { initializeParams, Method, parseSessionList, type SessionSummary } from "../acp/types";
import { currentPlatform } from "../lib/platform";
import { hasShell, shellCall, shellCallWithin } from "../lib/shell";
import { appVersion } from "../lib/updater";
import { forgetMachineCache, forgetTranscript, loadThreadList, saveThreadList } from "./machineCache";
import { forgetMachineOutbox, forgetThreadOutbox } from "./machineOutbox";
import {
  APP_VERSION,
  askConfirm,
  deferAnnouncement,
  flushMachineOutbox,
  isTombstoned,
  keptUnlisted,
  noteThreadList,
  request,
  settleKeptUnlisted,
  toast,
  tombstoneThread,
} from "./store";
import { splitThreadKey, threadKey } from "./threadKeys";
import { compareVersions } from "./updates";

/**
 * Where a machine stands, as the shell reports it. `unavailable` is a machine a route reached
 * that would not serve this computer (its server is not running, another protocol version, this
 * device not allowed): `reason` says which, and the shell keeps trying.
 */
export type MachineState =
  | "connecting"
  | "connected"
  | "offline"
  | "unavailable"
  | "needsPairing"
  | "stopped";

/**
 * Why a machine is not connected, as the shell names it: `unreachable` (no route answered),
 * `rejected_token`, `version_mismatch`, `not_allowed`, `server_not_running`, `busy`, `rejected`,
 * or `dropped` (it was up and the connection ended).
 */
export type MachineReason =
  | "unreachable"
  | "rejected_token"
  | "version_mismatch"
  | "not_allowed"
  | "server_not_running"
  | "busy"
  | "rejected"
  | "dropped";

/** One way to reach a machine: a LAN address, Tailscale, a VPN, or public. */
export interface MachineRoute {
  kind: string;
  address: string;
  /** The mesh product the address is on (`netbird`, `tailscale`, `zerotier`), when the machine named it. */
  provider?: string | null;
  /** The network encrypts the traffic (Tailscale, a VPN); a LAN route does not. */
  encrypted: boolean;
  /** What the last test of this route found, or null if it was never tested. */
  probe?: RouteProbe | null;
}

/** The last plain connect to one route: whether it answered and how fast. */
export interface RouteProbe {
  outcome: "reachable" | "refused" | "timeout" | "unreachable";
  /** Milliseconds to connect, or to be turned away; null for a timeout. */
  latencyMs: number | null;
  detail: string | null;
  at: string;
}

/** A saved machine, as `machines_list` reports it. */
export interface Machine {
  id: string;
  /** The name the machine reports. */
  name: string;
  /** The name this app shows, or null while the reported one is shown. */
  displayName?: string | null;
  /** The glyph this app draws, or null for the platform's own. */
  icon?: string | null;
  /** The series tint of that glyph, `1`..`8`. */
  iconTint?: string | null;
  /** `macos`, `windows`, `linux`, or `unknown`. */
  platform: string;
  /** The RustCode version it reported when it last answered, or null. */
  version: string | null;
  /** New threads are not started here from this app; its connection stays. */
  paused?: boolean;
  routes: MachineRoute[];
  /** The route in use, or the one it will try first. */
  route: MachineRoute | null;
  state: MachineState;
  /** Why it is not connected, or null while it is (or has not tried yet). */
  reason?: MachineReason | null;
  detail: string | null;
  /**
   * What to check when the machine is reachable one way and not another, e.g.
   * the LAN works while the NetBird address does not. Set by the shell; the
   * page may show it next to `detail`.
   */
  hint: string | null;
  /** Seconds until the next attempt, while it waits. */
  retryInSecs: number | null;
  addedAt: string;
  lastConnectedAt: string | null;
}

/**
 * A machine the Add sheet can list without an address typed, as
 * `machines_discover` reports it: one Bonjour found on this network, or one
 * whose own handshake reported an overlay address (NetBird, Tailscale) that
 * this app saved for it.
 */
export interface DiscoveredMachine {
  name: string;
  /** `bonjour` for a machine announcing itself here, `overlay` for a saved one reached over a tunnel. */
  source?: "bonjour" | "overlay";
  address: string | null;
  addresses: string[];
  gatewayId: string | null;
  /** The machine's platform, when it is saved and has said. */
  platform?: string | null;
  /** The RustCode version it runs, when it is saved and has answered. */
  version?: string | null;
  /** `lan`, `netbird`, `tailscale`, `zerotier`, `vpn` or `public`. */
  routeKind?: string;
  /** What to check when one kind of route does not answer (PR 3402). */
  hint?: string | null;
  /** This app already has it saved. */
  saved: boolean;
}

/**
 * What `machines_discover` answers: the rows, and what to say when the local
 * search failed — the overlay rows come from this app's registry and are in
 * `rows` either way.
 */
export interface Discovery {
  rows: DiscoveredMachine[];
  notice: string | null;
}

/** This computer's remote gateway, as `server status` reports it (`docs/remote.md`). */
export interface GatewayService {
  /** Remote access is on for this computer. */
  remoteEnabled: boolean;
  /** A gateway answers now, so paired devices can reach this computer. */
  running: boolean;
  /** What devices dial, while it runs. */
  address: string | null;
  advertised: string | null;
  /** Who started it: the server (it outlives the app), the app, or a terminal. */
  owner: "server" | "app" | "terminal" | null;
  version: string | null;
  pid: number | null;
  pairedDevices: number;
  /** The port it starts on from its recorded settings, while it runs not. */
  port: number | null;
}

/** The `rustcode` a shell finds first on `PATH`; an older copy is issue 3411. */
export interface PathRustcode {
  path: string;
  version: string | null;
}

/** The RustCode server as this computer runs it in the background (`server status`). */
export interface ServerService {
  /** A service is in place, so the server starts when you log in. */
  installed: boolean;
  /** A server answers on its socket now. */
  running: boolean;
  /** The running server's version. */
  version: string | null;
  /** The version of the `rustcode` that answered. */
  cliVersion: string | null;
  /** The remote gateway: whether this computer stays reachable with the app closed. */
  gateway: GatewayService;
  /** The `rustcode` on `PATH`, when it is another engine than this app's. */
  pathRustcode: PathRustcode | null;
  /** `linger-disabled`, `version-mismatch`, `path-version-mismatch` and the rest, as `server status` words them. */
  problems: { code: string; message: string }[];
  /** Why the status could not be read, when it could not. */
  error: string | null;
}

/** An accepted connection request, waiting for the code the machine shows. */
export interface AskView {
  askId: string;
  codeLength: number;
  expiresInSecs: number;
}

/**
 * Where the Add machine flow stands. `askId` is this page's own name for the
 * request, made before anything is dialled, so Cancel can drop it at any
 * point — even while the other machine's owner is still deciding. `since` is
 * when the stage began, so the page can say when nothing is coming back.
 */
export type Pairing =
  | { stage: "waiting"; address: string; host: string; askId: string; since: number }
  | {
      stage: "code";
      address: string;
      host: string;
      askId: string;
      codeLength: number;
      expiresAt: number;
    }
  | { stage: "pairing"; address: string; host: string; askId: string; since: number };

/**
 * How long the shell may take to check the code with the machine before the
 * page says it did not hear back. The exchange itself is bounded in the shell
 * (`CONNECT_TIMEOUT`, ten seconds) and so is the token write after it (ten
 * seconds more), so this is longer than the worst case that answers; a command
 * that never returns — a credential store waiting for an unlock, a shell that
 * went away — must not leave the page on a spinner (issue 3325).
 */
export const PAIR_DEADLINE_MS = 30_000;

interface MachinesStore {
  /** The first `machines_list` came back (or failed). */
  ready: boolean;
  error: string | null;
  machines: Machine[];
  /**
   * When each machine was first seen away, in ms: what an offline answer says
   * ("Studio is offline since 09:12"). Seeded from `lastConnectedAt` the first
   * time a machine is seen offline, so it reads as the last time it answered.
   */
  offlineSince: Record<string, number>;
  /** Each connected machine's thread count, from its own `session/list`. */
  counts: Record<string, number>;
  /** Last successfully fetched thread list per machine, retained across disconnects. */
  sessions: Record<string, SessionSummary[]>;
  discovered: DiscoveredMachine[] | null;
  discovering: boolean;
  discoverError: string | null;
  pairing: Pairing | null;
  pairingError: string | null;
  /** The connect code this computer shows for another one, while it is open. */
  connectOffer: ConnectOffer | null;
  /** Creating, withdrawing or redeeming a connect code is running now. */
  connectBusy: boolean;
  connectError: string | null;
  /** The connection over SSH in progress, or the last one: its steps as they go. */
  ssh: SshRun | null;
  sshError: string | null;
  /** This computer's RustCode server as a background service, once asked. */
  server: ServerService | null;
  /** This computer's own name and icon, kept here because it is not paired. */
  local: MachineIdentity;
  /** An install is running now. */
  serverInstalling: boolean;
  /** Pointing the `rustcode` on PATH at this app's engine is running now. */
  serverLinking: boolean;
  /** A route test is running now. */
  probing: boolean;
  /** The Add environment dialog is open (Settings › Environments, or the new-thread selector asked). */
  addOpen: boolean;
}

export const useMachines = create<MachinesStore>(() => ({
  ready: false,
  error: null,
  machines: [],
  offlineSince: {},
  counts: {},
  sessions: {},
  discovered: null,
  discovering: false,
  discoverError: null,
  pairing: null,
  pairingError: null,
  connectOffer: null,
  connectBusy: false,
  connectError: null,
  ssh: null,
  sshError: null,
  server: null,
  serverInstalling: false,
  serverLinking: false,
  probing: false,
  addOpen: false,
  local: readLocalIdentity(),
}));

// What a machine last listed is kept on this computer, so its rows survive the machine going
// away and the app restarting (`state/machineCache.ts`). A live read replaces the rows in memory,
// and this keeps the newest of them.
useMachines.subscribe((state, prev) => {
  if (state.sessions === prev.sessions) return;
  for (const [host, rows] of Object.entries(state.sessions)) {
    if (rows !== prev.sessions[host]) saveThreadList(host, rows);
  }
});

// ---------------------------------------------------------------- labels

/** The id `list_machines` gives this computer: the machine a call is on when it names none. */
export const LOCAL_MACHINE_ID = "local";

/** The glyph a machine gets: one of a small fixed set, so it always draws. */
export type MachineIcon =
  | "laptop"
  | "desktop"
  | "studio"
  | "server"
  | "tower"
  | "linux"
  | "cloud"
  | "phone"
  | "unknown";

/** The icons the picker offers, in the order it shows them. */
export const MACHINE_ICONS: readonly MachineIcon[] = [
  "laptop",
  "desktop",
  "studio",
  "server",
  "tower",
  "linux",
  "cloud",
  "phone",
  "unknown",
];

/** A colour from the app's chart palette, `--rc-series-1`..`8`. */
export type MachineTint = "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8";

export const MACHINE_TINTS: readonly MachineTint[] = ["1", "2", "3", "4", "5", "6", "7", "8"];

/** What this app shows for one computer: its local name and glyph. */
export interface MachineIdentity {
  displayName: string | null;
  icon: MachineIcon | null;
  tint: MachineTint | null;
}

const LOCAL_IDENTITY_KEY = "rustcode.localMachineIdentity";

/** A name as it is stored: trimmed, or null when blank so the fallback shows. */
export function cleanMachineName(value: string | null | undefined): string | null {
  const name = value?.trim();
  return name ? name : null;
}

/** An icon id the app knows, or null for one it does not (an older or newer file). */
export function machineIconOf(value: string | null | undefined): MachineIcon | null {
  return MACHINE_ICONS.includes(value as MachineIcon) ? (value as MachineIcon) : null;
}

/** A tint id the app knows, or null. */
export function machineTintOf(value: string | null | undefined): MachineTint | null {
  return MACHINE_TINTS.includes(value as MachineTint) ? (value as MachineTint) : null;
}

/** The glyph a platform gets when the user has not picked one. */
export function platformIcon(platform: string | null | undefined): MachineIcon {
  switch (platform?.toLowerCase()) {
    case "macos":
    case "darwin":
      return "laptop";
    case "windows":
      return "tower";
    case "linux":
      return "linux";
    default:
      return "unknown";
  }
}

/** The name to show for a paired machine: the local one, else what it reports. */
export function machineName(machine: Pick<Machine, "name" | "displayName">): string {
  return cleanMachineName(machine.displayName) ?? machine.name;
}

/** The glyph to draw for a paired machine: the chosen one, else its platform's. */
export function machineGlyph(machine: Pick<Machine, "platform" | "icon">): MachineIcon {
  return machineIconOf(machine.icon) ?? platformIcon(machine.platform);
}

/** The tint of that glyph, or null for the plain foreground colour. */
export function machineTint(machine: Pick<Machine, "iconTint">): MachineTint | null {
  return machineTintOf(machine.iconTint);
}

function readLocalIdentity(): MachineIdentity {
  try {
    const raw = JSON.parse(localStorage.getItem(LOCAL_IDENTITY_KEY) ?? "null") as unknown;
    const root = obj(raw);
    return {
      displayName: cleanMachineName(typeof root?.displayName === "string" ? root.displayName : null),
      icon: machineIconOf(typeof root?.icon === "string" ? root.icon : null),
      tint: machineTintOf(typeof root?.tint === "string" ? root.tint : null),
    };
  } catch {
    return { displayName: null, icon: null, tint: null };
  }
}

/** This computer's name and glyph, outside React. */
export function localIdentity(): MachineIdentity {
  return useMachines.getState().local;
}

/** This computer's name and glyph in a component, so a rename redraws it. */
export function useLocalIdentity(): MachineIdentity {
  return useMachines((state) => state.local);
}

/** Rename this computer here, or give it a glyph. Stored on this computer only. */
export function setLocalIdentity(patch: Partial<MachineIdentity>): void {
  const current = localIdentity();
  const next: MachineIdentity = {
    displayName: patch.displayName === undefined ? current.displayName : cleanMachineName(patch.displayName),
    icon: patch.icon === undefined ? current.icon : machineIconOf(patch.icon),
    tint: patch.tint === undefined ? current.tint : machineTintOf(patch.tint),
  };
  try {
    localStorage.setItem(LOCAL_IDENTITY_KEY, JSON.stringify(next));
  } catch {
    // A full or disabled store keeps the choice for this run only.
  }
  useMachines.setState({ local: next });
}

/** The name a platform gives this computer before the user renames it. */
export function defaultSelfName(): string {
  switch (currentPlatform()) {
    case "mac":
      return "This Mac";
    case "windows":
      return "This PC";
    default:
      return "This computer";
  }
}

/**
 * This computer's name as people say it (docs/terminology.md): the local entry
 * `list_machines` returns, and what an approval card calls it. Renaming it here
 * changes every place that names it; the platform's default is the fallback.
 */
export function selfName(): string {
  return localIdentity().displayName ?? defaultSelfName();
}

/** Rename a paired machine on this computer. The machine keeps its own name. */
export async function setMachineDisplayName(
  machine: Pick<Machine, "id">,
  displayName: string | null,
): Promise<void> {
  try {
    await shellCallWithin(SHELL_READ_MS, "machines_set_display_name", {
      id: machine.id,
      name: cleanMachineName(displayName),
    });
    await refreshMachines();
  } catch (error) {
    toast(errorText(error), "error");
  }
}

/** Choose a paired machine's glyph and tint, or clear both for its platform's. */
export async function setMachineIcon(
  machine: Pick<Machine, "id">,
  icon: MachineIcon | null,
  tint: MachineTint | null,
): Promise<void> {
  try {
    await shellCallWithin(SHELL_READ_MS, "machines_set_icon", {
      id: machine.id,
      icon: icon ?? null,
      tint: icon ? (tint ?? null) : null,
    });
    await refreshMachines();
  } catch (error) {
    toast(errorText(error), "error");
  }
}

/** When a machine went away, as `09:12`: the time `offlineSince` holds, or undefined. */
export function offlineSinceLabel(at: number | undefined): string | undefined {
  if (at === undefined) return undefined;
  return new Date(at).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

/**
 * Whether a machine that is away is already on its supervisor's retry clock.
 * The app reconnects on its own with backoff (PR 3506), so its state is
 * "reconnecting", not a failure the user has to act on (issue 3487).
 */
export function machineReconnecting(
  machine: Pick<Machine, "state" | "retryInSecs"> | null | undefined,
): boolean {
  return machine?.state === "offline" && !!machine.retryInSecs;
}

/** The same, for the machine a thread runs on, as its key's host (`state/threadKeys.ts`). */
export function useMachineReconnecting(host: string | null | undefined): boolean {
  return useMachines((state) =>
    machineReconnecting(host ? state.machines.find((entry) => entry.id === host) : undefined),
  );
}

/** The words a row's state chip uses. */
export function stateLabel(machine: Pick<Machine, "state" | "retryInSecs" | "reason">): string {
  switch (machine.state) {
    case "connected":
      return "Connected";
    case "connecting":
      return "Connecting";
    case "needsPairing":
      return "Needs pairing";
    case "offline":
      return machineReconnecting(machine) ? "Reconnecting…" : "Offline";
    case "unavailable":
      return reasonLabel(machine.reason);
    case "stopped":
      return "Not connected";
  }
}

/** A machine that answered and would not serve, in a chip's few words. */
export function reasonLabel(reason: MachineReason | null | undefined): string {
  switch (reason) {
    case "server_not_running":
      return "Server not running";
    case "version_mismatch":
      return "Version mismatch";
    case "not_allowed":
      return "Not allowed";
    case "rejected_token":
      return "Pairing refused";
    case "busy":
      return "Busy";
    default:
      return "Refused";
  }
}

/** The chip's tone for a state: `ok` while connected, `warn` once it has given up retrying by itself. */
export function stateTone(machine: Pick<Machine, "state" | "retryInSecs">): "ok" | "warn" | "bad" | "idle" {
  switch (machine.state) {
    case "connected":
      return "ok";
    case "connecting":
      return "idle";
    case "offline":
      return machineReconnecting(machine) ? "idle" : "warn";
    case "unavailable":
      return "warn";
    case "needsPairing":
      return "bad";
    case "stopped":
      return "idle";
  }
}

const ROUTE_NAME: Record<string, string> = {
  lan: "LAN",
  tailscale: "Tailscale",
  netbird: "NetBird",
  zerotier: "ZeroTier",
  vpn: "VPN",
  public: "Public",
};

/**
 * The network a route travels over, as people know it: `LAN`, `NetBird`,
 * `Tailscale`, `ZeroTier`, `VPN`, `Public`. The product the machine named wins
 * over the generic kind: `100.64.0.0/10` is shared by several overlays (PR 3402).
 */
export function routeName(route: MachineRoute | null | undefined, kind?: string): string {
  const wire = route?.provider || route?.kind || kind;
  if (!wire) return "No route saved";
  return ROUTE_NAME[wire] ?? ROUTE_NAME[kind ?? ""] ?? wire;
}

/**
 * One route as people read it: `LAN · Unencrypted`, `NetBird · Encrypted`.
 * Only a LAN route is plain: everything else travels through a network that
 * encrypts it.
 */
export function routeLabel(route: MachineRoute | null | undefined): string {
  if (!route) return "No route saved";
  return `${routeName(route)} · ${route.encrypted ? "Encrypted" : "Unencrypted"}`;
}

/**
 * What one machine row says under its name: the route it is on, or what is
 * wrong. The state itself is the row's chip, so it is not repeated here.
 */
export function machineLine(machine: Machine): string {
  switch (machine.state) {
    // The shell's hint (why another route to this machine is not used) is
    // the row's own line under this one, so it is not repeated here.
    case "connected":
      return routeLabel(machine.route);
    case "connecting":
      return `Connecting over ${routeLabel(machine.route)}`;
    case "needsPairing":
      return machine.detail ?? "This machine no longer accepts this computer's pairing. Pair it again.";
    case "offline":
      return (
        machine.detail ??
        (machineReconnecting(machine) ? "Trying again. The app keeps reconnecting." : "Not connected.")
      );
    case "unavailable":
      return machine.detail ?? `${routeLabel(machine.route)} answered but refused. The app keeps trying.`;
    case "stopped":
      return "Not connected.";
  }
}

/**
 * The hint a machine carries when one kind of route answers and another does
 * not (PR 3402): e.g. the overlay address failed while the LAN works, and what
 * to check on that machine. Absent when every route behaves the same.
 */
export function machineHint(machine: Pick<Machine, "hint">): string | null {
  return machine.hint ?? null;
}

/**
 * This app's own version, for the rows that compare it with another machine's.
 * The app knows it without the network (`tauri getVersion`); a dev build in a
 * browser asks the mock instead, so a mismatched row still shows there.
 */
let localVersion: string | null | undefined;
export async function localAppVersion(): Promise<string | null> {
  if (localVersion !== undefined) return localVersion;
  const own = await appVersion().catch(() => null);
  localVersion = own ?? (hasShell() ? await shellCall<string | null>("app_version").catch(() => null) : null);
  return localVersion;
}

/** For tests: forget the version this app read. */
export function forgetLocalAppVersion(): void {
  localVersion = undefined;
}

/**
 * The version `tauri.conf.json` gives a development build; a released app is
 * stamped with its own version by `scripts/bundle-version.ts`. Every machine
 * would read as newer against it, so the skew line is not shown in a `tauri
 * dev` build (the mock reports a released version and does show it).
 */
export const DEV_APP_VERSION = "0.1.0";

/**
 * A machine running another RustCode version, in words: an older one is
 * updated there, a newer one means this app is the one to update. Version
 * skew is how the Mac Studio kept talking RustCode 0.96 to a 1.28 app without
 * either end saying so.
 */
export function versionSkew(
  remote: string | null | undefined,
  local: string | null | undefined,
  platform?: string | null,
): string | null {
  if (!remote || !local || local === DEV_APP_VERSION) return null;
  const order = compareVersions(remote, local);
  if (order === 0) return null;
  return order < 0
    ? `RustCode ${remote} on that machine — update it there: ${updateAdvice(platform ?? "")}`
    : `RustCode ${remote} on that machine — update RustCode here`;
}

/**
 * How RustCode is updated on a machine of that platform, as docs/install.md
 * gives it: the app's own updater or Homebrew on a Mac, the app's updater or a
 * new AppImage on Linux, the installer on Windows.
 */
export function updateAdvice(platform: string): string {
  switch (platformName(platform)?.toLowerCase()) {
    case "mac":
      return "Settings › About › Check for updates, or `brew upgrade rustcode`";
    case "windows":
      return "run the RustCode installer";
    case "linux":
      return "install the new AppImage, or run `rustcode --update`";
    default:
      return "install the newer RustCode";
  }
}

/**
 * One row per machine, however many ways it was found: Bonjour hears a machine
 * on the LAN, and its own handshake reported the overlay address saved for it.
 * The first row keeps its route (the LAN one, when there is one) and takes
 * what only the other source knew — the version it runs, the platform, the
 * hint — so the Add sheet lists each machine once with everything known of it.
 */
export function mergeDiscovered(rows: DiscoveredMachine[]): DiscoveredMachine[] {
  const byMachine = new Map<string, DiscoveredMachine>();
  for (const row of rows) {
    const key = row.gatewayId ?? row.name;
    const known = byMachine.get(key);
    if (!known) {
      byMachine.set(key, row);
      continue;
    }
    byMachine.set(key, {
      ...known,
      addresses: [...new Set([...known.addresses, ...row.addresses])],
      platform: known.platform ?? row.platform,
      version: known.version ?? row.version,
      hint: known.hint ?? row.hint,
      saved: known.saved || row.saved,
    });
  }
  return [...byMachine.values()];
}

/** The platform as people say it: Mac, Windows, Linux, or nothing when unknown. */
export function platformName(platform: string): string | undefined {
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

/** When a machine was last reached: "just now", "3 min ago", or its date. */
export function lastSeen(iso: string | null, now = Date.now()): string {
  if (!iso) return "Never";
  const at = Date.parse(iso);
  if (Number.isNaN(at)) return "Never";
  const minutes = Math.floor(Math.max(0, now - at) / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} ${hours === 1 ? "hour" : "hours"} ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days} ${days === 1 ? "day" : "days"} ago`;
  return new Date(at).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

/** The threads a connected machine reports, or nothing while it is not connected. */
export function threadCount(machine: Machine, counts: Record<string, number>): number | undefined {
  return machine.state === "connected" ? counts[machine.id] : undefined;
}

/** "3 threads", "1 thread", or nothing before the machine answered. */
export function threadLabel(count: number | undefined): string | undefined {
  if (count === undefined) return undefined;
  return `${count} ${count === 1 ? "thread" : "threads"}`;
}

/** Whether the code on screen is being checked with a machine that has gone quiet. */
export function codeCheckOverdue(pairing: Pairing | null, now = Date.now()): boolean {
  return pairing?.stage === "pairing" && now - pairing.since > PAIR_DEADLINE_MS;
}

// ---------------------------------------------------------------- code input

/**
 * What a code field holds: the digits typed, in order, never more than the
 * machine's code has. A space or a dash is a separator the user typed, and a
 * paste may carry either.
 */
export function codeDigits(raw: string, length: number): string {
  return raw.replace(/\D/g, "").slice(0, Math.max(0, length));
}

/** Whether the code is complete and can be sent. */
export function codeComplete(code: string, length: number): boolean {
  return code.length === length && /^\d+$/.test(code);
}

// ---------------------------------------------------------------- the shell

interface Link {
  transport: MachineTransport;
  connection: AcpConnection | null;
  /** The connection has answered `initialize`: a later listing only asks for sessions. */
  ready: boolean;
}

const links = new Map<string, Link>();
const opening = new Set<string>();
/** Hear when a machine's connection opens or drops, so the store can route its threads there. */
const linkListeners = new Set<(id: string, connection: AcpConnection | null) => void>();
let started = false;
/** Counts asks, so an answer that arrives after Cancel is ignored. */
let asks = 0;

/**
 * A name for one ask, made here rather than by the machine: the request has to
 * be cancellable before the machine has answered, and only this page can name
 * it that early. Plain ASCII, because it travels as a command argument.
 */
export function newAskId(): string {
  asks += 1;
  return `ask-${Date.now().toString(36)}-${asks}`;
}

/**
 * The waits between first-read attempts at app start: 1, 2, 4, 8, 16 and 30
 * seconds, then the app stops trying and the Machines page reads again when it
 * is opened. The same doubling idea the supervisors use, bounded to a start.
 */
export const START_RETRY_WAITS_MS = [1_000, 2_000, 4_000, 8_000, 16_000, 30_000];

/** The app's own wait; a test passes one that does not sleep. */
async function waitMs(ms: number): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, ms));
}

/**
 * Read until the shell answers: `read` says whether it did, and is called once,
 * then again after each of `START_RETRY_WAITS_MS`. Answers at the first truthy
 * read, so a shell that is up at once costs no wait.
 */
export async function readUntilAnswering(
  read: () => Promise<boolean>,
  wait: (ms: number) => Promise<void> = waitMs,
): Promise<void> {
  if (await read()) return;
  for (const ms of START_RETRY_WAITS_MS) {
    await wait(ms);
    if (await read()) return;
  }
}

/**
 * Read the saved machines once, then let each one's supervisor reach the app.
 * Called when the app starts; a machine saved later is picked up by
 * `refreshMachines`.
 *
 * The first read can fail while the shell is still coming up (the registry
 * being written, a config folder that is not there yet). Giving up there would
 * leave every paired machine without a supervisor until a page asked, which is
 * how a machine stayed away after an app update: try again instead, and stop as
 * soon as one read answers.
 */
export async function startMachines(): Promise<void> {
  if (started || !hasShell()) return;
  started = true;
  await readUntilAnswering(async () => {
    await refreshMachines();
    return !useMachines.getState().error;
  });
}

/**
 * How long the app waits on the shell before a Machines page shows an error
 * instead of a spinner. A machine that is away must never hold the page: the
 * shell reads and writes its own files, so a few seconds is generous.
 */
const SHELL_READ_MS = 10_000;
/** Bonjour browses for a few seconds; the rest is the overlay rows. */
const DISCOVER_MS = 20_000;
/** `rustcode server status --json` starts a process. */
const SERVER_STATUS_MS = 15_000;
/** One task per machine runs in parallel, each with its own connect timeout. */
const PROBE_MS = 45_000;

/** Read the machines and their live state from the shell. */
export async function refreshMachines(): Promise<void> {
  if (!hasShell()) return;
  try {
    const machines = await shellCallWithin<Machine[]>(SHELL_READ_MS, "machines_list");
    const now = Date.now();
    const since = { ...useMachines.getState().offlineSince };
    // A machine that was removed leaves nothing behind.
    for (const id of Object.keys(since)) {
      if (!machines.some((machine) => machine.id === id)) delete since[id];
    }
    for (const machine of machines) {
      if (machine.state === "connected") {
        delete since[machine.id];
        continue;
      }
      // The first time this window sees it away: the last time it answered is
      // the closest the shell gives to when it stopped.
      if (since[machine.id] === undefined) {
        const last = Date.parse(machine.lastConnectedAt ?? "");
        since[machine.id] = Number.isNaN(last) ? now : last;
      }
    }
    // A machine this run has not listed yet draws the threads this computer last read from it, so
    // an offline one keeps its rows (and a reconnect replaces them: `readThreads`).
    const state = useMachines.getState();
    const cached: Record<string, SessionSummary[]> = {};
    for (const machine of machines) {
      if (state.sessions[machine.id]) continue;
      const rows = loadThreadList(machine.id);
      if (rows.length) cached[machine.id] = rows;
    }
    useMachines.setState({
      machines,
      offlineSince: since,
      ready: true,
      error: null,
      ...(Object.keys(cached).length ? { sessions: { ...state.sessions, ...cached } } : {}),
    });
    for (const machine of machines) void attach(machine);
    for (const id of [...links.keys()]) {
      if (!machines.some((machine) => machine.id === id)) detach(id);
    }
  } catch (error) {
    useMachines.setState({ ready: true, error: errorText(error) });
  }
}

/**
 * A thread a client just made on a machine (`_rustcode/threads/created`). The machine's server
 * owns its threads, so its list gains the row before any `session/list`: every window drawing
 * that machine shows it at once (docs/desktop.md › Several machines). The row itself is kept by
 * the store's one keep-unlisted set (`state/store.ts`), which a later read merges too.
 */
function rememberCreated(machine: string, params: JsonObject): void {
  const id = str(params.sessionId);
  const cwd = str(params.cwd);
  if (!id || !cwd) return;
  // While this window has a `session/new` out, the creation says which thread is its own.
  if (deferAnnouncement(threadKey(machine, id), () => rememberCreated(machine, params))) return;
  // A thread trashed a moment ago is not made again by a late announcement.
  if (isTombstoned(threadKey(machine, id))) return;
  useMachines.setState((state) => {
    const listed = state.sessions[machine] ?? [];
    // The client that asked for it may have listed it already, through its own `session/new`.
    if (listed.some((session) => session.id === id)) return {};
    const summary: SessionSummary = {
      id,
      cwd,
      updatedAt: Date.now(),
      title: str(params.title) || undefined,
      harness: str(params.harness) || undefined,
      model: str(params.model) || undefined,
    };
    return {
      sessions: { ...state.sessions, [machine]: [summary, ...listed] },
      counts: { ...state.counts, [machine]: listed.length + 1 },
    };
  });
}

/** A thread trashed on a machine (`_rustcode/threads/deleted`): its row leaves this window too. */
function forgetCreated(machine: string, sessionId: string | undefined): void {
  if (!sessionId) return;
  forgetMachineThread(machine, sessionId);
}

/**
 * Take a trashed thread out of a machine's list cache, whether the machine said so or this window
 * trashed it while the link was down, and refuse it to the reads that follow (a `session/list` that
 * was in flight already names it).
 */
export function forgetMachineThread(machine: string, sessionId: string): void {
  tombstoneThread(threadKey(machine, sessionId));
  forgetTranscript(threadKey(machine, sessionId));
  forgetThreadOutbox(threadKey(machine, sessionId));
  useMachines.setState((state) => {
    const listed = state.sessions[machine];
    if (!listed?.some((session) => session.id === sessionId)) return {};
    const sessions = listed.filter((session) => session.id !== sessionId);
    return {
      sessions: { ...state.sessions, [machine]: sessions },
      counts: { ...state.counts, [machine]: sessions.length },
    };
  });
}

/**
 * Give one machine a channel of its own. The supervisor inside the shell keeps
 * retrying on that channel, so it is made once and kept while the machine is
 * saved, whatever state it is in.
 */
async function attach(machine: Machine): Promise<void> {
  if (links.has(machine.id) || opening.has(machine.id)) return;
  opening.add(machine.id);
  const transport = new MachineTransport(machine.id, machine.name);
  const link: Link = { transport, connection: null, ready: false };
  try {
    await transport.start((event) => {
      if (event.type === "open") {
        const connection = new AcpConnection((line) => transport.send(line));
        // Keep the list cache current too: another machine's list refresh must not
        // overwrite a status/deadline just broadcast by this owner, and a thread
        // any client starts there must be listed before the next read.
        connection.onInbound((message) => {
          if (message.type !== "notification") return;
          if (message.method === Method.threadCreated) {
            rememberCreated(machine.id, message.params);
            return;
          }
          if (message.method === Method.threadDeleted) {
            forgetCreated(machine.id, str(message.params.sessionId));
            return;
          }
          if (message.method !== "session/update") return;
          const id = str(message.params.sessionId);
          const update = obj(message.params.update);
          // A title carries no `_meta`; a status and a snooze do.
          const title = str(update?.title);
          const meta = obj(obj(update?._meta)?.rustcode);
          if (!id || (!title && !meta)) return;
          const parsed = meta
            ? parseSessionList({ sessions: [{ sessionId: id, _meta: { rustcode: meta } }] }).sessions[0]
            : undefined;
          useMachines.setState((state) => ({
            sessions: {
              ...state.sessions,
              [machine.id]: (state.sessions[machine.id] ?? []).map((session) => {
                if (session.id !== id) return session;
                return {
                  ...session,
                  ...(title ? { title } : {}),
                  ...(parsed?.status ? { status: parsed.status } : {}),
                  ...(parsed?.snoozedUntil !== undefined ? { snoozedUntil: parsed.snoozedUntil } : {}),
                };
              }),
            },
          }));
        });
        link.connection = connection;
        link.ready = false;
        markMachine(machine.id, "connected", null);
        announce(machine.id, connection);
        void readThreads(machine.id, connection, link);
      } else if (event.type === "line") {
        link.connection?.receive(event.line);
      } else if (event.type === "lines") {
        for (const line of event.lines) link.connection?.receive(line);
      } else if (event.type === "closed") {
        link.connection?.close(event.reason);
        link.connection = null;
        link.ready = false;
        // The sidebar and the chat say the same as the shell's supervisor: it is away, read-only.
        markMachine(machine.id, "offline", event.reason || null);
        announce(machine.id, null);
        useMachines.setState((state) => {
          const counts = { ...state.counts };
          delete counts[machine.id];
          return { counts };
        });
      }
    });
    links.set(machine.id, link);
  } catch (error) {
    useMachines.setState({ error: errorText(error) });
  } finally {
    opening.delete(machine.id);
  }
}

/**
 * What the supervisor saw: the row, the sidebar group and the chat read it from here. A pairing
 * the shell says was revoked stays revoked, and a supervisor still backing off keeps the state and
 * retry clock `machines_list` gave it: only a machine that was connected turns offline here, and
 * nothing but `machines_list` writes `retryInSecs`.
 */
function markMachine(id: string, state: MachineState, detail: string | null): void {
  useMachines.setState((current) => ({
    machines: current.machines.map((machine) => {
      if (machine.id !== id) return machine;
      if (state === "connected")
        return { ...machine, state, detail: null, lastConnectedAt: new Date().toISOString() };
      if (machine.state !== "connected") return machine;
      return { ...machine, state: "offline", detail: detail ?? machine.detail };
    }),
  }));
}

/**
 * Tests only: drop every channel and the lists they gave, as a freshly started app would. The
 * app's own store calls this through `resetForTests` (`state/store.ts`), so one test file's
 * machines never reach another's.
 */
export function resetMachinesForTests(): void {
  for (const [id, link] of [...links]) {
    link.connection?.close("The app reset its machines.");
    void link.transport.stop();
    announce(id, null);
  }
  links.clear();
  opening.clear();
  linkListeners.clear();
  started = false;
  useMachines.setState({
    ready: false,
    error: null,
    machines: [],
    offlineSince: {},
    counts: {},
    sessions: {},
    discovered: null,
    discovering: false,
    discoverError: null,
    pairing: null,
    pairingError: null,
    server: null,
    serverInstalling: false,
    serverLinking: false,
    probing: false,
    addOpen: false,
  });
}

/** Stop a machine's channel: the shell stops its supervisor with it. */
function detach(id: string): void {
  // A machine that is removed leaves nothing of its threads on this computer, and nothing waits
  // to be sent to it.
  forgetMachineCache(id);
  forgetMachineOutbox(id);
  // Nothing of it is listed any more, and that is the whole truth: what waited on its threads goes.
  noteThreadList(id, true);
  const link = links.get(id);
  links.delete(id);
  if (!link) return;
  link.connection?.close("The machine was removed.");
  announce(id, null);
  void link.transport.stop();
  useMachines.setState((state) => {
    const counts = { ...state.counts };
    const sessions = { ...state.sessions };
    delete counts[id];
    delete sessions[id];
    return { counts, sessions };
  });
}

/**
 * The machine's threads, from its own `session/list`. The count is a nicety: a
 * machine that does not answer it is still listed as connected. `initialize`
 * is asked once per connection, so re-listing (a focus, a reconnect) is one
 * request.
 */
async function readThreads(id: string, connection: AcpConnection, link?: Link): Promise<void> {
  try {
    if (!link?.ready) {
      // A machine's server announces its threads to the clients that list them
      // (`_rustcode/threads/created`, docs/desktop.md › Several machines).
      await connection.request("initialize", initializeParams(APP_VERSION, { threads: 1 }), 10_000);
      if (link) link.ready = true;
    }
    const sessions: SessionSummary[] = [];
    let cursor: string | undefined;
    for (let page = 0; page < 10; page++) {
      const answer = await connection.request("session/list", cursor ? { cursor } : {}, 10_000);
      const result = parseSessionList(answer);
      sessions.push(...result.sessions);
      cursor = result.nextCursor;
      if (!cursor) break;
    }
    if (connection.isClosed) return;
    // A cursor left over is a list that stopped at its cap: a thread missing from it is not gone.
    noteThreadList(id, !cursor);
    // Rows the app keeps that this read does not name yet stay: a snapshot from before a thread
    // was announced is not news that it is gone. The ones it names now need no keeping.
    // A thread trashed while this read was out is not news that it came back.
    for (let i = sessions.length - 1; i >= 0; i--) {
      if (isTombstoned(threadKey(id, sessions[i]?.id ?? ""))) sessions.splice(i, 1);
    }
    // What the user changed here while the machine was away goes to it now; a change this list
    // shows the machine overrides is dropped first, so the rows below draw the machine's state.
    void flushMachineOutbox(id, connection, sessions);
    const listed = new Set(sessions.map((session) => session.id));
    settleKeptUnlisted(id, listed);
    const kept = keptUnlisted(id, listed).map((summary) => ({
      ...summary,
      id: splitThreadKey(summary.id).id,
      host: undefined,
    }));
    useMachines.setState((state) => {
      // The kept summary was stored with no status. The cache row has the one a status broadcast
      // wrote (a thread another client announced is running), so keep that, or the row reads idle
      // and its keep expires while the thread still runs.
      const cached = state.sessions[id] ?? [];
      const rows = [
        ...sessions,
        ...kept.map((summary) => {
          const status = cached.find((row) => row.id === summary.id)?.status;
          return status ? { ...summary, status } : summary;
        }),
      ];
      return {
        counts: { ...state.counts, [id]: rows.length },
        sessions: { ...state.sessions, [id]: rows },
      };
    });
  } catch {
    // An older machine, or one whose ACP is not allowed yet: no count.
  }
}

// ---------------------------------------------------------------- live links

/** One saved machine and the ACP connection its supervisor currently holds. */
export interface MachineLink {
  machine: Machine;
  connection: AcpConnection | null;
}

/** The connection to one machine, while it is open. */
export function machineConnection(id: string): AcpConnection | null {
  return links.get(id)?.connection ?? null;
}

/** Every saved machine with its connection, in the sidebar's order. */
export function machineLinks(): MachineLink[] {
  return useMachines.getState().machines.map((machine) => ({
    machine,
    connection: links.get(machine.id)?.connection ?? null,
  }));
}

/**
 * Hear when a machine's connection opens or drops. The store listens to route
 * that machine's thread operations over its own connection (state/store.ts
 * `connectionFor`), and to keep its threads listed while it is away.
 */
export function onMachineLinks(listener: (id: string, connection: AcpConnection | null) => void): () => void {
  linkListeners.add(listener);
  return () => linkListeners.delete(listener);
}

function announce(id: string, connection: AcpConnection | null): void {
  for (const listener of linkListeners) listener(id, connection);
}

/** The least time between two wakes the app sends on its own (a focus, the network coming back). */
const RETRY_GAP_MS = 5_000;
let lastRetryAt = 0;

/**
 * Ask the shell to try machines again now instead of at the end of their backoff: one machine, or
 * every machine waiting when none is named. Answers the ids woken; a machine that is connected,
 * connecting or needs pairing is left alone.
 */
export async function retryMachines(id?: string): Promise<string[]> {
  if (!hasShell()) return [];
  try {
    const woken = await shellCallWithin<string[]>(SHELL_READ_MS, "machines_retry", id ? { id } : {});
    if (woken.length) await refreshMachines();
    return woken;
  } catch {
    return [];
  }
}

/** Whether any saved machine is waiting out a backoff that a wake would end. */
function anyWaiting(): boolean {
  return useMachines
    .getState()
    .machines.some((machine) => machine.state === "offline" || machine.state === "unavailable");
}

// The window coming to the front, or the network coming back, is when a machine that went away is
// most likely back too, and when the person is looking: try it now, not at the end of the wait.
if (typeof window !== "undefined") {
  const wake = () => {
    const now = Date.now();
    if (!started || !anyWaiting() || now - lastRetryAt < RETRY_GAP_MS) return;
    lastRetryAt = now;
    void retryMachines();
  };
  window.addEventListener("focus", wake);
  window.addEventListener("online", wake);
  if (typeof document !== "undefined")
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "visible") wake();
    });
}

/** Re-read every connected machine's thread list (a window focus, a reconnect). */
export function refreshMachineThreads(): void {
  for (const [id, link] of links) if (link.connection) void readThreads(id, link.connection, link);
}

// ---------------------------------------------------------------- actions

/** Look for machines on this network (Bonjour, a few seconds). */
export async function discoverMachines(): Promise<void> {
  useMachines.setState({ discovering: true, discoverError: null });
  try {
    const { rows, notice } = await shellCallWithin<Discovery>(DISCOVER_MS, "machines_discover");
    // A failed browse still lists the overlay rows: the notice says why the
    // local search found nothing, and the saved machines are below it.
    useMachines.setState({ discovered: mergeDiscovered(rows), discoverError: notice, discovering: false });
  } catch (error) {
    useMachines.setState({ discovering: false, discoverError: errorText(error) });
  }
}

/**
 * This computer's RustCode server as a background service. The page asks when
 * it opens and after an install; nothing polls it, because reading it starts
 * a process (`rustcode server status --json`).
 */
export async function refreshServerService(): Promise<void> {
  if (!hasShell()) return;
  try {
    useMachines.setState({
      server: await shellCallWithin<ServerService>(SERVER_STATUS_MS, "server_service_status"),
    });
  } catch (error) {
    useMachines.setState({
      server: {
        installed: false,
        running: false,
        version: null,
        cliVersion: null,
        gateway: emptyGateway(),
        pathRustcode: null,
        problems: [],
        error: errorText(error),
      },
    });
  }
}

/**
 * Install the server as a background service: it stays up when the app closes
 * and starts when you log in, so threads that live in it keep running. The
 * app's own threads stay in its engine until the app is switched to the server. It is the
 * CLI's own `rustcode server install`, so a development build is refused there
 * with the reason shown here. Answers whether it ended up installed.
 */
export async function installServer(): Promise<boolean> {
  useMachines.setState({ serverInstalling: true });
  try {
    const server = await shellCall<ServerService>("server_service_install");
    useMachines.setState({ server, serverInstalling: false });
    if (server.installed) {
      toast(
        "The RustCode server is installed as a background service and starts when you log in. It runs the remote gateway too, while Remote access is on.",
        "success",
      );
    } else {
      toast(server.error ?? "The RustCode server is installed but is not running yet.", "error");
    }
    return server.installed;
  } catch (error) {
    useMachines.setState({ serverInstalling: false });
    toast(`Could not install the RustCode server: ${errorText(error)}`, "error");
    return false;
  }
}

/**
 * Point the `rustcode` a shell runs at this app's engine (issue 3411): the
 * CLI's own `rustcode server link-path`, run by the app's engine so it is the
 * app's version that the shell then finds. Homebrew's copy is refused with the
 * `brew` command. Answers whether the page now reads no version mismatch.
 */
export async function linkCliToApp(): Promise<boolean> {
  useMachines.setState({ serverLinking: true });
  try {
    const server = await shellCall<ServerService>("server_link_path");
    useMachines.setState({ server, serverLinking: false });
    const stale = server.problems.some((problem) => problem.code.startsWith("path-version-mismatch"));
    if (stale) {
      toast("The `rustcode` on PATH still is not this app's version. See the warning on this page.", "error");
    } else {
      toast(
        "`rustcode` in a terminal now runs this app's version. Open a new terminal to use it.",
        "success",
      );
    }
    return !stale;
  } catch (error) {
    useMachines.setState({ serverLinking: false });
    toast(`Could not point \`rustcode\` at this app: ${errorText(error)}`, "error");
    return false;
  }
}

/**
 * Ask before installing the server, in the words that say what it does: it is
 * where threads can live instead of the app, it stays up when the app closes,
 * it starts when you log in, and it runs the remote gateway. The app's own
 * threads are still its engine's.
 */
export function askInstallServer(): void {
  askConfirm({
    title: "Install the RustCode server?",
    message:
      "The RustCode server is where threads can live instead of the app: as a background service it stays up when the app closes and starts when you log in, so those threads keep running. It also runs the remote gateway while Remote access is on, so paired devices still reach this computer while the app is closed. The threads this app starts itself still run in its own engine and end with it, until RustCode uses the server. Threads, settings and pairings stay as they are.",
    action: "Install",
    run: () => void installServer(),
  });
}

/** A gateway this page has no reading for yet. */
function emptyGateway(): GatewayService {
  return {
    remoteEnabled: false,
    running: false,
    address: null,
    advertised: null,
    owner: null,
    version: null,
    pid: null,
    pairedDevices: 0,
    port: null,
  };
}

/** Who started the gateway, in the words the page shows. */
export function gatewayOwnerLabel(owner: GatewayService["owner"]): string {
  switch (owner) {
    case "server":
      return "the RustCode server";
    case "app":
      return "the app";
    case "terminal":
      return "a terminal";
    default:
      return "an unknown process";
  }
}

/**
 * The gateway in one line: where paired devices reach this computer, and who
 * keeps it there. With the server installed and remote access on, that is the
 * server, so the computer stays reachable when the app closes. `serverInstalled`
 * is what makes "the server starts it" honest: with no server installed,
 * nothing restarts it but `rustcode remote start`.
 */
export function gatewayLine(gateway: GatewayService | undefined, serverInstalled = false): string {
  if (!gateway) return "Not checked yet";
  if (gateway.running) {
    const where = gateway.address ?? "an unknown address";
    return `Running on ${where} (started by ${gatewayOwnerLabel(gateway.owner)})`;
  }
  if (gateway.remoteEnabled) {
    const who = serverInstalled ? "the server starts it" : "`rustcode remote start` starts it";
    return gateway.port ? `Not running · ${who} on port ${gateway.port}` : `Not running · ${who}`;
  }
  return "Off";
}

/** The background server in one line: `Installed · Running`, or what is wrong. */
export function serverLine(server: ServerService | null): string {
  if (!server) return "Not checked yet";
  if (server.error && !server.installed && !server.running) return server.error;
  const installed = server.installed ? "Installed" : "Not installed";
  if (!server.installed) return installed;
  return `${installed} · ${server.running ? "Running" : "Not running"}`;
}

/**
 * Ask `address` to let this app in. The request appears on that machine for
 * its owner to accept; once accepted, the code it shows is what pairs it.
 *
 * The shell holds the socket for the whole exchange, so it streams what is
 * happening there: waiting for the owner, then accepted (the code is next).
 * A denial or a request nobody answered comes back as this call's error, in
 * words the page shows where "Waiting for ‹host›…" was.
 */
export async function askMachine(address: string, name?: string): Promise<void> {
  const target = address.trim();
  if (!target) return;
  // One ask at a time: a second press drops the first request (and its socket)
  // instead of leaving two in front of two machines' owners.
  if (useMachines.getState().pairing) cancelPairing();
  const host = name?.trim() || target;
  const askId = newAskId();
  const asked = { address: target, host, askId, since: Date.now() };
  useMachines.setState({ pairing: { stage: "waiting", ...asked }, pairingError: null });
  const current = () => useMachines.getState().pairing?.askId === askId;
  const waiting = (event: PairingEvent) => {
    if (!current()) return;
    if (event.type === "approved")
      useMachines.setState({
        pairing: {
          stage: "code",
          ...asked,
          codeLength: event.codeLength,
          expiresAt: Date.now() + event.expiresInSecs * 1000,
        },
      });
  };
  try {
    let answer: AskView;
    if (isTauri()) {
      const { Channel, invoke } = await import("@tauri-apps/api/core");
      const channel = new Channel<PairingEvent>();
      channel.onmessage = waiting;
      answer = await invoke<AskView>("machines_ask", {
        askId,
        address: target,
        name: name ?? null,
        onEvent: channel,
      });
    } else {
      answer = await shellCall<AskView>("machines_ask", {
        askId,
        address: target,
        name: name ?? null,
      });
    }
    // Without a channel (the mock, `dev:web`) the answer is where the code stage begins.
    waiting({ type: "approved", ...answer });
  } catch (error) {
    // Cancelled: the request was the point, and its refusal is not the user's.
    if (!current()) return;
    useMachines.setState({ pairing: null, pairingError: errorText(error) });
  }
}

/** What the shell streams while a machine is asked. */
type PairingEvent =
  | { type: "waiting"; askId: string; host: string }
  | { type: "approved"; askId: string; codeLength: number; expiresInSecs: number }
  | { type: "refused"; message: string };

/**
 * Give up on a request. The shell is told by its ask id, which it has had since
 * before the machine was dialled: a request the owner has not answered yet
 * loses its socket then and there, and an accepted one loses the one waiting
 * for its code.
 */
export function cancelPairing(): void {
  const pairing = useMachines.getState().pairing;
  useMachines.setState({ pairing: null, pairingError: null });
  if (pairing) void shellCall("machines_cancel", { askId: pairing.askId }).catch(() => undefined);
}

/** Send the code the machine shows. */
export async function pairMachine(code: string): Promise<void> {
  const pairing = useMachines.getState().pairing;
  if (pairing?.stage !== "code") return;
  useMachines.setState({
    pairing: { ...pairing, stage: "pairing", since: Date.now() },
    pairingError: null,
  });
  try {
    const machine = await shellCall<Machine>("machines_pair", {
      askId: pairing.askId,
      code,
    });
    useMachines.setState((state) => ({
      pairing: null,
      machines: [...state.machines.filter((saved) => saved.id !== machine.id), machine],
    }));
    toast(`Connected to ${machine.name}.`, "success");
    await refreshMachines();
  } catch (error) {
    // The request is spent either way; asking again is what recovers it.
    useMachines.setState({ pairing: null, pairingError: errorText(error) });
  }
}

// ---------------------------------------------------------------- connect codes

/**
 * A connect code this computer shows for another computer to pair with it: no request and no
 * accept step, valid ten minutes, once (docs/remote.md › Connect codes). The engine creates it on
 * this computer's gateway; the other computer redeems it with `machines_connect_code` (or
 * `rustcode machines connect`).
 */
export interface ConnectOffer {
  /** The short form, `K7MQ-29XH-TP4W-NRD6`. */
  code: string;
  /** The long form, `rustcode://connect/…`: every address and the gateway's id. */
  link: string;
  /** The address the short code carries, or the one to type next to it. */
  address: string;
  addressInCode: boolean;
  hostName: string | null;
  loopbackOnly: boolean;
  serial: number;
  expiresAt: number;
  /** The computer that paired with it, once one has. */
  pairedWith: string | null;
  /** Its wrong tries ran out. */
  usedUp: boolean;
}

/** Read the engine's `connectCode` answer. */
export function parseConnectOffer(value: unknown, now = Date.now()): ConnectOffer | null {
  const offer = obj(value);
  const code = str(offer?.code);
  const link = str(offer?.link);
  if (!offer || !code || !link) return null;
  return {
    code,
    link,
    address: str(offer.address) ?? "",
    addressInCode: bool(offer.addressInCode) ?? false,
    hostName: str(offer.hostName) ?? null,
    loopbackOnly: bool(offer.loopbackOnly) ?? false,
    serial: num(offer.serial) ?? 0,
    expiresAt: now + (num(offer.expiresInSecs) ?? 600) * 1000,
    pairedWith: null,
    usedUp: false,
  };
}

/** Create a connect code on this computer. Remote access is switched on first if it was off. */
export async function createConnectCode(): Promise<void> {
  useMachines.setState({ connectBusy: true, connectError: null });
  try {
    const answer = obj(await request<JsonObject>(RemoteMethod.connectCode, {}, 20_000));
    const offer = parseConnectOffer(answer?.connectCode);
    const error = str(answer?.connectCodeError);
    useMachines.setState({
      connectOffer: offer,
      connectError: offer ? null : (error ?? "The gateway did not hand out a connect code."),
    });
  } catch (error) {
    useMachines.setState({ connectError: errorText(error) });
  } finally {
    useMachines.setState({ connectBusy: false });
  }
}

/** Withdraw the open connect code, or take a spent one off the screen. */
export async function cancelConnectCode(): Promise<void> {
  const offer = useMachines.getState().connectOffer;
  useMachines.setState({ connectOffer: null, connectError: null });
  if (!offer || offer.pairedWith || offer.usedUp || offer.expiresAt <= Date.now()) return;
  try {
    await request(RemoteMethod.connectCode, { cancel: true }, 15_000);
  } catch (error) {
    toast(errorText(error), "error");
  }
}

/**
 * Follow the open connect code: the gateway says when a computer paired with it, or its tries ran
 * out. The page asks while the code is on screen.
 */
export async function followConnectCode(): Promise<void> {
  const offer = useMachines.getState().connectOffer;
  if (!offer || offer.pairedWith || offer.usedUp) return;
  try {
    const status = obj(await request<JsonObject>(RemoteMethod.status, {}, 10_000));
    const ended = obj(obj(obj(status?.gateway)?.machineOffer)?.lastEnded);
    if (!ended || num(ended.serial) !== offer.serial) return;
    const current = useMachines.getState().connectOffer;
    if (current?.serial !== offer.serial) return;
    useMachines.setState({
      connectOffer:
        str(ended.end) === "paired"
          ? { ...current, pairedWith: str(ended.deviceName) ?? "A computer" }
          : { ...current, usedUp: true },
    });
  } catch {
    // The next tick asks again; the countdown says when it no longer matters.
  }
}

/**
 * Pair with a connect code created on another computer. `address` is where to reach it when the
 * code carries none.
 */
export async function connectWithCode(code: string, address?: string): Promise<boolean> {
  const typed = code.trim();
  if (!typed) {
    useMachines.setState({ connectError: "Paste or type the connect code first." });
    return false;
  }
  useMachines.setState({ connectBusy: true, connectError: null });
  try {
    const machine = await shellCallWithin<Machine>(PAIR_DEADLINE_MS, "machines_connect_code", {
      code: typed,
      address: address?.trim() || null,
      name: null,
    });
    useMachines.setState((state) => ({
      machines: [...state.machines.filter((saved) => saved.id !== machine.id), machine],
    }));
    toast(`Connected to ${machine.name}.`, "success");
    await refreshMachines();
    return true;
  } catch (error) {
    useMachines.setState({ connectError: errorText(error) });
    return false;
  } finally {
    useMachines.setState({ connectBusy: false });
  }
}

/** One step of a connection over SSH (`rustcode_client::ssh::Progress`). */
export interface SshStep {
  step: "sign_in" | "check" | "install" | "start" | "pair";
  state: "running" | "done" | "skipped" | "failed";
  label: string;
  detail?: string | null;
}

export interface SshRun {
  running: boolean;
  steps: SshStep[];
}

/**
 * How long the page waits for a connection over SSH: an install downloads a release on the other
 * computer, which the native side bounds at ten minutes, plus sign-in, start and pairing.
 */
export const SSH_DEADLINE_MS = 15 * 60_000;

/**
 * Connect over SSH: install RustCode on `target` at this version when allowed and needed, start
 * its server and pair with it. The steps are polled while it runs.
 */
export async function connectOverSsh(target: string, allowInstall: boolean): Promise<boolean> {
  const typed = target.trim();
  if (!typed) {
    useMachines.setState({ sshError: "Type the host first, like lars@box or an alias from ~/.ssh/config." });
    return false;
  }
  useMachines.setState({ ssh: { running: true, steps: [] }, sshError: null });
  const poll = setInterval(() => {
    void shellCall<SshRun>("machines_ssh_progress").then(
      (run) => {
        if (useMachines.getState().ssh?.running) useMachines.setState({ ssh: { ...run, running: true } });
      },
      () => {},
    );
  }, 400);
  try {
    const machine = await shellCallWithin<Machine>(SSH_DEADLINE_MS, "machines_connect_ssh", {
      target: typed,
      port: null,
      identity: null,
      allowInstall,
      name: null,
    });
    useMachines.setState((state) => ({
      machines: [...state.machines.filter((saved) => saved.id !== machine.id), machine],
    }));
    toast(`Connected to ${machine.name}.`, "success");
    await refreshMachines();
    return true;
  } catch (error) {
    useMachines.setState({ sshError: errorText(error) });
    return false;
  } finally {
    clearInterval(poll);
    const last = await shellCall<SshRun>("machines_ssh_progress").catch(() => null);
    useMachines.setState((state) => ({
      ssh: { running: false, steps: last?.steps ?? state.ssh?.steps ?? [] },
    }));
  }
}

/** Clear the steps of a finished connection over SSH. */
export function dismissSsh(): void {
  useMachines.setState({ ssh: null, sshError: null });
}

/** Forget a machine, after asking: its token and its routes go with it. */
export function removeMachine(machine: Machine): void {
  askConfirm({
    title: `Remove ${machine.name}?`,
    message:
      `${machine.name} is forgotten on this computer: its pairing token and routes are deleted, and its threads are no longer listed here. ` +
      "It keeps listing this computer until you revoke it there.",
    action: "Remove",
    destructive: true,
    run: () => {
      void shellCall("machines_remove", { id: machine.id }).then(
        async () => {
          detach(machine.id);
          toast(`Removed ${machine.name}.`, "success");
          await refreshMachines();
        },
        (error) => toast(errorText(error), "error"),
      );
    },
  });
}

/** A probe in words: `Reachable · 4 ms`, `Refused · 2 ms`, `Timed out`, `Not tested`. */
export function probeLabel(probe: RouteProbe | null | undefined): string {
  if (!probe) return "Not tested";
  const ms = probe.latencyMs === null ? "" : ` · ${probe.latencyMs} ms`;
  switch (probe.outcome) {
    case "reachable":
      return `Reachable${ms}`;
    case "refused":
      return `Refused${ms}`;
    case "timeout":
      return "Timed out";
    default:
      return "Unreachable";
  }
}

/** The chip tone of a probe: green when it answered, red when it did not, none before a test. */
export function probeTone(probe: RouteProbe | null | undefined): "ok" | "bad" | "idle" {
  if (!probe) return "idle";
  return probe.outcome === "reachable" ? "ok" : "bad";
}

/**
 * Test every route of one machine (every machine when none is named) with a
 * plain connect, and show what each found. Answers whether the test ran.
 */
export async function probeMachineRoutes(machine?: Pick<Machine, "id">): Promise<boolean> {
  useMachines.setState({ probing: true });
  try {
    const machines = await shellCallWithin<Machine[]>(
      PROBE_MS,
      "machines_probe",
      machine ? { id: machine.id } : {},
    );
    useMachines.setState({ machines, probing: false });
    return true;
  } catch (error) {
    useMachines.setState({ probing: false });
    toast(`Could not test the routes: ${errorText(error)}`, "error");
    return false;
  }
}

/** Pause new threads on a machine, or let them start there again. */
export async function setMachinePaused(machine: Pick<Machine, "id">, paused: boolean): Promise<void> {
  // The row answers at once, and the shell's own read settles the value.
  useMachines.setState((state) => ({
    machines: state.machines.map((entry) => (entry.id === machine.id ? { ...entry, paused } : entry)),
  }));
  try {
    await shellCallWithin(SHELL_READ_MS, "machines_set_paused", { id: machine.id, paused });
    await refreshMachines();
  } catch (error) {
    await refreshMachines();
    toast(errorText(error), "error");
  }
}

/** Open or close the Add environment dialog, which lives on Settings › Environments. */
export function setAddEnvironmentOpen(open: boolean): void {
  useMachines.setState({ addOpen: open });
}

/** Clear what the last attempt to connect with a code said, so the dialog starts clean. */
export function clearConnectError(): void {
  useMachines.setState({ connectError: null });
}

/** Close a machine's connection without forgetting it; Run on can still use it another time. */
export async function disconnectMachine(machine: Pick<Machine, "id">): Promise<void> {
  try {
    await shellCallWithin(SHELL_READ_MS, "machine_disconnect", { id: machine.id });
    await refreshMachines();
  } catch (error) {
    toast(errorText(error), "error");
  }
}

/**
 * Ask a machine to try again now. The shell stops its supervisor and starts a new
 * one, so a backoff that had grown to minutes is a fresh attempt; the app keeps
 * its channel and reopens it as the machine answers. A machine the shell is not
 * keeping (it was saved after the last read) is picked up by reading the list.
 */
export function reconnectMachine(machine: Pick<Machine, "id">): void {
  const link = links.get(machine.id);
  if (!link) {
    void refreshMachines();
    return;
  }
  // The row says it is trying while it asks, rather than staying offline until an answer.
  useMachines.setState((state) => ({
    machines: state.machines.map((entry) =>
      entry.id === machine.id ? { ...entry, state: "connecting", detail: null, retryInSecs: null } : entry,
    ),
  }));
  link.connection?.close("Reconnecting.");
  link.connection = null;
  link.ready = false;
  void link.transport
    .restart()
    .then(() => refreshMachines())
    .catch((error) => {
      useMachines.setState((state) => ({
        machines: state.machines.map((entry) =>
          entry.id === machine.id ? { ...entry, state: "offline", detail: errorText(error) } : entry,
        ),
      }));
    });
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
