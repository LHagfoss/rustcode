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

import { useEffect, useState } from "react";
import type { Harness } from "../acp/types";
import { opError, opRunning, useOp } from "../lib/op";
import { currentPlatform } from "../lib/platform";
import { devicePlatform, deviceRoute, deviceStatus, machineAccess, versionLine } from "../state/deviceRows";
import { accountState } from "../state/harness";
import { addMachineRoute, removeMachineRoute, setMachineRouteOff } from "../state/machineRoutes";
import {
  defaultSelfName,
  disconnectMachine,
  LOCAL_MACHINE_ID,
  lastSeen,
  localAppVersion,
  type Machine,
  machineGlyph,
  machineLine,
  machineName,
  machineTint,
  probeLabel,
  probeMachineRoutes,
  probeTone,
  reconnectMachine,
  removeMachine,
  routeLabel,
  selfName,
  setLocalIdentity,
  setMachineDisplayName,
  setMachineIcon,
  setMachinePaused,
  useLocalIdentity,
  useMachines,
} from "../state/machines";
import { revokeRemoteDevice } from "../state/remoteAccess";
import { setFollowsThisComputer, useSettingsFollow } from "../state/settingsFollow";
import { SERVER_SETTINGS } from "../state/settingsRegistry";
import {
  checkMachineHarnesses,
  loadServerSettings,
  localServerSettingValues,
  serverValueDiffers,
  useMachineHarness,
  useServerValues,
  useSettingsScope,
} from "../state/settingsScope";
import { errorText, openSettings, toast, useApp } from "../state/store";
import { LOCAL_HOST } from "../state/threadKeys";
import { ArrowLeft, Plus, RefreshCw, Trash2, Unplug } from "./icons";
import { DeviceGlyphIcon } from "./MachineGlyph";
import { MachineIdentityEditor } from "./MachineProfile";
import { cx, noTextHelpers, Segments, Spinner, Switch, Tooltip } from "./primitives";
import { Action, Chip, ErrorDetails, fieldClass, Group, Row, SectionTitle } from "./settingsKit";
import { SYNC_COLUMNS, syncRows } from "./syncRows";

/** One machine's name, state and controls. */
export function MachineSettingsPage({ id }: { id: string }) {
  const machine = useMachines((state) => state.machines.find((entry) => entry.id === id));
  if (id === LOCAL_MACHINE_ID) return <LocalMachineSettings />;
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

function BackToMachines() {
  return (
    <div className="mb-3 flex items-center gap-2 px-4">
      <Action quiet size="sm" onClick={() => openSettings("machines")}>
        <ArrowLeft size={13} />
        Machines
      </Action>
    </div>
  );
}

function LocalMachineSettings() {
  const identity = useLocalIdentity();
  const platform = currentPlatform();
  return (
    <>
      <BackToMachines />
      <Group title={selfName()}>
        <Row title="Machine identity" subtitle="Change the name and icon shown for this computer.">
          <MachineIdentityEditor
            label={`${selfName()} on this computer`}
            reportedName={defaultSelfName()}
            platform={platform === "mac" ? "macos" : platform}
            identity={identity}
            onName={(name) => setLocalIdentity({ displayName: name })}
            onIcon={(icon, tint) => setLocalIdentity({ icon, tint })}
            trigger={<Action>Edit identity</Action>}
          />
        </Row>
        <Row title="Remote access" subtitle="Manage how other devices connect to this computer.">
          <Action onClick={() => openSettings("remote")}>Open Remote access</Action>
        </Row>
      </Group>
    </>
  );
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
      <BackToMachines />

      <Group title="Machine">
        <div className="flex flex-col gap-3 p-4">
          <div className="flex min-w-0 items-start gap-3">
            <span className="flex size-10 shrink-0 items-center justify-center rounded-xl bg-fill">
              <DeviceGlyphIcon glyph={machineGlyph(machine)} tint={machineTint(machine)} size={24} />
            </span>
            <div className="min-w-0 flex-1">
              <div className="flex min-w-0 flex-wrap items-center gap-2">
                <h3 className="min-w-0 truncate font-semibold text-title3 text-fg">{name}</h3>
                <Chip tone={status.tone}>{status.label}</Chip>
              </div>
              <div className="pt-0.5 text-callout text-fg-muted">
                {devicePlatform(machine.platform)}
                {machine.version && <span className="tabular"> · RustCode {machine.version}</span>}
                {machine.paused && " · Paused for new threads"}
              </div>
              <div className="pt-0.5 text-callout text-fg-muted" title={deviceRoute(machine)}>
                {deviceRoute(machine)}
                {machine.lastConnectedAt && !online && <> · Last seen {lastSeen(machine.lastConnectedAt)}</>}
              </div>
              {skew && <p className="m-0 pt-1 text-callout text-warning">{skew}</p>}
              {!online && machine.detail && (
                <details className="pt-1 text-callout text-fg-muted">
                  <summary className="w-fit cursor-pointer">Connection details</summary>
                  <p className="selectable mt-1 break-words">{machineLine(machine)}</p>
                </details>
              )}
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
              trigger={<Action size="sm">Edit identity</Action>}
            />
            <Action
              size="sm"
              disabled={machine.state === "connecting" || machine.state === "needsPairing"}
              onClick={() => reconnectMachine(machine)}
            >
              <RefreshCw size={12} />
              Reconnect
            </Action>
            <Action size="sm" disabled={!online} onClick={() => void disconnectMachine(machine)}>
              <Unplug size={12} />
              Disconnect
            </Action>
          </div>
        </div>
      </Group>

      <Group title="Controls">
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

      <Access machine={machine} />
      <Routes machine={machine} />
      <Group title="Remove machine">
        <Row
          title="Remove from this computer"
          subtitle="Deletes the saved pairing and routes. Its threads stay on the remote machine."
        >
          <Action
            danger
            onClick={() =>
              removeMachine(machine, () => {
                if (useApp.getState().settingsMachine === machine.id) openSettings("machines");
              })
            }
          >
            <Trash2 size={13} /> Remove machine
          </Action>
        </Row>
      </Group>
      <Harnesses machine={machine} />
      <WhatSyncs machine={machine} online={online} />
    </>
  );
}

/** Which computer may reach which: each direction on its own, with its own way to end it. */
function Access({ machine }: { machine: Machine }) {
  const devices = useApp((state) => state.remote?.devices);
  const access = machineAccess(machine, devices ?? []);
  return (
    <Group
      title={`Access · ${access.label}`}
      hint="The two directions are separate pairings. Ending one leaves the other as it is."
    >
      <Row title="This computer reaches it" subtitle={access.outboundText}>
        <Chip tone={access.outbound ? "ok" : "bad"}>{access.outbound ? "Allowed" : "Ended"}</Chip>
      </Row>
      <Row title="It reaches this computer" subtitle={access.inboundText}>
        <Chip tone={access.inbound ? "ok" : "idle"}>{access.inbound ? "Allowed" : "Not allowed"}</Chip>
        {access.inbound && (
          <Action size="sm" danger onClick={() => void revokeRemoteDevice(access.inbound!.id)}>
            Revoke
          </Action>
        )}
      </Row>
    </Group>
  );
}

/**
 * Every route saved for the machine with what the last test of it found, one button that tests
 * them all now, a switch per route, and Add route: SSH (any server this computer signs in to) or an
 * address (a NetBird or Tailscale one once that network is up on both computers).
 */
function Routes({ machine }: { machine: Machine }) {
  const probing = useMachines((state) => state.probe.status === "running");
  const [adding, setAdding] = useState(false);
  const inUse = machine.state === "connected" ? machine.route?.address : undefined;
  const on = machine.routes.filter((route) => !route.off).length;
  return (
    <Group
      title="Routes"
      action={
        <div className="flex items-center gap-1">
          <Action quiet size="sm" disabled={probing} onClick={() => void probeMachineRoutes(machine)}>
            <RefreshCw size={12} />
            {probing ? "Testing…" : "Test routes"}
          </Action>
          {!adding && (
            <Action quiet size="sm" onClick={() => setAdding(true)}>
              <Plus size={12} />
              Add route
            </Action>
          )}
        </div>
      }
      hint="The app tries the LAN first, then a VPN such as NetBird or Tailscale, then SSH, and keeps the first that answers. A route that is off is never tried. Test routes connects to each one without signing in. The machine reports its own addresses on every connection, so a NetBird or Tailscale address appears here once that network is up on both computers."
    >
      {machine.routes.length === 0 && !adding && (
        <Row title="No route saved" subtitle="Add one: SSH works with any server you can sign in to." />
      )}
      {machine.routes.map((route) => (
        <Row
          key={route.address}
          title={
            <span className="flex items-center gap-2">
              {routeLabel(route)}
              {inUse === route.address && <Chip tone="ok">In use</Chip>}
              {route.off && <Chip tone="idle">Off</Chip>}
            </span>
          }
          subtitle={
            route.probe?.detail ??
            (route.kind === "public" && !route.probe
              ? "Not tested: this app never sends a sign-in to a public address, so it never connects to one."
              : undefined)
          }
          status={
            <span className="tabular">
              {route.sshDestination
                ? `${route.sshDestination} › port ${route.sshGatewayPort ?? 17879}`
                : route.address}
              {route.probe && ` · tested ${lastSeen(route.probe.at)}`}
            </span>
          }
          badge={<Chip tone={probeTone(route.probe)}>{probeLabel(route.probe)}</Chip>}
        >
          <Switch
            label={`Use ${routeLabel(route)} at ${route.address}`}
            checked={!route.off}
            // The last route on stays on: the machine would have no way in.
            disabled={!route.off && on <= 1}
            onChange={(next) => void setMachineRouteOff(machine, route.address, !next)}
          />
          <Tooltip
            label={
              route.manual
                ? "Remove this route"
                : "Remove this route. The machine reports it again on its next connection; turn it off to keep it unused."
            }
          >
            <Action
              quiet
              size="sm"
              aria-label={`Remove ${route.address}`}
              disabled={!route.off && on <= 1}
              onClick={() => void removeMachineRoute(machine, route.address)}
            >
              <Trash2 size={12} />
            </Action>
          </Tooltip>
        </Row>
      ))}
      {adding && <AddRoute machine={machine} onDone={() => setAdding(false)} />}
    </Group>
  );
}

type RouteMethod = "ssh" | "address";
const ROUTE_METHODS: { value: RouteMethod; label: string }[] = [
  { value: "ssh", label: "SSH" },
  { value: "address", label: "Address" },
];

/** Add route: an SSH host from `~/.ssh/config` or `user@host`, or a network address. Tested when added. */
function AddRoute({ machine, onDone }: { machine: Machine; onDone: () => void }) {
  const [method, setMethod] = useState<RouteMethod>("ssh");
  const [value, setValue] = useState("");
  const [port, setPort] = useState("");
  const [adding, runAdd, resetAdd] = useOp();
  const busy = opRunning(adding);
  const error = opError(adding);
  const portNumber = Number.parseInt(port, 10);
  const add = async () => {
    if (busy || !value.trim()) return;
    const added = await runAdd(async () => {
      const failed = await addMachineRoute(
        machine,
        method === "ssh"
          ? { kind: "ssh", destination: value, gatewayPort: Number.isFinite(portNumber) ? portNumber : null }
          : { kind: "address", address: value },
      );
      if (failed) throw new Error(failed);
    });
    if (added.status === "done") onDone();
  };
  return (
    <div className="flex flex-col gap-2 px-3 py-3">
      <div className="flex items-center gap-2">
        <Segments
          value={method}
          options={ROUTE_METHODS}
          onChange={(next) => {
            setMethod(next);
            if (error) resetAdd();
          }}
          label="Route type"
        />
        <span className="text-caption text-fg-muted">
          {method === "ssh"
            ? "Through your own ssh, its config and agent. Nothing else to install."
            : "A LAN, NetBird or Tailscale address of this machine."}
        </span>
      </div>
      <div className="flex w-full flex-wrap items-center gap-2">
        <input
          autoFocus
          value={value}
          onChange={(event) => setValue(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") void add();
            if (event.key === "Escape") onDone();
          }}
          placeholder={
            method === "ssh" ? "lars@box or an alias from ~/.ssh/config" : "100.90.16.237 or box.local"
          }
          aria-label={method === "ssh" ? "SSH host" : "Address"}
          disabled={busy}
          {...noTextHelpers}
          className={cx(fieldClass, "min-w-0 flex-[2] font-mono")}
        />
        {method === "ssh" && (
          <input
            value={port}
            onChange={(event) => setPort(event.target.value.replace(/\D/g, "").slice(0, 5))}
            placeholder="17879"
            aria-label="RustCode's port there"
            disabled={busy}
            inputMode="numeric"
            {...noTextHelpers}
            className={cx(fieldClass, "w-20 font-mono")}
          />
        )}
        <Action prominent disabled={busy || !value.trim()} onClick={() => void add()}>
          {busy ? <Spinner /> : null}
          {busy ? "Testing…" : "Add"}
        </Action>
        <Action quiet disabled={busy} onClick={onDone}>
          Cancel
        </Action>
      </div>
      {error && (
        <p role="alert" className="m-0 text-callout text-danger leading-normal">
          {error}
        </p>
      )}
    </div>
  );
}

/** The harnesses that machine reports, what they need, and one Check for all of them. */
function Harnesses({ machine }: { machine: Machine }) {
  const harness = useMachineHarness(machine.id);
  const [checking, runCheck] = useOp<string>();
  const busy = opRunning(checking);
  const checked = busy ? null : (checking.data ?? null);
  const harnesses = harness.snapshot?.harnesses ?? [];
  const online = machine.state === "connected";
  const check = () =>
    runCheck(async () => {
      // One request for all of them: a harness that is slow to answer holds only itself.
      const lines = await checkMachineHarnesses(
        machine.id,
        harnesses.map((entry) => entry.id),
      );
      return (
        harnesses.map((entry) => `${entry.name}: ${lines[entry.id]}`).join(" · ") ||
        "The machine listed no harnesses."
      );
    });
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
        <Row
          title="Could not read the harnesses"
          subtitle={
            <ErrorDetails
              summary="That machine did not answer the request for its harnesses."
              detail={harness.error}
            />
          }
          badge={
            <Action quiet size="sm" disabled={harness.loading} onClick={harness.reload}>
              {harness.loading ? "Retrying…" : "Retry"}
            </Action>
          }
        />
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
/** One switch: this machine takes this computer's shared settings and keeps them (state/settingsFollow.ts). */
function FollowSettings({ machine, online }: { machine: Machine; online: boolean }) {
  const follows = useSettingsFollow((state) => state.ids.includes(machine.id));
  const name = machineName(machine);
  const here = selfName();
  const toggle = (next: boolean) => {
    void setFollowsThisComputer(machine.id, next).then(
      (count) => {
        if (next && online)
          toast(
            count
              ? `Copied ${count} ${count === 1 ? "setting" : "settings"} from ${here} to ${name}.`
              : `${name} already has ${here}'s settings.`,
            "success",
          );
      },
      (error) => toast(errorText(error), "error"),
    );
  };
  return (
    <Group title="Settings">
      <Row
        title={`Use ${here}'s settings`}
        subtitle={
          follows
            ? `${name} uses ${here}'s shared settings. A change on ${name} itself is put back when it next connects.`
            : `${name} keeps its own shared settings. Turn this on to copy ${here}'s to it now and keep them the same.`
        }
      >
        <Switch label={`Use ${here}'s settings on ${name}`} checked={follows} onChange={toggle} />
      </Row>
    </Group>
  );
}

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
      <FollowSettings machine={machine} online={online} />
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
