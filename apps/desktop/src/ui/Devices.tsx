// Devices and one machine, in the sidebar (docs/desktop.md › Several machines).
//
// The list is one row per computer, this one first: the platform's glyph, the
// name, the state as a chip, then the route once, how many of its threads work
// now, and for a machine that is away, when it last answered. An offline row
// says why in words a reader can act on, with the one action that brings it
// back. Choosing a row replaces the project list with that machine's threads.
//
// The machine's own card stands above its threads: what it runs, the RustCode
// version it reports and whether that differs from this app's, and the three
// actions a machine has. What all of it says is decided in
// state/deviceRows.ts, so the words are tested without a window.

import { Settings as Gear, RefreshCw, SquarePen } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import type { SessionSummary } from "../acp/types";
import { appPlatform } from "../lib/platform";
import {
  deviceMeta,
  devicePlatform,
  deviceRoute,
  deviceStatus,
  localDeviceStatus,
  machineThreadCounts,
  offlineReason,
  threadCountLabel,
  versionLine,
} from "../state/deviceRows";
import {
  askMachine,
  lastSeen,
  localAppVersion,
  type Machine,
  machineGlyph,
  machineName,
  machineTint,
  platformIcon,
  reconnectMachine,
  selfName,
  useLocalIdentity,
  useMachines,
} from "../state/machines";
import { openSettings, startNewThread, useApp } from "../state/store";
import { LOCAL_HOST } from "../state/threadKeys";
import { DeviceGlyphIcon } from "./MachineGlyph";
import { Pill } from "./primitives";
import { Chip } from "./settingsKit";

/** The platform this app runs on, in the names a machine reports. */
export function localPlatform(): string {
  const platform = appPlatform();
  return platform === "mac" ? "macos" : platform;
}

/** The ids of the threads working right now, so a count includes a turn this app is watching. */
function useRunningIds(): ReadonlySet<string> {
  // A string rather than an array, so a streamed chunk that changes no flag re-renders nothing.
  const flags = useApp((s) =>
    Object.values(s.threads)
      .filter((thread) => thread.running)
      .map((thread) => thread.id)
      .join(" "),
  );
  return useMemo(() => new Set(flags ? flags.split(" ") : []), [flags]);
}

function StateChip({ tone, label }: { tone: "ok" | "warn" | "bad" | "idle"; label: string }) {
  return <Chip tone={tone}>{label}</Chip>;
}

/**
 * This computer, first in the list: the engine this app runs itself, its own
 * threads' count, and the name people use for it (docs/terminology.md).
 */
function ThisComputerRow({
  sessions,
  running,
  engine,
  onOpen,
}: {
  sessions: SessionSummary[];
  running: ReadonlySet<string>;
  engine: "connecting" | "ready" | "closed";
  /** Back to the project list, which is what this computer's row opens. */
  onOpen: () => void;
}) {
  const identity = useLocalIdentity();
  const status = localDeviceStatus(engine);
  const counts = machineThreadCounts(sessions, LOCAL_HOST, running);
  const name = identity.displayName ?? selfName();
  return (
    <div className="rounded-xl px-2 py-2 hover:bg-fill-subtle">
      <button
        type="button"
        onClick={onOpen}
        aria-label={`${name}, ${status.label}. Back to projects`}
        className="flex w-full flex-col gap-0.5 text-left"
      >
        <span className="flex h-5 min-w-0 items-center gap-2">
          <DeviceGlyphIcon glyph={identity.icon ?? platformIcon(localPlatform())} tint={identity.tint} />
          <span className="min-w-0 flex-1 truncate font-medium text-body text-fg">{name}</span>
          <StateChip tone={status.tone} label={status.label} />
        </span>
        <span className="min-w-0 truncate pl-6 text-sub text-fg-muted tabular">
          {threadCountLabel(counts)}
        </span>
      </button>
    </div>
  );
}

/** The offline row's action, and the one a pairing the machine ended needs instead. */
function MachineAction({ machine, connected }: { machine: Machine; connected?: boolean }) {
  const address = machine.route?.address ?? machine.routes[0]?.address;
  if (machine.state === "needsPairing") {
    return (
      <Pill
        size="sm"
        disabled={!address}
        title={
          address ? `Pair with ${machineName(machine)} again` : "That machine announced no address to dial."
        }
        onClick={() => {
          if (address) void askMachine(address, machineName(machine));
        }}
      >
        <RefreshCw size={12} />
        Pair again
      </Pill>
    );
  }
  // A machine's own view offers it too: a fresh supervisor is the way out of a
  // connection that reports itself up and answers nothing.
  if (machine.state === "connected" && !connected) return null;
  return (
    <Pill size="sm" title={`Try ${machineName(machine)} again now`} onClick={() => reconnectMachine(machine)}>
      <RefreshCw size={12} />
      Reconnect
    </Pill>
  );
}

/**
 * One paired computer in the list: its glyph and name, the chip, the line under
 * it, and, while it is away, why and the action that tries again.
 */
export function DeviceRow({
  machine,
  counts,
  now,
  onOpen,
}: {
  machine: Machine;
  counts: { running: number; total: number } | undefined;
  now: number;
  onOpen: () => void;
}) {
  const status = deviceStatus(machine);
  const meta = deviceMeta(machine, counts, now);
  const reason = offlineReason(machine);
  const name = machineName(machine);
  return (
    <div className="rounded-xl px-2 py-2 hover:bg-fill-subtle">
      <button
        type="button"
        onClick={onOpen}
        aria-label={`${name}, ${status.label}. ${meta}`}
        className="flex w-full flex-col gap-0.5 text-left"
      >
        <span className="flex h-5 min-w-0 items-center gap-2">
          <DeviceGlyphIcon glyph={machineGlyph(machine)} tint={machineTint(machine)} />
          <span className="min-w-0 flex-1 truncate font-medium text-body text-fg">{name}</span>
          <StateChip tone={status.tone} label={status.label} />
        </span>
        {/* The address is the hover's: a narrow row keeps the route's name and what works. */}
        <span className="min-w-0 truncate pl-6 text-sub text-fg-muted" title={deviceRoute(machine)}>
          {meta}
        </span>
      </button>
      {reason && (
        <div className="mt-1 flex items-start gap-2 pl-6">
          <span className="line-clamp-2 min-w-0 flex-1 text-sub text-fg-muted leading-normal" title={reason}>
            {reason}
          </span>
          <MachineAction machine={machine} />
        </div>
      )}
    </div>
  );
}

/**
 * The Devices view: this computer, then every paired one in the shell's order
 * (the main machine first), and, with none paired, how to pair one.
 */
export function DeviceList({
  now,
  onOpenMachine,
  onClose,
}: {
  now: number;
  onOpenMachine: (id: string) => void;
  onClose: () => void;
}) {
  const machines = useMachines((s) => s.machines);
  const sessions = useApp((s) => s.sessions);
  const engine = useApp((s) => s.status);
  const running = useRunningIds();
  return (
    <div className="flex flex-col gap-0.5">
      <ThisComputerRow sessions={sessions} running={running} engine={engine} onOpen={onClose} />
      {machines.map((machine) => (
        <DeviceRow
          key={machine.id}
          machine={machine}
          counts={
            machine.state === "connected" ? machineThreadCounts(sessions, machine.id, running) : undefined
          }
          now={now}
          onOpen={() => onOpenMachine(machine.id)}
        />
      ))}
      {machines.length === 0 && <NoOtherComputers />}
    </div>
  );
}

function NoOtherComputers() {
  return (
    <div className="rounded-xl px-2 py-2">
      <div className="font-medium text-body text-fg">No other computers</div>
      <p className="m-0 max-w-[34ch] pt-0.5 text-sub text-fg-muted leading-normal">
        A computer running RustCode appears here once it is paired, and its threads are listed under it.
      </p>
      <Pill size="sm" className="mt-2" onClick={() => openSettings("machines")}>
        Add a computer
      </Pill>
    </div>
  );
}

/**
 * The card above a machine's threads: what it is, the version it runs, the
 * route it answers on, and the three actions. Reconnect is here as well as on
 * the row, because a machine's own view is where someone looks when its
 * threads stop answering.
 */
export function MachineCard({ machine, now }: { machine: Machine; now: number }) {
  const [local, setLocal] = useState<string | null>(null);
  useEffect(() => {
    void localAppVersion().then(setLocal);
  }, []);
  const status = deviceStatus(machine);
  const reason = offlineReason(machine);
  const name = machineName(machine);
  const skew = versionLine(name, machine.version, local, selfName());
  const connected = machine.state === "connected";
  return (
    <div className="mx-3 mb-2 rounded-lg bg-surface p-3">
      <div className="flex min-w-0 items-center gap-2">
        <DeviceGlyphIcon glyph={machineGlyph(machine)} tint={machineTint(machine)} size={18} />
        <span className="min-w-0 flex-1 truncate font-medium text-title3 text-fg">{name}</span>
      </div>
      <div className="mt-1 flex min-w-0 flex-wrap items-center gap-x-1.5 text-sub text-fg-muted">
        <span className="shrink-0">{devicePlatform(machine.platform)}</span>
        {machine.version && (
          <span className="shrink-0">
            <span className="text-fg-faint">·</span>{" "}
            <span className="tabular">RustCode {machine.version}</span>
          </span>
        )}
      </div>
      <div className="mt-0.5 flex min-w-0 items-center gap-2">
        <StateChip tone={status.tone} label={status.label} />
        <span className="min-w-0 flex-1 truncate text-sub text-fg-muted" title={deviceRoute(machine)}>
          {deviceRoute(machine)}
        </span>
      </div>
      {!connected && machine.state !== "connecting" && machine.lastConnectedAt && (
        <div className="mt-1 text-sub text-fg-muted">Last seen {lastSeen(machine.lastConnectedAt, now)}</div>
      )}
      {reason && <p className="mt-1 mb-0 text-sub text-fg-muted leading-normal">{reason}</p>}
      {skew && <p className="mt-1 mb-0 text-sub text-warning leading-normal">{skew}</p>}
      {machine.hint && (
        <p className="mt-1 mb-0 text-sub text-fg-muted leading-normal" title={machine.hint}>
          {machine.hint}
        </p>
      )}
      <div className="mt-2 flex flex-wrap items-center gap-1.5">
        <Pill
          size="sm"
          disabled={!connected}
          title={connected ? `New thread on ${name}` : "Reconnect before starting a thread there."}
          onClick={() => startNewThread(undefined, machine.id)}
        >
          <SquarePen size={12} />
          New thread
        </Pill>
        <MachineAction machine={machine} connected />
        <Pill size="sm" quiet title={`Settings for ${name}`} onClick={() => openSettings("machines")}>
          <Gear size={12} />
          Machine settings
        </Pill>
      </div>
    </div>
  );
}
