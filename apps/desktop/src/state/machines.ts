// The other computers this app is paired with (Settings › Machines). One
// saved machine is one entry, reached over an ordered list of routes; the
// app's Rust shell runs a supervisor per machine and relays its ACP here, and
// this module holds what the page shows: the machines, where each connection
// stands, and one ACP client per machine (the same class the local engine
// uses), so a connected machine's `session/list` gives its thread count.
//
// Step 1.9 of the machine network plan merges those threads into the sidebar;
// this step only lists the machines and their counts.
//
// A machine is a computer, a route is one way to reach it, and the server is
// what runs on it (docs/terminology.md).

import { create } from "zustand";
import { AcpConnection, arr, type Json, obj } from "../acp/jsonrpc";
import { isTauri, MachineTransport } from "../acp/transport";
import { hasShell, shellCall } from "../lib/shell";
import { askConfirm, toast } from "./store";

/** Where a machine stands, as the shell reports it. */
export type MachineState = "connecting" | "connected" | "offline" | "needsPairing" | "stopped";

/** One way to reach a machine: a LAN address, Tailscale, a VPN, or public. */
export interface MachineRoute {
  kind: string;
  address: string;
  /** The network encrypts the traffic (Tailscale, a VPN); a LAN route does not. */
  encrypted: boolean;
}

/** A saved machine, as `machines_list` reports it. */
export interface Machine {
  id: string;
  name: string;
  /** `macos`, `windows`, `linux`, or `unknown`. */
  platform: string;
  /** The main machine is listed first and is the default **Run on**. */
  main: boolean;
  routes: MachineRoute[];
  /** The route in use, or the one it will try first. */
  route: MachineRoute | null;
  state: MachineState;
  detail: string | null;
  /** Seconds until the next attempt, while it waits. */
  retryInSecs: number | null;
  addedAt: string;
  lastConnectedAt: string | null;
}

/** A machine Bonjour found on this network, as `machines_discover` reports it. */
export interface DiscoveredMachine {
  name: string;
  address: string | null;
  addresses: string[];
  gatewayId: string | null;
  /** This app already has it saved. */
  saved: boolean;
}

/** An accepted connection request, waiting for the code the machine shows. */
export interface AskView {
  requestId: string;
  codeLength: number;
  expiresInSecs: number;
}

/**
 * Where the Add machine flow stands. The request id arrives as soon as the
 * request is on that machine, so Cancel can name it in both stages.
 */
export type Pairing =
  | { stage: "waiting"; address: string; host: string; requestId?: string }
  | { stage: "code"; address: string; host: string; requestId: string; codeLength: number; expiresAt: number }
  | { stage: "pairing"; address: string; host: string; requestId: string };

interface MachinesStore {
  /** The first `machines_list` came back (or failed). */
  ready: boolean;
  error: string | null;
  machines: Machine[];
  /** Each connected machine's thread count, from its own `session/list`. */
  counts: Record<string, number>;
  discovered: DiscoveredMachine[] | null;
  discovering: boolean;
  discoverError: string | null;
  pairing: Pairing | null;
  pairingError: string | null;
}

export const useMachines = create<MachinesStore>(() => ({
  ready: false,
  error: null,
  machines: [],
  counts: {},
  discovered: null,
  discovering: false,
  discoverError: null,
  pairing: null,
  pairingError: null,
}));

// ---------------------------------------------------------------- labels

/** The words a row's state chip uses. */
export function stateLabel(machine: Pick<Machine, "state" | "retryInSecs">): string {
  switch (machine.state) {
    case "connected":
      return "Connected";
    case "connecting":
      return "Connecting";
    case "needsPairing":
      return "Pairing expired";
    case "offline":
      return machine.retryInSecs ? `Offline · retrying in ${machine.retryInSecs} s` : "Offline";
    case "stopped":
      return "Not connected";
  }
}

/** The chip's tone for a state: green while connected, amber while it is not. */
export function stateTone(machine: Pick<Machine, "state">): "ok" | "warn" | "bad" | "idle" {
  switch (machine.state) {
    case "connected":
      return "ok";
    case "connecting":
      return "idle";
    case "offline":
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
  vpn: "VPN",
  public: "Public",
};

/**
 * One route as people read it: `LAN · Unencrypted (LAN)` for a local network,
 * `Tailscale · Encrypted` for an overlay. Only a LAN route is plain: everything
 * else travels through a network that encrypts it.
 */
export function routeLabel(route: MachineRoute | null | undefined): string {
  if (!route) return "No route saved";
  const name = ROUTE_NAME[route.kind] ?? route.kind;
  return `${name} · ${route.encrypted ? "Encrypted" : "Unencrypted (LAN)"}`;
}

/** What one machine row says under its name. */
export function machineLine(machine: Machine): string {
  switch (machine.state) {
    case "connected":
      return `Connected over ${routeLabel(machine.route)}`;
    case "connecting":
      return `Connecting over ${routeLabel(machine.route)}`;
    case "needsPairing":
      return machine.detail ?? "This machine no longer accepts this computer's pairing. Pair it again.";
    case "offline":
      return machine.detail ?? "Not connected. The app keeps trying.";
    case "stopped":
      return "Not connected.";
  }
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
}

const links = new Map<string, Link>();
const opening = new Set<string>();
let started = false;
/** Counts asks, so an answer that arrives after Cancel is ignored. */
let asks = 0;

/**
 * Read the saved machines once, then let each one's supervisor reach the app.
 * Called when the app starts; a machine saved later is picked up by
 * `refreshMachines`.
 */
export async function startMachines(): Promise<void> {
  if (started || !hasShell()) return;
  started = true;
  await refreshMachines();
}

/** Read the machines and their live state from the shell. */
export async function refreshMachines(): Promise<void> {
  if (!hasShell()) return;
  try {
    const machines = await shellCall<Machine[]>("machines_list");
    useMachines.setState({ machines, ready: true, error: null });
    for (const machine of machines) void attach(machine);
    for (const id of [...links.keys()]) {
      if (!machines.some((machine) => machine.id === id)) detach(id);
    }
  } catch (error) {
    useMachines.setState({ ready: true, error: errorText(error) });
  }
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
  const link: Link = { transport, connection: null };
  try {
    await transport.start((event) => {
      if (event.type === "open") {
        const connection = new AcpConnection((line) => transport.send(line));
        link.connection = connection;
        void readThreads(machine.id, connection);
      } else if (event.type === "line") {
        link.connection?.receive(event.line);
      } else if (event.type === "lines") {
        for (const line of event.lines) link.connection?.receive(line);
      } else if (event.type === "closed") {
        link.connection?.close(event.reason);
        link.connection = null;
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

/** Stop a machine's channel: the shell stops its supervisor with it. */
function detach(id: string): void {
  const link = links.get(id);
  links.delete(id);
  if (!link) return;
  link.connection?.close("The machine was removed.");
  void link.transport.stop();
  useMachines.setState((state) => {
    const counts = { ...state.counts };
    delete counts[id];
    return { counts };
  });
}

/**
 * The machine's thread count, from its own `session/list`. A count is a
 * nicety: a machine that does not answer it is still listed as connected.
 */
async function readThreads(id: string, connection: AcpConnection): Promise<void> {
  try {
    await connection.request("initialize", { protocolVersion: 1 }, 10_000);
    const answer = (await connection.request("session/list", {}, 10_000)) as Json;
    const sessions = arr(obj(answer)?.sessions);
    useMachines.setState((state) => ({ counts: { ...state.counts, [id]: sessions.length } }));
  } catch {
    // An older machine, or one whose ACP is not allowed yet: no count.
  }
}

// ---------------------------------------------------------------- actions

/** Look for machines on this network (Bonjour, a few seconds). */
export async function discoverMachines(): Promise<void> {
  useMachines.setState({ discovering: true, discoverError: null });
  try {
    const discovered = await shellCall<DiscoveredMachine[]>("machines_discover");
    useMachines.setState({ discovered, discovering: false });
  } catch (error) {
    useMachines.setState({ discovering: false, discoverError: errorText(error) });
  }
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
  const host = name?.trim() || target;
  const asked = { address: target, host };
  const ask = ++asks;
  useMachines.setState({ pairing: { stage: "waiting", ...asked }, pairingError: null });
  const current = () => ask === asks;
  const waiting = (event: PairingEvent) => {
    if (!current()) return;
    if (event.type === "waiting")
      useMachines.setState({ pairing: { stage: "waiting", ...asked, requestId: event.requestId } });
    else if (event.type === "approved")
      useMachines.setState({
        pairing: {
          stage: "code",
          ...asked,
          requestId: event.requestId,
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
        address: target,
        name: name ?? null,
        onEvent: channel,
      });
    } else {
      answer = await shellCall<AskView>("machines_ask", { address: target, name: name ?? null });
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
  | { type: "waiting"; requestId: string; host: string }
  | { type: "approved"; requestId: string; codeLength: number; expiresInSecs: number }
  | { type: "refused"; message: string };

/** Give up on a request: the owner no longer sees it, and its socket is dropped. */
export function cancelPairing(): void {
  const pairing = useMachines.getState().pairing;
  asks += 1;
  useMachines.setState({ pairing: null, pairingError: null });
  if (pairing && "requestId" in pairing && pairing.requestId)
    void shellCall("machines_cancel", { requestId: pairing.requestId }).catch(() => undefined);
}

/** Send the code the machine shows. */
export async function pairMachine(code: string): Promise<void> {
  const pairing = useMachines.getState().pairing;
  if (pairing?.stage !== "code") return;
  asks += 1;
  useMachines.setState({ pairing: { ...pairing, stage: "pairing" }, pairingError: null });
  try {
    const machine = await shellCall<Machine>("machines_pair", {
      requestId: pairing.requestId,
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

/** Make this machine the main one: listed first, the default **Run on**. */
export async function setMainMachine(machine: Machine): Promise<void> {
  try {
    await shellCall("machines_set_main", { id: machine.id });
    await refreshMachines();
  } catch (error) {
    toast(errorText(error), "error");
  }
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
