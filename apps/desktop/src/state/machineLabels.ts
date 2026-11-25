// How a machine, its routes, its version and its codes read in the app: names, icons, tints and status lines. Reads the machines store only when called.
import { obj } from "../acp/jsonrpc";
import { currentPlatform } from "../lib/platform";
import { hasShell, shellCall, shellCallWithin } from "../lib/shell";
import { readStorage, writeStorage } from "../lib/storage";
import { appVersion } from "../lib/updater";
import {
  type DiscoveredMachine,
  type Machine,
  type MachineReason,
  type MachineRoute,
  PAIR_DEADLINE_MS,
  type Pairing,
  refreshMachines,
  SHELL_READ_MS,
  useMachines,
} from "./machines";
import { errorText, toast } from "./store";
import { compareVersions } from "./updates";

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
  /**
   * When `displayName` was chosen, here or on another computer, as an ISO time. The machine
   * sync (`state/fleet.ts`) keeps the newest rename: a name without a time stays on this computer.
   */
  nameUpdatedAt?: string | null;
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

export function readLocalIdentity(): MachineIdentity {
  try {
    const raw = JSON.parse(readStorage(LOCAL_IDENTITY_KEY) ?? "null") as unknown;
    const root = obj(raw);
    return {
      displayName: cleanMachineName(typeof root?.displayName === "string" ? root.displayName : null),
      icon: machineIconOf(typeof root?.icon === "string" ? root.icon : null),
      tint: machineTintOf(typeof root?.tint === "string" ? root.tint : null),
      nameUpdatedAt: typeof root?.nameUpdatedAt === "string" ? root.nameUpdatedAt : null,
    };
  } catch {
    return { displayName: null, icon: null, tint: null, nameUpdatedAt: null };
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

/** How far ahead of this computer's clock a rename time may be, as the engine allows. */
export const MAX_CLOCK_SKEW_MS = 5 * 60_000;

/** `stamp`, no later than [`MAX_CLOCK_SKEW_MS`] after `now`: a clock that is ahead cannot pin a name. */
export function boundedStamp(stamp: string, now = Date.now()): string {
  const time = Date.parse(stamp);
  return Number.isFinite(time) && time > now + MAX_CLOCK_SKEW_MS
    ? new Date(now + MAX_CLOCK_SKEW_MS).toISOString()
    : stamp;
}

/** The stamp of a rename made now: after the one it replaces, so it always wins. */
export function renameStamp(current: string | null | undefined, now = Date.now()): string {
  const after = current ? Date.parse(current) + 1 : Number.NaN;
  return new Date(Number.isFinite(after) ? Math.max(now, after) : now).toISOString();
}

/**
 * Rename this computer, or give it a glyph. A rename is stamped, so the machine sync
 * (`state/fleet.ts`) tells every paired computer and the newest rename wins; a name another
 * computer chose arrives with its own `nameUpdatedAt` and is not stamped again.
 */
export function setLocalIdentity(patch: Partial<MachineIdentity>): void {
  const current = localIdentity();
  const renamed = patch.displayName !== undefined;
  const next: MachineIdentity = {
    displayName: patch.displayName === undefined ? current.displayName : cleanMachineName(patch.displayName),
    icon: patch.icon === undefined ? current.icon : machineIconOf(patch.icon),
    tint: patch.tint === undefined ? current.tint : machineTintOf(patch.tint),
    nameUpdatedAt: renamed
      ? patch.nameUpdatedAt
        ? boundedStamp(patch.nameUpdatedAt)
        : renameStamp(current.nameUpdatedAt)
      : (current.nameUpdatedAt ?? null),
  };
  // A full or disabled store keeps the choice for this run only.
  writeStorage(LOCAL_IDENTITY_KEY, JSON.stringify(next));
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

/** Rename a paired machine. The newest name wins across every paired computer (`state/fleet.ts`). */
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
 * "reconnecting", not a failure the user has to act on (issue 3487). An
 * attempt in flight counts too: after a wake every attempt runs for seconds,
 * and the chat said "offline since" all that time while the row said
 * Connecting.
 */
export function machineReconnecting(
  machine: Pick<Machine, "state" | "retryInSecs"> | null | undefined,
): boolean {
  return machine?.state === "connecting" || (machine?.state === "offline" && !!machine.retryInSecs);
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
  ssh: "SSH",
};

/** A LAN route on this computer's own loopback: a port forwarded to the machine by something else. */
export function isLocalForward(route: Pick<MachineRoute, "kind" | "address">): boolean {
  if (route.kind !== "lan") return false;
  const host = route.address.replace(/^\[/, "").replace(/\]?(:\d+)?$/, "");
  return host === "localhost" || host === "::1" || /^127\./.test(host);
}

/**
 * The network a route travels over, as people know it: `LAN`, `NetBird`,
 * `Tailscale`, `ZeroTier`, `VPN`, `Public`. The product the machine named wins
 * over the generic kind: `100.64.0.0/10` is shared by several overlays (PR 3402).
 */
export function routeName(route: MachineRoute | null | undefined, kind?: string): string {
  const wire = route?.provider || route?.kind || kind;
  if (!wire) return "No route saved";
  if (route && isLocalForward(route)) return "Forwarded port";
  return ROUTE_NAME[wire] ?? ROUTE_NAME[kind ?? ""] ?? wire;
}

/**
 * One route as people read it: `LAN · Unencrypted`, `NetBird · Encrypted`.
 * Only a tunnel encrypts (the shell applies the gateway's own
 * rule): a LAN or a public address never does.
 */
export function routeLabel(route: MachineRoute | null | undefined): string {
  if (!route) return "No route saved";
  // Whatever forwards the port decides; this computer cannot tell.
  if (isLocalForward(route)) return "Forwarded port";
  return `${routeName(route)} · ${route.encrypted ? "Encrypted" : "Unencrypted"}`;
}

/** One change of route, as a toast says it: `Studio: Now on LAN · Unencrypted`. */
export interface RouteChangeNotice {
  machineId: string;
  text: string;
  /** A move off a tunnel is an error tone; any other change is plain information. */
  tone: "info" | "error";
}

/**
 * The route changes `machines` report that `seen` has not told yet. A machine's first listing in a
 * window only records where it stands: a change from before the window opened is not news. `seen`
 * is updated, so the same change is told once however often the list is read.
 */
export function routeChangeNotices(
  machines: readonly Machine[],
  seen: Map<string, number>,
): RouteChangeNotice[] {
  const notices: RouteChangeNotice[] = [];
  for (const machine of machines) {
    const change = machine.routeChange;
    const told = seen.get(machine.id);
    seen.set(machine.id, change?.seq ?? 0);
    // A machine listed for the first time only records where it stands.
    if (!change || told === undefined || change.seq <= told) continue;
    notices.push({
      machineId: machine.id,
      text: `${machineName(machine)}: Now on ${routeLabel(change.to)}`,
      tone: change.downgrade ? "error" : "info",
    });
  }
  return notices;
}

/** The changes already told in this window, by machine. */
export const toldRouteChanges = new Map<string, number>();

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
      // A detail while connecting is what the last attempt found and the
      // owner can fix (a forwarded port's tunnel that is down).
      return machine.detail ?? `Connecting over ${routeLabel(machine.route)}`;
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
      return "run `rc --update`, or install the new AppImage";
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
