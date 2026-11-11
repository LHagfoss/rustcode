// Settings › Machines: where this app's threads can run.
//
// This computer first — the RustCode server the app starts itself — then every
// other computer the app is paired with, each with the route it is reached
// over, how long ago it answered and how many threads it has. "Add machine"
// asks one to let this app in: the request appears on that machine, its owner
// accepts, and the eight-digit code it then shows is typed here.
//
// The app's Rust shell holds each machine's connection supervisor (an app
// cannot dial a gateway from the web view), so this page only asks, lists and
// explains; `state/machines.ts` keeps what it is told.
//
// This computer's row also says whether the RustCode server is installed as a
// background service — the server that keeps this computer reachable with the
// app closed, and that Settings › Remote access installs when network access is
// turned on. Threads this app runs stay in the app and end with it.
// A row of another machine says which RustCode version it runs, so an old one
// is a sentence with a command, not a silent connection failure.

import { CircleHelp, Laptop, Monitor, RefreshCw, Search, Server, SquareTerminal } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { currentPlatform } from "../lib/platform";
import { hasShell } from "../lib/shell";
import {
  askInstallServer,
  askMachine,
  cancelPairing,
  codeCheckOverdue,
  codeComplete,
  codeDigits,
  type DiscoveredMachine,
  discoverMachines,
  gatewayLine,
  lastSeen,
  localAppVersion,
  type Machine,
  machineLine,
  type Pairing,
  pairMachine,
  platformName,
  refreshMachines,
  refreshServerService,
  removeMachine,
  routeName,
  serverLine,
  setMainMachine,
  stateLabel,
  stateTone,
  threadCount,
  threadLabel,
  useMachines,
  versionSkew,
} from "../state/machines";
import { toast, useApp } from "../state/store";
import { cx, noTextHelpers, Spinner, Switch } from "./primitives";
import { Action, Chip, fieldClass, Group, Loading, Row } from "./settingsKit";

/** How often the page asks the shell where the machines stand. */
const POLL_MS = 3000;

/** A machine's icon by what it runs: a laptop, a monitor, or a terminal. */
function MachineIcon({ platform }: { platform: string }) {
  const Icon =
    platform === "macos"
      ? Laptop
      : platform === "windows"
        ? Monitor
        : platform === "linux"
          ? SquareTerminal
          : CircleHelp;
  return <Icon size={16} className="shrink-0 text-fg-muted" aria-hidden />;
}

/** This computer's name as people say it (docs/terminology.md). */
function selfName(): string {
  switch (currentPlatform()) {
    case "mac":
      return "This Mac";
    case "windows":
      return "This PC";
    default:
      return "This computer";
  }
}

export function MachinesPage() {
  const ready = useMachines((s) => s.ready);
  const error = useMachines((s) => s.error);
  const machines = useMachines((s) => s.machines);
  const counts = useMachines((s) => s.counts);
  const pairing = useMachines((s) => s.pairing);
  const pairingError = useMachines((s) => s.pairingError);
  const [now, setNow] = useState(() => Date.now());
  const [local, setLocal] = useState<string | null>(null);

  // The shell learns about a machine on its own (a retry that worked, a
  // pairing ended there), so the page keeps asking while it is open.
  useEffect(() => {
    if (!hasShell()) return;
    void refreshMachines();
    const timer = setInterval(() => void refreshMachines(), POLL_MS);
    return () => clearInterval(timer);
  }, []);

  // Countdowns: "retrying in 4 s", and the code's two minutes.
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);

  // The background server and this app's own version: both read once, and
  // again after an install.
  useEffect(() => {
    void refreshServerService();
    void localAppVersion().then(setLocal);
  }, []);

  if (!ready) return <Loading error={error} />;

  return (
    <>
      {error && <p className="mb-6 text-callout text-warning">Could not read the machines: {error}</p>}

      <Group
        title="Machines"
        footer="A thread stays on the computer that runs it. A LAN route is authenticated but not encrypted; Tailscale and VPN tunnels encrypt what they carry. Removing a machine forgets its pairing here; the machine keeps listing this computer until you revoke it there."
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
        {machines.length === 0 && <NoMachines />}
      </Group>

      {pairing ? <PairingGroup pairing={pairing} /> : <AddMachine error={pairingError} local={local} />}
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
  const platform = currentPlatform();
  const running = status === "ready";
  const installed = server?.installed === true;
  return (
    <Row
      title={
        <>
          <MachineIcon platform={platform === "mac" ? "macos" : platform} />
          {selfName()}
        </>
      }
      subtitle={`The RustCode server this app runs. ${open === 1 ? "One thread is open." : `${open} threads are open.`}${
        installed ? "" : " It runs only while the app is open, so closed threads stop with it."
      }`}
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
            <span className="block pt-0.5">Gateway: {gatewayLine(server.gateway)}</span>
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
          {running ? "Connected" : status === "connecting" ? "Starting" : "Stopped"}
        </Chip>
      }
    >
      {server && !installed && (
        <Action disabled={installing} onClick={() => askInstallServer()}>
          {installing ? <Spinner /> : <Server size={13} />}
          Install server
        </Action>
      )}
    </Row>
  );
}

function NoMachines() {
  return (
    <Row
      title="No other computers yet"
      subtitle="Add a computer running RustCode below to see its threads here, and to start threads on it."
    />
  );
}

function MachineRow({
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
  const hint = machine.hint;
  return (
    <Row
      stacked
      title={
        <>
          <MachineIcon platform={machine.platform} />
          {machine.name}
        </>
      }
      subtitle={
        <>
          <span className="block">{machineLine(machine)}</span>
          {/* Who is older, and what updates it: the Mac Studio on 0.96 against a 1.28 app. */}
          {skew && <span className="mt-1 block text-warning">{skew}</span>}
          {/* PR 3402's words for a route that does not answer while another does. */}
          {hint && <span className="mt-1 block">{hint}</span>}
        </>
      }
      status={
        <>
          {machine.main && "Main machine · "}
          {platform && `${platform} · `}
          {/* The state is the chip; this line says when and how much, not what. */}
          {machine.lastConnectedAt
            ? `Last seen ${lastSeen(machine.lastConnectedAt, now)}`
            : "Not reached yet"}
          {threads && ` · ${threads}`}
        </>
      }
      badge={<Chip tone={stateTone(machine)}>{stateLabel(machine)}</Chip>}
    >
      {machine.state === "needsPairing" && <PairAgainAction machine={machine} />}
      <span className="flex items-center gap-2">
        <span className="text-caption text-fg-muted">Main machine</span>
        <Switch
          label={`Make ${machine.name} the main machine`}
          checked={machine.main}
          onChange={(next) => next && void setMainMachine(machine)}
        />
      </span>
      <Action quiet size="sm" aria-label={`Remove ${machine.name}`} onClick={() => removeMachine(machine)}>
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
        if (address) void askMachine(address, machine.name);
      }}
    >
      <RefreshCw size={12} /> Pair again
    </Action>
  );
}

// ---------------------------------------------------------------- adding

function AddMachine({ error, local }: { error: string | null; local: string | null }) {
  const [address, setAddress] = useState("");
  const discovered = useMachines((s) => s.discovered);
  const discovering = useMachines((s) => s.discovering);
  const discoverError = useMachines((s) => s.discoverError);
  const discover = useCallback(() => void discoverMachines(), []);
  // The list is what the sheet is for, so it fills itself: Bonjour on this
  // network and the overlay addresses the saved machines reported.
  useEffect(() => {
    if (discovered === null) void discoverMachines();
  }, [discovered]);
  const ask = () => {
    const target = address.trim();
    if (!target) {
      toast("Enter the address of the computer to add.", "error");
      return;
    }
    setAddress("");
    void askMachine(target);
  };
  return (
    <Group
      title="Machines found"
      action={
        <Action quiet disabled={discovering} onClick={discover}>
          {discovering ? <Spinner /> : <Search size={13} />}
          {discovering ? "Looking…" : "Look again"}
        </Action>
      }
      footer="Found automatically: Bonjour, the same way a phone finds this computer, on this network; and a saved machine's NetBird, Tailscale or ZeroTier address, which its own handshake reported and no browse can hear. Both computers need RustCode running with remote access on. A machine away from home is reached over an overlay or a VPN; entering its address is the fallback."
    >
      {error && <p className="m-0 px-4 py-4 text-callout text-warning">{error}</p>}
      {discoverError && (
        <p className="m-0 px-4 py-3 text-callout text-warning leading-normal">{discoverError}</p>
      )}
      {(discovered ?? []).map((found) => (
        <DiscoveredRow key={found.gatewayId ?? found.name} found={found} local={local} />
      ))}
      {discovered?.length === 0 && (
        <Row
          title="Nothing found yet"
          subtitle="A machine on this network appears once it announces itself. One on another network appears here after it has been paired once, or by entering its address below."
        />
      )}
      <Row
        stacked
        title="Enter an address"
        subtitle="The address this computer reaches it on: an IP address or name, with the gateway's port when it is not 17879. An address on the public internet is refused: the gateway does not encrypt, so the pairing code and the token would travel in the clear."
      >
        <div className="flex w-full items-center gap-2">
          <input
            value={address}
            onChange={(event) => setAddress(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") ask();
            }}
            placeholder="192.168.1.20 or studio.local:17879"
            aria-label="Machine address"
            {...noTextHelpers}
            className={cx(fieldClass, "min-w-0 flex-1 font-mono")}
          />
          <Action onClick={ask}>Ask to connect</Action>
        </div>
      </Row>
    </Group>
  );
}

/**
 * One found machine, whichever way it was found: its name, then what this app
 * knows of it — platform, the RustCode version it runs and the network its
 * address is on — with the address. An older version is a sentence with the
 * command that updates it there, and a route that does not answer carries
 * PR 3402's hint.
 */
function DiscoveredRow({ found, local }: { found: DiscoveredMachine; local: string | null }) {
  const platform = found.platform ? platformName(found.platform) : undefined;
  const facts = [
    platform,
    found.version ? `RustCode ${found.version}` : undefined,
    routeName(null, found.routeKind),
  ];
  const skew = versionSkew(found.version, local, found.platform);
  return (
    <Row
      title={
        <>
          {found.platform && <MachineIcon platform={found.platform} />}
          {found.name}
        </>
      }
      subtitle={
        <>
          <span className="block">{facts.filter(Boolean).join(" · ")}</span>
          <span className="block font-mono">
            {found.address ?? "It announced no address this computer can reach."}
          </span>
          {skew && <span className="mt-1 block text-warning">{skew}</span>}
          {found.hint && <span className="mt-1 block">{found.hint}</span>}
        </>
      }
    >
      {found.saved ? (
        <Chip tone="ok">Saved</Chip>
      ) : (
        <Action
          disabled={!found.address}
          onClick={() => {
            if (found.address) void askMachine(found.address, found.name);
          }}
        >
          Ask to connect
        </Action>
      )}
    </Row>
  );
}

// ---------------------------------------------------------------- pairing

function PairingGroup({ pairing }: { pairing: Pairing }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  const overdue = codeCheckOverdue(pairing, now);
  return (
    <Group title={pairingTitle(pairing, overdue)}>
      {pairing.stage === "waiting" && (
        <Row
          title={`Waiting for ${pairing.host} to accept on that machine…`}
          subtitle="The request is on that machine's screen. Accept it there, and the code to type appears here. Deny it there to stop, or wait two minutes."
        >
          <Spinner />
          <Action onClick={cancelPairing}>Cancel</Action>
        </Row>
      )}
      {pairing.stage === "pairing" &&
        (overdue ? (
          <Row
            title={`${pairing.host} did not answer`}
            subtitle="The code was sent, and nothing came back. If that machine accepted it, it now lists this computer under its paired devices: pair again, or remove this computer there first."
            status="Nothing was saved on this computer. Ask again to start over."
          >
            <Action onClick={() => void askMachine(pairing.address, pairing.host)}>Ask again</Action>
            <Action quiet onClick={cancelPairing}>
              Cancel
            </Action>
          </Row>
        ) : (
          <Row title={`Pairing with ${pairing.host}…`} subtitle="Checking the code with that machine.">
            <Spinner />
            {/* Every stage has a way out: a shell that stopped answering is not a reason to reload the app. */}
            <Action onClick={cancelPairing}>Cancel</Action>
          </Row>
        ))}
      {pairing.stage === "code" && (
        <CodeRow host={pairing.host} length={pairing.codeLength} expiresAt={pairing.expiresAt} />
      )}
    </Group>
  );
}

/** The section's name: what this stage is doing, in the words of whoever must act. */
function pairingTitle(pairing: Pairing, overdue: boolean): string {
  switch (pairing.stage) {
    case "waiting":
      return "Waiting for approval";
    case "pairing":
      return overdue ? "No answer" : "Checking the code";
    case "code":
      return "Enter the code";
  }
}

/** The eight-digit code the machine shows, typed in its own boxes. */
function CodeRow({ host, length, expiresAt }: { host: string; length: number; expiresAt: number }) {
  const [code, setCode] = useState("");
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  const left = Math.max(0, Math.ceil((expiresAt - now) / 1000));
  const complete = codeComplete(code, length) && left > 0;
  const send = () => {
    if (complete) void pairMachine(code);
  };
  return (
    <div className="px-4 py-4">
      <div className="font-medium text-body text-fg">The code {host} shows</div>
      <p className="m-0 max-w-[52ch] pt-1 text-callout text-fg-muted leading-normal">
        {length} digits, on that machine's screen now. It works once and{" "}
        {left > 0 ? (
          <span className="tabular">
            expires in {Math.floor(left / 60)}:{String(left % 60).padStart(2, "0")}.
          </span>
        ) : (
          "has expired; ask again for a new one."
        )}
      </p>
      <div className="flex flex-wrap items-center gap-3 pt-3">
        <CodeInput length={length} value={code} onChange={setCode} onComplete={send} />
        <Action prominent disabled={!complete} onClick={send}>
          Connect
        </Action>
        <Action quiet onClick={cancelPairing}>
          Cancel
        </Action>
      </div>
    </div>
  );
}

/**
 * One box per digit, each advancing to the next as it is typed (Backspace goes
 * back), a pasted code filling the rest. The boxes are the field shape every
 * Settings row uses, so the code reads as one control.
 */
export function CodeInput({
  length,
  value,
  onChange,
  onComplete,
  disabled,
}: {
  length: number;
  value: string;
  onChange: (code: string) => void;
  /** Called once the last digit is in, so Enter-free entry can pair at once. */
  onComplete?: (code: string) => void;
  disabled?: boolean;
}) {
  const boxes = useRef<(HTMLInputElement | null)[]>([]);

  useEffect(() => {
    boxes.current[0]?.focus();
  }, []);

  const focus = (index: number) => {
    const box = boxes.current[Math.max(0, Math.min(index, length - 1))];
    box?.focus();
    box?.select();
  };

  const write = (index: number, typed: string) => {
    const digits = codeDigits(typed, length);
    const next = codeDigits(value.slice(0, index) + digits + value.slice(index + digits.length), length);
    onChange(next);
    const landed = index + Math.max(1, digits.length);
    focus(landed);
    if (onComplete && next.length === length) onComplete(next);
  };

  return (
    <div role="group" aria-label={`${length}-digit code`} className="flex items-center gap-1.5">
      {Array.from({ length }, (_, index) => (
        <input
          key={index}
          ref={(box) => {
            boxes.current[index] = box;
          }}
          value={value[index] ?? ""}
          disabled={disabled}
          inputMode="numeric"
          autoComplete="one-time-code"
          aria-label={`Digit ${index + 1} of ${length}`}
          maxLength={length}
          onChange={(event) => write(index, event.target.value)}
          onPaste={(event) => {
            event.preventDefault();
            write(index, event.clipboardData.getData("text"));
          }}
          onKeyDown={(event) => {
            if (event.key === "Backspace") {
              event.preventDefault();
              const keep = value.slice(0, index) + value.slice(index + 1);
              if (value[index]) onChange(keep);
              else {
                onChange(value.slice(0, index - 1) + value.slice(index));
                focus(index - 1);
              }
            } else if (event.key === "ArrowLeft") {
              event.preventDefault();
              focus(index - 1);
            } else if (event.key === "ArrowRight") {
              event.preventDefault();
              focus(index + 1);
            } else if (event.key === "Enter" && value.length === length) {
              onComplete?.(value);
            }
          }}
          className={cx(fieldClass, "size-9 px-0 text-center font-mono text-title3")}
        />
      ))}
    </div>
  );
}
