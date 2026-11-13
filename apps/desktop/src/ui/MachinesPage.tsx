// Settings › Environments: where this app's threads can run (an "environment" is what the app
// calls a machine: a computer running RustCode, after T3 Code; `Machine` stays the name in code,
// the shell and the protocol).
//
// This computer first — the RustCode server the app starts itself — then every other computer the
// app is paired with, each with a live status (online, connecting, offline), the address and route
// it is reached over, the RustCode version it runs and its recovery action (Connect, Reconnect, Pair
// again), a rename (shared across the fleet, `state/fleet.ts`) and Remove. Computers only known
// through a paired one are listed read-only below them, with a Pair button. "Add environment" opens one dialog with every way in
// (`ui/AddEnvironmentDialog.tsx`): a connect code, SSH, or an address on the same network or a
// tailnet; the new-thread selector (`ui/EnvironmentSelector.tsx`) can open it too.
//
// The app's Rust shell holds each machine's connection supervisor (an app cannot dial a gateway
// from the web view), so this page only asks, lists and explains; `state/machines.ts` keeps what it
// is told.
//
// This computer's row also says whether the RustCode server is installed as a background service —
// the server that keeps this computer reachable with the app closed, and that Settings › Remote
// access installs when network access is turned on. Threads this app runs stay in the app and end
// with it. A row of another environment says which RustCode version it runs, so an old one is a
// sentence with a command, not a silent connection failure.

import { Plus, RefreshCw, Server, SquareTerminal } from "lucide-react";
import { useEffect, useState } from "react";
import { currentPlatform } from "../lib/platform";
import { hasShell } from "../lib/shell";
import { knownOnly, useFleet, viaName } from "../state/fleet";
import {
  askInstallServer,
  askMachine,
  defaultSelfName,
  gatewayLine,
  lastSeen,
  linkCliToApp,
  localAppVersion,
  type Machine,
  machineGlyph,
  machineLine,
  machineName,
  machineTint,
  platformIcon,
  platformName,
  reconnectMachine,
  refreshMachines,
  refreshServerService,
  removeMachine,
  selfName,
  serverLine,
  setAddEnvironmentOpen,
  setLocalIdentity,
  setMachineDisplayName,
  setMachineIcon,
  threadCount,
  threadLabel,
  useLocalIdentity,
  useMachines,
  versionSkew,
} from "../state/machines";
import { openMachineSettings, useApp } from "../state/store";
import { AddEnvironmentDialog } from "./AddEnvironmentDialog";
import {
  environmentAddress,
  environmentOutdated,
  environmentRecovery,
  environmentRoute,
  environmentStatus,
  knownEnvironment,
  recoveryLabel,
} from "./environments";
import { DeviceGlyphIcon } from "./MachineGlyph";
import { MachineIdentityEditor } from "./MachineProfile";
import { Spinner } from "./primitives";
import { Action, Chip, Group, Loading, Row } from "./settingsKit";
import { ThreadMirrorGroup } from "./ThreadMirrorGroup";

/** How often the page asks the shell where the environments stand. */
const POLL_MS = 3000;

export function MachinesPage() {
  const ready = useMachines((s) => s.ready);
  const error = useMachines((s) => s.error);
  const machines = useMachines((s) => s.machines);
  const counts = useMachines((s) => s.counts);
  const addOpen = useMachines((s) => s.addOpen);
  const adding = useMachines(
    (s) => s.pairing !== null || s.ssh?.running === true || s.connectBusy || s.connectOffer !== null,
  );
  const [now, setNow] = useState(() => Date.now());
  const [local, setLocal] = useState<string | null>(null);

  // The shell learns about an environment on its own (a retry that worked, a pairing ended
  // there), so the page keeps asking while it is open.
  useEffect(() => {
    if (!hasShell()) return;
    void refreshMachines();
    const timer = setInterval(() => void refreshMachines(), POLL_MS);
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

  if (!ready) return <Loading error={error} onRetry={() => void refreshMachines()} />;

  return (
    <>
      {error && <p className="mb-6 text-callout text-warning">Could not read the environments: {error}</p>}

      <Group
        title="Environments"
        action={
          <Action prominent onClick={() => setAddEnvironmentOpen(true)}>
            <Plus size={13} />
            Add environment
          </Action>
        }
        footer="Choose where a thread runs under Run on when you start it."
        hint="A LAN route is authenticated but not encrypted; Tailscale and VPN tunnels encrypt what they carry. Removing an environment forgets its pairing here; it keeps listing this computer until you revoke it there."
      >
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
        <KnownEnvironments now={now} local={local} />
        {machines.length === 0 && <NoEnvironments />}
        {adding && !addOpen && (
          <Row
            title="Adding an environment…"
            subtitle="A connection is in progress. Open the dialog to see where it stands."
          >
            <Spinner />
            <Action onClick={() => setAddEnvironmentOpen(true)}>Show progress</Action>
          </Row>
        )}
      </Group>

      {machines.length > 0 && <ThreadMirrorGroup />}

      <AddEnvironmentDialog open={addOpen} onOpenChange={setAddEnvironmentOpen} local={local} />
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
  const status = useApp((s) => s.status);
  const open = useApp((s) => Object.values(s.threads).filter((thread) => thread.loaded).length);
  const server = useMachines((s) => s.server);
  const installing = useMachines((s) => s.serverInstalling);
  const linking = useMachines((s) => s.serverLinking);
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
          <Chip>This computer</Chip>
        </>
      }
      subtitle={`Local server · ${open === 1 ? "1 thread" : `${open} threads`}`}
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
          {server?.gateway.remoteEnabled || server?.gateway.running ? (
            <span className="block pt-0.5">Gateway: {gatewayLine(server.gateway, server.installed)}</span>
          ) : null}
          {server?.problems.map((problem) => (
            <span key={problem.code} className="block pt-0.5 text-warning">
              {problem.message}
            </span>
          ))}
        </>
      }
      badge={
        <Chip tone={running ? "ok" : "warn"}>
          {running ? "Online" : status === "connecting" ? "Connecting" : "Offline"}
        </Chip>
      }
    >
      {server && !installed && (
        <Action disabled={installing} onClick={() => askInstallServer()}>
          {installing ? <Spinner /> : <Server size={13} />}
          Install server
        </Action>
      )}
      <MachineIdentityEditor
        label={`${selfName()} on this computer`}
        reportedName={defaultSelfName()}
        platform={reported}
        identity={local}
        onName={(name) => setLocalIdentity({ displayName: name })}
        onIcon={(icon, tint) => setLocalIdentity({ icon, tint })}
        trigger={
          <Action quiet size="sm">
            Rename
          </Action>
        }
      />
      {/* One action for a terminal's older `rustcode`: the install script's copy
          or any other that Homebrew does not own (issue 3411). */}
      {server?.problems.some((problem) => problem.code === "path-version-mismatch") && (
        <Action disabled={linking} onClick={() => void linkCliToApp()}>
          {linking ? <Spinner /> : <SquareTerminal size={13} />}
          Use this app's version in a terminal
        </Action>
      )}
    </Row>
  );
}

/**
 * Computers the paired ones are paired with, which this computer is not: listed read-only, with
 * who told us, so the fleet is visible from any one computer. Knowing one is not access to it; it
 * is paired on its own, with **Pair** (`state/fleet.ts`).
 */
function KnownEnvironments({ now, local }: { now: number; local: string | null }) {
  const machines = useMachines((s) => s.machines);
  const entries = useFleet((s) => s.entries);
  const known = knownOnly(entries, machines);
  return (
    <>
      {known.map((entry) => {
        const row = knownEnvironment(entry, viaName(entry, machines, entries), local, now);
        return (
          <Row
            key={row.id}
            title={
              <>
                <DeviceGlyphIcon glyph={platformIcon(row.platform)} size={16} />
                {row.name}
                {row.outdated && <Chip tone="warn">Outdated</Chip>}
              </>
            }
            subtitle={
              row.via
                ? `Known through ${row.via}. Not paired on this computer.`
                : "Not paired on this computer."
            }
            status={row.facts.join(" · ")}
            badge={<Chip tone="idle">Not paired</Chip>}
          >
            <Action onClick={() => setAddEnvironmentOpen(true)}>
              <Plus size={12} /> Pair
            </Action>
          </Row>
        );
      })}
    </>
  );
}

function NoEnvironments() {
  return (
    <Row
      title="No other environments yet"
      subtitle="Add a computer running RustCode to see its threads here, and to start threads on it."
    >
      <Action onClick={() => setAddEnvironmentOpen(true)}>
        <Plus size={13} />
        Add environment
      </Action>
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
  const platform = platformName(machine.platform);
  const threads = threadLabel(count);
  const skew = versionSkew(machine.version, local, machine.platform);
  const outdated = environmentOutdated(machine.version, local);
  const hint = machine.hint;
  const name = machineName(machine);
  const status = environmentStatus(machine);
  const recovery = environmentRecovery(machine);
  const connected = machine.state === "connected";
  const facts = [
    platform,
    machine.version ? `RustCode ${machine.version}` : undefined,
    machine.lastConnectedAt ? `Last seen ${lastSeen(machine.lastConnectedAt, now)}` : "Not reached yet",
    threads,
  ].filter(Boolean);
  return (
    <Row
      stacked
      title={
        <>
          <span
            aria-hidden
            data-status={status.kind}
            className={`size-2 shrink-0 rounded-full ${
              status.kind === "online"
                ? "bg-success"
                : status.kind === "attention"
                  ? "bg-warning"
                  : status.kind === "connecting"
                    ? "animate-pulse bg-fg-muted"
                    : "bg-fg-faint"
            }`}
          />
          <DeviceGlyphIcon glyph={machineGlyph(machine)} tint={machineTint(machine)} size={16} />
          {name}
          {outdated && <Chip tone="warn">Outdated</Chip>}
        </>
      }
      subtitle={
        <>
          {/* Online: where it is and how it travels. Otherwise what is wrong, then the route it tries. */}
          <span className="selectable block">
            {connected ? environmentRoute(machine) : machineLine(machine)}
          </span>
          {!connected && environmentAddress(machine) && (
            <span className="selectable block">{environmentRoute(machine)}</span>
          )}
          {/* Who is older, and what updates it: the Mac Studio on 0.96 against a 1.28 app. */}
          {skew && <span className="mt-1 block text-warning">{skew}</span>}
          {/* PR 3402's words for a route that does not answer while another does. */}
          {hint && <span className="mt-1 block">{hint}</span>}
        </>
      }
      status={facts.join(" · ")}
      badge={<Chip tone={status.tone}>{status.label}</Chip>}
    >
      {recovery === "pair" ? (
        <PairAgainAction machine={machine} />
      ) : recovery ? (
        <Action onClick={() => reconnectMachine(machine)}>
          <RefreshCw size={12} /> {recoveryLabel(recovery)}
        </Action>
      ) : null}
      <MachineIdentityEditor
        label={`${name} on this computer`}
        reportedName={machine.name}
        platform={machine.platform}
        identity={{
          displayName: machine.displayName ?? null,
          icon: machineGlyph(machine),
          tint: machineTint(machine),
        }}
        onName={(next) => void setMachineDisplayName(machine, next)}
        onIcon={(icon, tint) => void setMachineIcon(machine, icon, tint)}
        trigger={
          <Action quiet size="sm">
            Rename
          </Action>
        }
      />
      <Action size="sm" onClick={() => openMachineSettings(machine.id)}>
        Settings
      </Action>
      <Action quiet size="sm" aria-label={`Remove ${name}`} onClick={() => removeMachine(machine)}>
        Remove
      </Action>
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
        setAddEnvironmentOpen(true);
      }}
    >
      <RefreshCw size={12} /> Pair again
    </Action>
  );
}
