import { RpcError } from "../acp/jsonrpc";
import type { HubStatus } from "./hub";
import { fetchLoopsOn, type Loop, type LoopHubMember, type LoopRun, type LoopsSnapshot } from "./loops";
import { type Machine, machineName } from "./machines";
import { errorText } from "./store";
import { LOCAL_HOST, threadKey } from "./threadKeys";

/** What one machine answered for `_rustcode/loops/list`. */
export interface HostLoopsState {
  host: string;
  daemonRunning: boolean;
  loops: Loop[];
  hubMembers?: LoopHubMember[];
  /** True when the remote machine runs an older RustCode without remote `_rustcode/loops/*`. */
  unsupported?: boolean;
  /** Non-fatal read error on a remote host. */
  error?: string;
}

/** One machine a job can target in the New / Edit job dialog. */
export interface JobMachineTarget {
  /** Unique `<Select>` value: `host:<id>` for a direct scheduler, or `hub:<hubHost>:<memberName>`. */
  key: string;
  /** The scheduler host that owns the job (`""` for this computer, or a paired machine id). */
  host: string;
  /** Set only when the scheduler at `host` forwards runs over its hub link to this member. */
  member?: string;
  /** Option label shown in the picker. */
  label: string;
  /** Human name of the computer where the job runs. */
  machineName: string;
  /** Human name of the hub keeping the schedule when `member` is set. */
  viaHubName?: string;
  /** Whether the target machine is currently online. */
  online: boolean;
  /** Disabled when offline and not reachable through any hub mailbox. */
  disabled?: boolean;
}

/** Unique key for one job across the network. */
export function loopKey(loop: Pick<Loop, "host" | "id">): string {
  return loop.host ? `${loop.host}/${loop.id}` : loop.id;
}

/** Picker value for a `(host, member)` target. */
export function targetKeyOf(host: string, member?: string): string {
  const trimmed = member?.trim();
  return trimmed ? `hub:${host}:${trimmed}` : `host:${host}`;
}

/** Whether an RPC error means a remote engine predates remote `_rustcode/loops/*`. */
export function isLoopsUnsupported(error: unknown): boolean {
  return error instanceof RpcError && error.isMethodNotFound;
}

/** Human-friendly error when a remote machine refuses a loops call because it needs an update. */
export function remoteLoopError(error: unknown, name: string): string {
  if (isLoopsUnsupported(error)) {
    return `Update RustCode on ${name} to manage its jobs from here.`;
  }
  return errorText(error);
}

/** Where a job runs, as a short machine name for cards and review rows. */
export function loopMachineLabel(
  loop: Pick<Loop, "host" | "machine">,
  machines: readonly Pick<Machine, "id" | "name" | "displayName">[],
  localName: string,
): string {
  if (loop.machine) return loop.machine;
  if (!loop.host) return localName;
  const saved = machines.find((machine) => machine.id === loop.host);
  return saved ? machineName(saved) : loop.host;
}

/**
 * The app thread key (`host/id` or local `id`) for a run's thread, or undefined when the thread
 * lives on a hub member this app is not paired with.
 */
export function runThreadKey(
  loop: Pick<Loop, "host">,
  run: Pick<LoopRun, "machine" | "threadId"> | undefined,
  machines: readonly Pick<Machine, "id" | "name" | "displayName">[] = [],
  localName?: string,
): string | undefined {
  if (!run?.threadId) return undefined;
  if (!run.machine) return threadKey(loop.host, run.threadId);
  const wanted = run.machine.trim().toLowerCase();
  if (!wanted) return threadKey(loop.host, run.threadId);
  if (localName && wanted === localName.trim().toLowerCase()) return run.threadId;
  const matched = machines.filter(
    (machine) =>
      machine.id.toLowerCase() === wanted ||
      machine.name.toLowerCase() === wanted ||
      machineName(machine).toLowerCase() === wanted,
  );
  if (matched.length === 1) return threadKey(matched[0]!.id, run.threadId);
  return undefined;
}

interface HubRoster {
  hubHost: string;
  hubName: string;
  members: readonly LoopHubMember[];
}

function findHubMember(
  rosters: readonly HubRoster[],
  machine: Pick<Machine, "id" | "name" | "displayName">,
): { hubHost: string; hubName: string; member: LoopHubMember } | null {
  const names = new Set([machine.name.toLowerCase(), machineName(machine).toLowerCase()]);
  for (const roster of rosters) {
    const member = roster.members.find(
      (entry) => entry.hostId === machine.id || names.has(entry.name.toLowerCase()),
    );
    if (member) return { hubHost: roster.hubHost, hubName: roster.hubName, member };
  }
  return null;
}

/**
 * Every machine a job can run on: this computer, every paired machine (directly over its own
 * scheduler when online, or queued via a hub when offline/older), and any hub member not paired
 * directly here.
 */
export function jobMachineTargets({
  localName,
  localHostId,
  machines,
  localHub,
  remoteHubMembers = {},
  unsupportedHosts = new Set<string>(),
  current,
}: {
  localName: string;
  localHostId?: string | null;
  machines: readonly Pick<Machine, "id" | "name" | "displayName" | "state">[];
  localHub?: HubStatus | null;
  remoteHubMembers?: Readonly<Record<string, readonly LoopHubMember[]>>;
  unsupportedHosts?: ReadonlySet<string>;
  /** Existing job target when editing, kept selectable even if now offline or unlinked. */
  current?: { host: string; member?: string };
}): JobMachineTarget[] {
  const rosters: HubRoster[] = [];
  if (localHub?.hub && localHub.members?.length) {
    rosters.push({
      hubHost: LOCAL_HOST,
      hubName: localName,
      members: localHub.members.map((m) => ({ hostId: m.hostId, name: m.name, online: m.online })),
    });
  }
  for (const machine of machines) {
    if (machine.state !== "connected") continue;
    const members = remoteHubMembers[machine.id];
    if (members?.length) {
      rosters.push({ hubHost: machine.id, hubName: machineName(machine), members });
    }
  }

  const hasPeers = machines.length > 0 || rosters.some((roster) => roster.members.length > 0);
  const targets: JobMachineTarget[] = [
    {
      key: targetKeyOf(LOCAL_HOST),
      host: LOCAL_HOST,
      label: hasPeers ? `${localName} (this computer)` : "This computer",
      machineName: localName,
      online: true,
    },
  ];

  const coveredIds = new Set<string>();
  const coveredNames = new Set<string>([localName.toLowerCase()]);
  if (localHostId) coveredIds.add(localHostId);

  for (const machine of machines) {
    const name = machineName(machine);
    coveredIds.add(machine.id);
    coveredNames.add(machine.name.toLowerCase());
    coveredNames.add(name.toLowerCase());

    const online = machine.state === "connected";
    const unsupported = unsupportedHosts.has(machine.id);
    if (online && !unsupported) {
      targets.push({
        key: targetKeyOf(machine.id),
        host: machine.id,
        label: name,
        machineName: name,
        online: true,
      });
      continue;
    }

    const viaHub = findHubMember(rosters, machine);
    if (viaHub) {
      const qualifier = online ? `via ${viaHub.hubName}` : `offline · via ${viaHub.hubName}`;
      targets.push({
        key: targetKeyOf(viaHub.hubHost, viaHub.member.name),
        host: viaHub.hubHost,
        member: viaHub.member.name,
        label: `${name} (${qualifier})`,
        machineName: name,
        viaHubName: viaHub.hubName,
        online: viaHub.member.online,
      });
      continue;
    }

    targets.push({
      key: targetKeyOf(machine.id),
      host: machine.id,
      label: unsupported ? `${name} (update required)` : `${name} (offline)`,
      machineName: name,
      online,
      disabled: !online,
    });
  }

  for (const roster of rosters) {
    for (const member of roster.members) {
      if (coveredIds.has(member.hostId) || coveredNames.has(member.name.toLowerCase())) continue;
      coveredIds.add(member.hostId);
      coveredNames.add(member.name.toLowerCase());
      const detail =
        roster.hubHost === LOCAL_HOST
          ? member.online
            ? ""
            : " (offline)"
          : member.online
            ? ` (via ${roster.hubName})`
            : ` (via ${roster.hubName}, offline)`;
      targets.push({
        key: targetKeyOf(roster.hubHost, member.name),
        host: roster.hubHost,
        member: member.name,
        label: `${member.name}${detail}`,
        machineName: member.name,
        viaHubName: roster.hubName,
        online: member.online,
      });
    }
  }

  if (current) {
    const key = targetKeyOf(current.host, current.member);
    if (!targets.some((target) => target.key === key)) {
      const hostMachine = machines.find((machine) => machine.id === current.host);
      const hubName = current.host ? (hostMachine ? machineName(hostMachine) : current.host) : localName;
      targets.push({
        key,
        host: current.host,
        member: current.member,
        label: current.member ? `${current.member} (not a member)` : `${hubName} (offline)`,
        machineName: current.member || hubName,
        viaHubName: current.member ? hubName : undefined,
        online: false,
      });
    }
  }

  return targets;
}

/**
 * Fetch `_rustcode/loops/list` from this computer and every connected paired machine in parallel.
 * An older or disconnected remote machine never blocks the rest of the network.
 */
export async function fetchFleetLoops(remoteHosts: readonly string[]): Promise<{
  byHost: Record<string, HostLoopsState>;
  localError: string | null;
}> {
  const byHost: Record<string, HostLoopsState> = {};
  let localError: string | null = null;

  const hosts = [LOCAL_HOST, ...remoteHosts.filter(Boolean)];
  await Promise.all(
    hosts.map(async (host) => {
      try {
        const snapshot: LoopsSnapshot = await fetchLoopsOn(host);
        byHost[host] = {
          host,
          daemonRunning: snapshot.daemonRunning,
          loops: snapshot.loops,
          hubMembers: snapshot.hubMembers,
        };
      } catch (error) {
        if (host === LOCAL_HOST) {
          localError = errorText(error);
        } else if (isLoopsUnsupported(error)) {
          byHost[host] = { host, daemonRunning: false, loops: [], unsupported: true };
        } else {
          byHost[host] = { host, daemonRunning: false, loops: [], error: errorText(error) };
        }
      }
    }),
  );

  return { byHost, localError };
}
