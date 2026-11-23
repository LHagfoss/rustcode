import { isLoaded } from "../state/thread";
// Settings › Machines: where this app's threads can run (a machine is a computer running
// RustCode; docs/terminology.md › Machine).
//
// This computer first — the RustCode server the app starts itself — then every other computer the
// app is paired with, each with a live status (online, connecting, offline), the address and route
// it is reached over, the RustCode version it runs and its recovery action (Connect, Reconnect, Pair
// again), and a link to its settings. Computers only known
// through a paired one are listed read-only below them, with a Pair button. "Add machine" opens one dialog with every way in
// (`ui/AddMachineDialog.tsx`): a connect code, SSH, or an address on the same network or a
// tailnet; the new-thread selector (`ui/MachineSelector.tsx`) can open it too.
//
// The app's Rust shell holds each machine's connection supervisor (an app cannot dial a gateway
// from the web view), so this page only asks, lists and explains; `state/machines.ts` keeps what it
// is told.
//
// This computer's row also says whether the RustCode server is installed as a background service —
// the server that keeps this computer reachable with the app closed, and that Settings › Remote
// access installs when network access is turned on. Threads this app runs stay in the app and end
// with it. A row of another machine says which RustCode version it runs, so an old one is a
// sentence with a command, not a silent connection failure.

import { useEffect, useState } from "react";
import { currentPlatform } from "../lib/platform";
import { hasShell } from "../lib/shell";
import { readStorage, writeStorage } from "../lib/storage";
import {
  type FleetEntry,
  knownOnly,
  setKnownHidden,
  showHiddenKnown,
  useFleet,
  useHiddenKnown,
  viaName,
} from "../state/fleet";
import {
  askInstallServer,
  askMachine,
  gatewayLine,
  LOCAL_MACHINE_ID,
  lastSeen,
  localAppVersion,
  type Machine,
  machineGlyph,
  machineName,
  machineTint,
  platformIcon,
  platformName,
  reconnectMachine,
  refreshMachines,
  refreshServerService,
  selfName,
  serverLine,
  setAddMachineOpen,
  threadCount,
  threadLabel,
  useLocalIdentity,
  useMachines,
  versionSkew,
} from "../state/machines";
import {
  remoteUpdateLine,
  updateMachine,
  updateMachines,
  updateRunning,
  useMachineUpdates,
} from "../state/machineUpdate";
import { refreshRemote } from "../state/remoteAccess";
import { openMachineSettings, useApp } from "../state/store";
import { AddMachineDialog } from "./AddMachineDialog";
import { connectionDirection, peerInbound } from "./connectionDirection";
import { ForwardTakeover } from "./ForwardTakeover";
import { HubGroup } from "./HubGroup";
import { Settings } from "./icons";
import { DeviceGlyphIcon } from "./MachineGlyph";
import { MachinesMap, SELF_NODE } from "./MachinesMap";
import {
  knownMachine,
  machineOutdated,
  machineRecovery,
  machineRoute,
  machineStatus,
  recoveryLabel,
} from "./machineLabels";
import { ChromeButton, Segments, Spinner } from "./primitives";
import { ServerProblems } from "./ServerProblems";
import { Action, Chip, Group, Row, SettingsHeaderActions, withCode } from "./settingsKit";
import { ThreadMirrorGroup } from "./ThreadMirrorGroup";

/** How often the page asks the shell where the machines stand. */
const POLL_MS = 3000;

const VIEW_KEY = "rustcode.machinesView";

type MachinesView = "map" | "list";

export function MachinesPage() {
  const ready = useMachines((s) => s.ready);
  const error = useMachines((s) => s.error);
  const machines = useMachines((s) => s.machines);
  const counts = useMachines((s) => s.counts);
  const addOpen = useMachines((s) => s.addOpen);
  const adding = useMachines(
    (s) =>
      s.pairing.status === "running" ||
      s.ssh.status === "running" ||
      s.connect.status === "running" ||
      s.connectOffer !== null,
  );
  const [now, setNow] = useState(() => Date.now());
  const [local, setLocal] = useState<string | null>(null);
  const [view, setView] = useState<MachinesView>(() => (readStorage(VIEW_KEY) === "list" ? "list" : "map"));
  const [selected, setSelected] = useState<string | null>(null);
  const entries = useFleet((s) => s.entries);
  const hidden = useHiddenKnown((s) => s.ids);
  const known = knownOnly(entries, machines, hidden);
  const hiddenCount = knownOnly(entries, machines).length - known.length;
  const chooseView = (next: MachinesView) => {
    setView(next);
    writeStorage(VIEW_KEY, next);
  };
  const updates = useMachineUpdates((s) => s.byMachine);
  // Every connected machine on an older RustCode than this app, not already updating: one press
  // updates them all, as each row's Update does.
  const outdated = machines.filter(
    (m) =>
      m.state === "connected" && machineOutdated(m.version, local) && !updateRunning(updates[m.id]?.update),
  );

  // The shell learns about a machine on its own (a retry that worked, a pairing ended
  // there), so the page keeps asking while it is open.
  useEffect(() => {
    if (!hasShell()) return;
    void refreshMachines();
    void refreshRemote();
    const timer = setInterval(() => {
      void refreshMachines();
      void refreshRemote();
    }, POLL_MS);
    return () => clearInterval(timer);
  }, []);

  // Countdowns: "retrying in 4 s".
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);

  // The background server and this app's own version: both read once, and again after an install.
  useEffect(() => {
    void refreshServerService();
    void localAppVersion().then(setLocal);
  }, []);

  return (
    <>
      <SettingsHeaderActions>
        {outdated.length > 1 && (
          <Action onClick={() => updateMachines(outdated)}>Update all ({outdated.length})</Action>
        )}
        <Action prominent onClick={() => setAddMachineOpen(true)}>
          Add machine
        </Action>
      </SettingsHeaderActions>
      {!ready && (
        <p role="status" className="mb-4 text-callout text-fg-muted">
          Updating machines…
        </p>
      )}
      {error && <p className="mb-6 text-callout text-warning">Could not read the machines: {error}</p>}

      <Group
        title="Network"
        action={<ViewSwitch view={view} onChange={chooseView} />}
        hint="Choose where a thread runs under Run on when you start it. A LAN route is authenticated but not encrypted; Tailscale and VPN tunnels encrypt what they carry. Removing a machine forgets its pairing here; it keeps listing this computer until you revoke it there."
      >
        {view === "map" ? (
          <MachinesMap
            machines={machines}
            known={known}
            selected={selected}
            onSelect={setSelected}
            footer={
              <SelectedMachine
                id={selected}
                machines={machines}
                known={known}
                counts={counts}
                now={now}
                local={local}
              />
            }
          />
        ) : (
          <>
            <ThisComputer />
            {machines.map((machine) => (
              <MachineRow
                key={machine.id}
                machine={machine}
                count={threadCount(machine, counts)}
                now={now}
                local={local}
              />
            ))}
            {known.map((entry) => (
              <KnownMachineRow key={entry.hostId} entry={entry} now={now} local={local} />
            ))}
          </>
        )}
        {machines.length === 0 && <NoMachines />}
        {hiddenCount > 0 && (
          <Row
            title={hiddenCount === 1 ? "1 hidden machine" : `${hiddenCount} hidden machines`}
            subtitle="Known through another machine and hidden here."
          >
            <Action size="sm" onClick={showHiddenKnown}>
              Show
            </Action>
          </Row>
        )}
        {adding && !addOpen && (
          <Row
            title="Adding a machine…"
            subtitle="A connection is in progress. Open the dialog to see where it stands."
          >
            <Spinner />
            <Action onClick={() => setAddMachineOpen(true)}>Show progress</Action>
          </Row>
        )}
      </Group>

      {machines.length > 0 && <ThreadMirrorGroup />}

      <HubGroup />

      <AddMachineDialog open={addOpen} onOpenChange={setAddMachineOpen} local={local} />
    </>
  );
}

/**
 * This app's own computer: the local RustCode server, over the local socket,
 * and whether the server is installed to run in the background — which keeps
 * this computer reachable with the app closed (docs/server.md). The threads
 * this app runs are its own engine's, and end when the app does.
 */
function ThisComputer() {
  const status = useApp((s) => s.engine.phase);
  const open = useApp((s) => Object.values(s.threads).filter((thread) => isLoaded(thread)).length);
  const server = useMachines((s) => s.server);
  const installing = useMachines((s) => s.serverInstall.status === "running");
  const local = useLocalIdentity();
  const platform = currentPlatform();
  const reported = platform === "mac" ? "macos" : platform;
  const running = status === "ready";
  const installed = server?.installed === true;
  return (
    <Row
      title={
        <>
          <DeviceGlyphIcon glyph={local.icon ?? platformIcon(reported)} tint={local.tint} size={16} />
          {selfName()}
          <Chip tone={running ? "ok" : "warn"}>
            {running ? "Online" : status === "connecting" ? "Connecting" : "Offline"}
          </Chip>
          <span className="font-normal text-caption text-fg-faint">This computer</span>
        </>
      }
      subtitle={`Local server · ${open === 1 ? "1 thread" : `${open} threads`}`}
      hint={
        server?.gateway.remoteEnabled || server?.gateway.running
          ? `Gateway: ${gatewayLine(server.gateway, server.installed)}`
          : undefined
      }
      status={
        <>
          <span className="tabular">Server: {serverLine(server)}</span>
          {server?.version && server.version !== server.cliVersion ? (
            <span className="text-warning">
              {" "}
              · it runs {server.version}, this app ships {server.cliVersion}
            </span>
          ) : null}
          {/* The gateway with the app closed: the server keeps it running, the
              app's own threads still end with the app (docs/remote.md). */}
          {server?.gateway.running ? (
            <span className="tabular"> · Gateway on {server.gateway.address ?? "an unknown address"}</span>
          ) : server?.gateway.remoteEnabled ? (
            <span> · Gateway not running</span>
          ) : null}
          <ServerProblems problems={server?.problems} />
        </>
      }
    >
      {server && !installed && (
        <Action disabled={installing} onClick={() => askInstallServer()}>
          {installing ? <Spinner /> : null}
          Install server
        </Action>
      )}
      <MachineSettingsButton name={selfName()} onClick={() => openMachineSettings(LOCAL_MACHINE_ID)} />
    </Row>
  );
}

/** Switches the Machines group between the map and the list; the choice is kept. */
function ViewSwitch({ view, onChange }: { view: MachinesView; onChange: (view: MachinesView) => void }) {
  return (
    <Segments
      label="Machines view"
      value={view}
      options={[
        { value: "map", label: "Map" },
        { value: "list", label: "List" },
      ]}
      onChange={onChange}
    />
  );
}

/** The list row of the node selected on the map, with the actions the list has for it. */
function SelectedMachine({
  id,
  machines,
  known,
  counts,
  now,
  local,
}: {
  id: string | null;
  machines: readonly Machine[];
  known: readonly FleetEntry[];
  counts: Record<string, number>;
  now: number;
  local: string | null;
}) {
  if (id === null) return null;
  if (id === SELF_NODE) return <ThisComputer />;
  const machine = machines.find((saved) => saved.id === id);
  if (machine) {
    return <MachineRow machine={machine} count={threadCount(machine, counts)} now={now} local={local} />;
  }
  const entry = known.find((candidate) => candidate.hostId === id);
  return entry ? <KnownMachineRow entry={entry} now={now} local={local} /> : null;
}

/**
 * A computer a paired one is paired with, which this computer is not: listed read-only, with who
 * told us, so the fleet is visible from any one computer. Knowing one is not access to it; it is
 * paired on its own, with **Pair** (`state/fleet.ts`).
 */
function KnownMachineRow({ entry, now, local }: { entry: FleetEntry; now: number; local: string | null }) {
  const machines = useMachines((s) => s.machines);
  const entries = useFleet((s) => s.entries);
  const row = knownMachine(entry, viaName(entry, machines, entries), local, now);
  return (
    <Row
      title={
        <>
          <DeviceGlyphIcon glyph={platformIcon(row.platform)} size={16} />
          {row.name}
          {row.outdated && <Chip tone="warn">Outdated</Chip>}
        </>
      }
      subtitle={
        row.via
          ? `Paired with ${row.via}, not with this computer. To remove it everywhere, remove it on ${row.via}.`
          : "Not paired on this computer."
      }
      status={row.facts.join(" · ")}
    >
      <Action onClick={() => setKnownHidden(entry.hostId, true)}> Hide</Action>
      <Action onClick={() => setAddMachineOpen(true)}> Pair</Action>
    </Row>
  );
}

function NoMachines() {
  return (
    <Row
      title="No other machines yet"
      subtitle="Add a computer running RustCode to see its threads here, and to start threads on it."
    >
      <Action onClick={() => setAddMachineOpen(true)}>Add machine</Action>
    </Row>
  );
}

export function MachineRow({
  machine,
  count,
  now,
  local,
}: {
  machine: Machine;
  count: number | undefined;
  now: number;
  /** This app's version, for the row that says another machine runs another. */
  local: string | null;
}) {
  const threads = threadLabel(count);
  const outdated = machineOutdated(machine.version, local);
  const name = machineName(machine);
  const status = machineStatus(machine);
  const recovery = machineRecovery(machine);
  const devices = useApp((s) => s.remote?.devices);
  const connected = machine.state === "connected";
  const direction = connectionDirection(connected, peerInbound(machine.id, devices ?? []), name);
  const facts = [
    platformName(machine.platform),
    machine.version ? `RustCode ${machine.version}` : undefined,
    machine.lastConnectedAt ? `Last seen ${lastSeen(machine.lastConnectedAt, now)}` : "Not reached yet",
    threads,
    // A probe from before the link dropped says nothing about the route now.
    connected && machine.route?.probe?.latencyMs != null
      ? `Route latency ${machine.route.probe.latencyMs} ms`
      : undefined,
  ].filter(Boolean);
  // What to do about another version, in the row itself rather than behind the chip.
  const skew = versionSkew(machine.version, local, machine.platform);
  // An update started from here (Update): where it stands, in place of the advice.
  const update = useMachineUpdates((s) => s.byMachine[machine.id]?.update);
  const updating = updateRunning(update);
  const updateLine = update ? remoteUpdateLine(update, name) : null;
  return (
    <Row
      title={
        <>
          <DeviceGlyphIcon glyph={machineGlyph(machine)} tint={machineTint(machine)} size={16} />
          {name}
          <span data-status={status.kind}>
            <Chip tone={status.tone}>{status.label}</Chip>
          </span>
          {outdated && <Chip tone="warn">Outdated</Chip>}
        </>
      }
      hint={
        machine.hint ? (
          <>
            {direction}
            <br />
            {machine.hint}
          </>
        ) : (
          direction
        )
      }
      subtitle={<span className="selectable break-words">{machineRoute(machine)}</span>}
      status={
        <>
          <span className="tabular">{facts.join(" · ")}</span>
          {updateLine ? (
            <span
              role="status"
              className={`mt-1 block ${update?.phase === "failed" ? "text-warning" : update?.phase === "done" ? "text-success" : "text-fg-muted"}`}
            >
              {withCode(updateLine)}
            </span>
          ) : (
            skew && <span className="mt-1 block text-warning">{withCode(skew)}</span>
          )}
          {machine.forward && !connected && <ForwardTakeover machine={machine} forward={machine.forward} />}
        </>
      }
    >
      {(outdated || updating) && connected && (
        <Action disabled={updating} onClick={() => void updateMachine(machine)}>
          {updating ? <Spinner /> : null}
          {updating ? "Updating…" : update?.phase === "failed" ? "Retry update" : "Update"}
        </Action>
      )}
      {recovery === "pair" ? (
        <PairAgainAction machine={machine} />
      ) : recovery ? (
        <Action onClick={() => reconnectMachine(machine)}>
          {status.kind === "connecting" ? "Retry now" : recoveryLabel(recovery)}
        </Action>
      ) : null}
      <MachineSettingsButton name={name} onClick={() => openMachineSettings(machine.id)} />
    </Row>
  );
}

/** A machine that ended the pairing: asking it again is what brings it back. */
function PairAgainAction({ machine }: { machine: Machine }) {
  const address = machine.route?.address ?? machine.routes[0]?.address;
  return (
    <Action
      disabled={!address}
      onClick={() => {
        if (!address) return;
        void askMachine(address, machineName(machine));
        setAddMachineOpen(true);
      }}
    >
      Pair again
    </Action>
  );
}

/**
 * A machine's settings, as a gear at the row's end: every row ends on the same control in the same
 * place, and the row's one text button is the action that changes its state.
 */
function MachineSettingsButton({ name, onClick }: { name: string; onClick: () => void }) {
  return (
    <ChromeButton label={`Settings for ${name}`} tooltip="Machine settings" onClick={onClick}>
      <Settings size={14} />
    </ChromeButton>
  );
}
