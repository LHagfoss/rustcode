// What the Machines page, the Add machine dialog and the new-thread selector say about a
// saved machine, in one place (docs/desktop.md › Several machines).
//
// Pure functions over `state/machines.ts` values, so the words are testable without a page.

import type { FleetEntry } from "../state/fleet";
import {
  DEV_APP_VERSION,
  lastSeen,
  type Machine,
  machineReconnecting,
  platformName,
  reasonLabel,
  routeLabel,
  type SshStep,
} from "../state/machines";
import { compareVersions } from "../state/updates";

/** What a row's dot and chip say at a glance: the four states people tell apart. */
export type MachineStatusKind = "online" | "connecting" | "offline" | "attention";

export interface MachineStatus {
  kind: MachineStatusKind;
  label: string;
  tone: "ok" | "warn" | "bad" | "idle";
}

/** Online, connecting, offline, or a state the user has to act on. */
export function machineStatus(machine: Pick<Machine, "state" | "retryInSecs" | "reason">): MachineStatus {
  switch (machine.state) {
    case "connected":
      return { kind: "online", label: "Online", tone: "ok" };
    case "connecting":
      return { kind: "connecting", label: "Connecting", tone: "idle" };
    case "offline":
      return machineReconnecting(machine)
        ? { kind: "connecting", label: "Reconnecting", tone: "idle" }
        : { kind: "offline", label: "Offline", tone: "warn" };
    case "unavailable":
      return { kind: "attention", label: reasonLabel(machine.reason), tone: "warn" };
    case "needsPairing":
      return { kind: "attention", label: "Needs pairing", tone: "bad" };
    case "stopped":
      return { kind: "offline", label: "Not connected", tone: "idle" };
  }
}

/**
 * Whether a machine runs an older RustCode than this app: the row says so and how to update
 * it. A development build (`DEV_APP_VERSION`) is older than everything, so it never says it, and
 * a machine that has not reported a version is not called outdated.
 */
export function machineOutdated(
  remote: string | null | undefined,
  local: string | null | undefined,
): boolean {
  if (!remote || !local || local === DEV_APP_VERSION) return false;
  return compareVersions(remote, local) < 0;
}

/**
 * A computer this app is not paired with, known through one that is: the facts its row shows
 * (platform, version, when it was last seen) and who told us. Read-only: knowing a computer is
 * not access to it, and it is paired on its own.
 */
export interface KnownMachine {
  id: string;
  name: string;
  platform: string;
  facts: string[];
  /** The name of the computer that told us, when there is one. */
  via: string | null;
  outdated: boolean;
}

export function knownMachine(
  entry: FleetEntry,
  via: string | null,
  local: string | null,
  now: number,
): KnownMachine {
  const facts = [
    platformName(entry.platform),
    entry.version ? `RustCode ${entry.version}` : undefined,
    entry.lastSeenAt ? `Last seen ${lastSeen(entry.lastSeenAt, now)}` : undefined,
  ].filter((fact): fact is string => Boolean(fact));
  return {
    id: entry.hostId,
    name: entry.name,
    platform: entry.platform,
    facts,
    via,
    outdated: machineOutdated(entry.version, local),
  };
}

/** The address a machine is reached on (or tried first), or null before one is saved. */
export function machineAddress(machine: Pick<Machine, "route" | "routes">): string | null {
  return machine.route?.address ?? machine.routes[0]?.address ?? null;
}

/** `192.168.1.31:17879 · LAN · Unencrypted`: where it is, and how it travels. */
export function machineRoute(machine: Pick<Machine, "route" | "routes">): string {
  const route = machine.route ?? machine.routes[0] ?? null;
  if (!route) return "No route saved";
  return `${route.address} · ${routeLabel(route)}`;
}

/** The one action a row offers to bring the machine back, by what is wrong with it. */
export type MachineRecovery = "connect" | "reconnect" | "pair" | null;

export function machineRecovery(machine: Pick<Machine, "state" | "paused">): MachineRecovery {
  switch (machine.state) {
    case "connected":
      return null;
    case "stopped":
      return "connect";
    case "needsPairing":
      return "pair";
    case "connecting":
      return null;
    default:
      return "reconnect";
  }
}

/** The words of the recovery button. */
export function recoveryLabel(recovery: Exclude<MachineRecovery, null>): string {
  switch (recovery) {
    case "connect":
      return "Connect";
    case "reconnect":
      return "Reconnect";
    case "pair":
      return "Pair again";
  }
}

// ---------------------------------------------------------------- the new-thread selector

/** One row of the selector a new thread starts from. */
export interface MachineChoice {
  /** The machine id, or `""` for this computer (`LOCAL_HOST`). */
  id: string;
  name: string;
  status: MachineStatus;
  /** Whether a thread can start there now. */
  available: boolean;
  /** The row's second line: its address, or why it cannot be chosen. */
  detail: string;
}

/**
 * The machines a new thread can run on, this computer first. One that is away or paused is
 * listed, so the user sees why it is missing, but cannot be chosen (`setNewThreadHost` refuses it).
 */
export function machineChoices(
  machines: readonly Machine[],
  local: { name: string },
  nameOf: (machine: Machine) => string,
): MachineChoice[] {
  const here: MachineChoice = {
    id: "",
    name: local.name,
    status: { kind: "online", label: "Online", tone: "ok" },
    available: true,
    detail: "This computer",
  };
  const others = machines.map((machine): MachineChoice => {
    const status = machineStatus(machine);
    const paused = machine.paused === true;
    const available = machine.state === "connected" && !paused;
    return {
      id: machine.id,
      name: nameOf(machine),
      status,
      available,
      detail: paused ? "Paused for new threads" : available ? machineRoute(machine) : status.label,
    };
  });
  return [here, ...others];
}

// ---------------------------------------------------------------- the Add machine dialog

export type AddMethod = "code" | "ssh" | "address";

export const ADD_METHODS: { value: AddMethod; label: string }[] = [
  { value: "code", label: "Connect code" },
  { value: "ssh", label: "SSH" },
  { value: "address", label: "Address" },
];

export interface ProgressStep {
  key: string;
  label: string;
  state: "todo" | "running" | "done" | "failed" | "skipped";
  detail?: string | null;
}

/** The steps of a connect code, as far as the app can tell: one exchange, in two named halves. */
export function codeSteps(busy: boolean, failed: boolean): ProgressStep[] {
  return [
    { key: "check", label: "Check the code", state: failed ? "failed" : busy ? "running" : "todo" },
    { key: "save", label: "Connect and save the machine", state: "todo" },
  ];
}

/** The steps of asking an address to connect: ask, accept there, type its code, pair. */
export function addressSteps(
  stage: "waiting" | "code" | "pairing" | null,
  failed: boolean,
  host?: string,
): ProgressStep[] {
  const there = host ?? "that computer";
  const order = ["waiting", "code", "pairing"] as const;
  const names = [
    `Ask ${there} to connect`,
    `Accept the request on ${there}`,
    `Type the code ${there} shows`,
    "Pair and save the machine",
  ];
  // The step in progress: asking has been sent once a stage exists, so "waiting" is on step two.
  const active = stage ? order.indexOf(stage) + 1 : failed ? 0 : -1;
  return names.map((label, index) => {
    const state: ProgressStep["state"] =
      active < 0 || index > active ? "todo" : index < active ? "done" : failed ? "failed" : "running";
    return { key: String(index), label, state };
  });
}

const SSH_ORDER: SshStep["step"][] = ["sign_in", "check", "install", "start", "pair", "pair_back"];
const SSH_LABEL: Record<SshStep["step"], string> = {
  sign_in: "Sign in over SSH",
  check: "Check RustCode there",
  install: "Install RustCode",
  start: "Start its server",
  pair: "Pair",
  pair_back: "Let it reach this computer",
};

/** The SSH run's steps in the order they run, with the ones not reached yet listed as to do. */
export function sshProgress(steps: readonly SshStep[], running: boolean, mutual = false): ProgressStep[] {
  const failed = steps.some((step) => step.state === "failed");
  return SSH_ORDER.flatMap((name): ProgressStep[] => {
    const step = steps.find((seen) => seen.step === name);
    if (!step && !running) return [];
    // Pairing back is a step of mutual access only: one way never lists it.
    if (!step && name === "pair_back" && !mutual) return [];
    const state = step?.state ?? "todo";
    if (failed && state === "todo") return [];
    return [
      {
        key: name,
        label: step?.label ?? SSH_LABEL[name],
        state,
        detail: state === "failed" ? null : step?.detail,
      },
    ];
  });
}

/** A failed step in plain words: what did not work, and the next thing to try. */
export interface AddFailure {
  title: string;
  detail: string;
  hint: string;
}

/** Make a raw error into the card the dialog shows, with the method's own advice. */
export function addFailure(method: AddMethod, raw: string): AddFailure {
  const detail = raw.trim().replace(/\s+/g, " ");
  switch (method) {
    case "code":
      return {
        title: "Could not connect with that code",
        detail,
        hint: "A code works once and for ten minutes. Create a new one on the other computer if it ran out, and check that both computers can reach each other.",
      };
    case "ssh":
      return {
        title: "Could not connect over SSH",
        detail,
        hint: "Check that `ssh` reaches the host from a terminal without a password prompt: a key with a passphrase must be in the agent (ssh-add).",
      };
    case "address":
      return {
        title: "Could not reach that address",
        detail,
        hint: "Check the address and that RustCode is running there with remote access on. Away from home, use its Tailscale or VPN address.",
      };
  }
}

/** The ids of the saved machines, for telling a new one from the ones already there. */
export function machineIds(machines: readonly Pick<Machine, "id">[]): Set<string> {
  return new Set(machines.map((machine) => machine.id));
}

/** The machine that appeared since `before`, when exactly one did. */
export function newMachine(machines: readonly Machine[], before: ReadonlySet<string>): Machine | null {
  const added = machines.filter((machine) => !before.has(machine.id));
  return added.length === 1 ? added[0]! : null;
}
