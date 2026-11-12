// One machine's own Settings page (docs/desktop.md › Settings on one machine
// or all, and › Several machines): what that computer is, the controls this app
// has over it, its harnesses, and which Settings rows are shared, kept there,
// or this app's own. Reached from the machine view's Machine settings button
// and from Settings › Machines.
//
// The page never writes another machine's values in place: a row opens the
// setting it belongs to with **Applying to** set to this machine, so the write
// goes through the same scope bar as every other server setting. A row set from
// here waits for that machine's next launch, which the footer says.

import { ArrowLeft, RefreshCw, Trash2, Unplug } from "lucide-react";
import { useEffect, useState } from "react";
import type { Harness } from "../acp/types";
import { devicePlatform, deviceRoute, deviceStatus, versionLine } from "../state/deviceRows";
import { accountState } from "../state/harness";
import {
  disconnectMachine,
  lastSeen,
  localAppVersion,
  type Machine,
  machineGlyph,
  machineName,
  machineTint,
  probeLabel,
  probeMachineRoutes,
  probeTone,
  reconnectMachine,
  removeMachine,
  routeLabel,
  selfName,
  setMachineDisplayName,
  setMachineIcon,
  setMachinePaused,
  setMainMachine,
  useMachines,
} from "../state/machines";
import { SERVER_SETTINGS } from "../state/settingsRegistry";
import {
  checkMachineHarness,
  loadServerSettings,
  localServerSettingValues,
  serverValueDiffers,
  useMachineHarness,
  useServerValues,
  useSettingsScope,
} from "../state/settingsScope";
import { openSettings } from "../state/store";
import { LOCAL_HOST } from "../state/threadKeys";
import { DeviceGlyphIcon } from "./MachineGlyph";
import { MachineIdentityEditor } from "./MachineProfile";
import { cx, Switch } from "./primitives";
import { Action, Chip, Group, Row, SectionTitle } from "./settingsKit";
import { SYNC_COLUMNS, syncRows } from "./syncRows";

/** One machine's name, state and controls. */
export function MachineSettingsPage({ id }: { id: string }) {
  const machine = useMachines((state) => state.machines.find((entry) => entry.id === id));
  if (!machine) {
    return (
      <Group title="Machine">
        <Row
          title="That machine is no longer saved"
          subtitle="It was removed on this computer. Back to Machines lists the others."
        >
          <Action onClick={() => openSettings("machines")}>
            <ArrowLeft size={13} />
            Machines
          </Action>
        </Row>
      </Group>
    );
  }
  return <MachineSettings machine={machine} />;
}

function MachineSettings({ machine }: { machine: Machine }) {
  const [local, setLocal] = useState<string | null>(null);
  const status = deviceStatus(machine);
  const name = machineName(machine);
  const online = machine.state === "connected";
  const skew = versionLine(name, machine.version, local, selfName());

  useEffect(() => {
    void localAppVersion().then(setLocal);
  }, []);

  return (
    <>
      <div className="mb-3 flex items-center gap-2 px-4">
        <Action quiet size="sm" onClick={() => openSettings("machines")}>
          <ArrowLeft size={13} />
          Machines
        </Action>
      </div>

      <Group title="Machine">
        <div className="flex flex-col gap-3 p-4">
          <div className="flex min-w-0 items-start gap-3">
            <span className="flex size-10 shrink-0 items-center justify-center rounded-xl bg-fill">
              <DeviceGlyphIcon glyph={machineGlyph(machine)} tint={machineTint(machine)} size={22} />
            </span>
            <div className="min-w-0 flex-1">
              <div className="flex min-w-0 flex-wrap items-center gap-2">
                <h3 className="min-w-0 truncate font-semibold text-title3 text-fg">{name}</h3>
                <Chip tone={status.tone}>{status.label}</Chip>
              </div>
              <div className="pt-0.5 text-callout text-fg-muted">
                {devicePlatform(machine.platform)}
                {machine.version && <span className="tabular"> · RustCode {machine.version}</span>}
                {machine.main && " · Main machine"}
                {machine.paused && " · Paused for new threads"}
              </div>
              <div className="pt-0.5 text-callout text-fg-muted" title={deviceRoute(machine)}>
                {deviceRoute(machine)}
                {machine.lastConnectedAt && !online && <> · Last seen {lastSeen(machine.lastConnectedAt)}</>}
              </div>
              {skew && <p className="m-0 pt-1 text-callout text-warning">{skew}</p>}
              {machine.hint && <p className="m-0 pt-1 text-callout text-fg-muted">{machine.hint}</p>}
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
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
              trigger={<Action size="sm">Name and icon</Action>}
            />
            <Action size="sm" onClick={() => reconnectMachine(machine)}>
              <RefreshCw size={12} />
              Reconnect
            </Action>
            <Action size="sm" disabled={!online} onClick={() => void disconnectMachine(machine)}>
              <Unplug size={12} />
              Disconnect
            </Action>
            <Action size="sm" danger onClick={() => removeMachine(machine)}>
              <Trash2 size={12} />
              Forget
            </Action>
          </div>
        </div>
      </Group>

      <Group title="Controls">
        <Row
          title="Make default for new threads"
          subtitle={`A new thread starts here unless Run on names another computer. ${machine.main ? "This machine is the default." : "This computer is the default now."}`}
        >
          <Switch
            label={`Make ${name} the default for new threads`}
            checked={machine.main}
            onChange={(next) => next && void setMainMachine(machine)}
          />
        </Row>
        <Row
          title="Pause new threads here"
          subtitle="The connection and the threads already on it stay as they are; Run on refuses this machine until it is resumed."
        >
          <Switch
            label={`Pause new threads on ${name}`}
            checked={machine.paused === true}
            onChange={(next) => void setMachinePaused(machine, next)}
          />
        </Row>
      </Group>

      <Routes machine={machine} />
      <Harnesses machine={machine} />
      <WhatSyncs machine={machine} online={online} />
    </>
  );
}

/** Every route saved for the machine with what the last test of it found, and one button that tests them all now. */
function Routes({ machine }: { machine: Machine }) {
  const probing = useMachines((state) => state.probing);
  return (
    <Group
      title="Routes"
      action={
        <Action quiet size="sm" disabled={probing} onClick={() => void probeMachineRoutes(machine)}>
          <RefreshCw size={12} />
          {probing ? "Testing…" : "Test routes"}
        </Action>
      }
      footer="Test routes opens a plain connection to each address, with no sign-in, and shows whether it answers and how fast. The app tries the LAN first and the VPN when the LAN does not answer."
    >
      {machine.routes.length === 0 && (
        <Row title="No route saved" subtitle="Pair the machine again to add one." />
      )}
      {machine.routes.map((route) => (
        <Row
          key={route.address}
          title={routeLabel(route)}
          subtitle={
            route.probe?.detail ??
            (route.kind === "public" && !route.probe
              ? "Not tested: this app never sends a sign-in to a public address, so it never connects to one."
              : undefined)
          }
          status={
            <span className="tabular">
              {route.address}
              {route.probe && ` · tested ${lastSeen(route.probe.at)}`}
            </span>
          }
          badge={<Chip tone={probeTone(route.probe)}>{probeLabel(route.probe)}</Chip>}
        />
      ))}
    </Group>
  );
}

/** The harnesses that machine reports, what they need, and one Check for all of them. */
function Harnesses({ machine }: { machine: Machine }) {
  const harness = useMachineHarness(machine.id);
  const [busy, setBusy] = useState(false);
  const [checked, setChecked] = useState<string | null>(null);
  const harnesses = harness.snapshot?.harnesses ?? [];
  const online = machine.state === "connected";
  const check = async () => {
    setBusy(true);
    setChecked(null);
    const results: string[] = [];
    for (const entry of harnesses) {
      results.push(`${entry.name}: ${await checkMachineHarness(machine.id, entry.id)}`);
    }
    setChecked(results.join(" · ") || "The machine listed no harnesses.");
    setBusy(false);
  };
  return (
    <Group
      title="Harnesses"
      action={
        <Action quiet size="sm" disabled={!online || busy} onClick={() => void check()}>
          <RefreshCw size={12} />
          {busy ? "Checking…" : "Check"}
        </Action>
      }
      footer={
        checked ??
        (online
          ? "Check re-detects each harness's install and sign-in on that machine."
          : "Last known values while the machine is away; it is read-only until it is back.")
      }
    >
      {harness.loading && harnesses.length === 0 && (
        <Row title="Reading harnesses…" subtitle="Asking that machine what it can run." />
      )}
      {harness.error && harnesses.length === 0 && (
        <Row title="Could not read the harnesses" subtitle={harness.error} />
      )}
      {!harness.loading && !harness.error && harnesses.length === 0 && (
        <Row title="No harnesses listed" subtitle="That machine reported none." />
      )}
      {harnesses.map((entry) => (
        <Row key={entry.id} title={entry.name} status={harnessStatus(entry)} />
      ))}
    </Group>
  );
}

/** One line per harness: installed and signed in, needs sign-in, or not installed. */
function harnessStatus(harness: Harness): string {
  if (!harness.installed) {
    return harness.command[0] ? `Not installed: ${harness.command[0]} was not found` : "Not installed";
  }
  const accounts = accountState(harness);
  if (accounts === "needs_login" || (accounts === undefined && harness.auth === "needs_login")) {
    return "Needs sign-in";
  }
  return harness.version ? `Ready · ${harness.version}` : "Ready";
}

/**
 * What syncs, from the one registry: every Settings row in its column, with a
 * mark where this machine's value differs from this Mac's. A row opens its own
 * page with Applying to set to this machine.
 */
function WhatSyncs({ machine, online }: { machine: Machine; online: boolean }) {
  const values = useServerValues();
  const local = localServerSettingValues();
  const setScope = useSettingsScope((state) => state.setScope);
  const rows = syncRows();
  const specByTitle = new Map(SERVER_SETTINGS.map((spec) => [spec.label, spec.key]));

  useEffect(() => {
    if (!online) return;
    void loadServerSettings(machine.id);
    void loadServerSettings(LOCAL_HOST);
  }, [machine.id, online]);

  const open = (section: Parameters<typeof openSettings>[0]) => {
    if (!online) return;
    setScope({ mode: "machine", host: machine.id });
    openSettings(section);
  };

  return (
    <>
      <SectionTitle title="What syncs" />
      <div className="mb-6 overflow-hidden rounded-xl bg-surface">
        <div className="grid grid-cols-[minmax(0,1fr)_auto_auto_auto] items-center gap-x-4 border-border border-b px-4 py-2 font-medium text-caption text-fg-muted">
          <span>Setting</span>
          {SYNC_COLUMNS.map((column) => (
            <span key={column.column} className="w-16 text-center">
              {column.label}
            </span>
          ))}
        </div>
        <div className="divide-y divide-border">
          {rows.map((row) => {
            const key = specByTitle.get(row.title);
            const differs =
              row.column === "shared" &&
              key !== undefined &&
              serverValueDiffers(values, machine.id, key, local);
            const cells = (
              <>
                <span className="flex min-w-0 items-center gap-2">
                  <span className="min-w-0 truncate text-body text-fg">{row.title}</span>
                  {differs && <Chip tone="warn">Differs</Chip>}
                </span>
                {SYNC_COLUMNS.map((column) => (
                  <span
                    key={column.column}
                    className={
                      column.column === row.column
                        ? "w-16 text-center text-body text-accent"
                        : "w-16 text-center text-body text-fg-faint"
                    }
                    title={column.column === row.column ? column.label : undefined}
                  >
                    {column.column === row.column ? "●" : "·"}
                  </span>
                ))}
              </>
            );
            const classes =
              "grid w-full grid-cols-[minmax(0,1fr)_auto_auto_auto] items-center gap-x-4 px-4 py-2";
            return online ? (
              <button
                key={`${row.section}:${row.title}`}
                type="button"
                onClick={() => open(row.section)}
                className={cx(classes, "text-left hover:bg-fill-subtle")}
              >
                {cells}
              </button>
            ) : (
              <div key={`${row.section}:${row.title}`} className={classes}>
                {cells}
              </div>
            );
          })}
        </div>
      </div>
      <p className="mt-2 max-w-[70ch] px-4 text-caption text-fg-muted">
        Shared values are kept in each machine's own `settings.json`; a value set from {selfName()} for{" "}
        {machineName(machine)} takes effect there on its next launch. Credentials, accounts and local paths
        never leave the machine they belong to.
      </p>
    </>
  );
}
