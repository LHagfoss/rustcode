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

import { CircleHelp, Laptop, Monitor, RefreshCw, Search, SquareTerminal } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { currentPlatform } from "../lib/platform";
import { hasShell } from "../lib/shell";
import {
  askMachine,
  cancelPairing,
  codeCheckOverdue,
  codeComplete,
  codeDigits,
  type DiscoveredMachine,
  discoverMachines,
  lastSeen,
  type Machine,
  machineLine,
  type Pairing,
  pairMachine,
  platformName,
  refreshMachines,
  removeMachine,
  selfName,
  setMainMachine,
  stateLabel,
  stateTone,
  threadCount,
  threadLabel,
  useMachines,
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

export function MachinesPage() {
  const ready = useMachines((s) => s.ready);
  const error = useMachines((s) => s.error);
  const machines = useMachines((s) => s.machines);
  const counts = useMachines((s) => s.counts);
  const pairing = useMachines((s) => s.pairing);
  const pairingError = useMachines((s) => s.pairingError);
  const [now, setNow] = useState(() => Date.now());

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
          <MachineRow key={machine.id} machine={machine} count={threadCount(machine, counts)} now={now} />
        ))}
        {machines.length === 0 && <NoMachines />}
      </Group>

      {pairing ? <PairingGroup pairing={pairing} /> : <AddMachine error={pairingError} />}
    </>
  );
}

/** This app's own computer: the local RustCode server, over the local socket. */
function ThisComputer() {
  const status = useApp((s) => s.status);
  const open = useApp((s) => Object.values(s.threads).filter((thread) => thread.loaded).length);
  const platform = currentPlatform();
  const running = status === "ready";
  return (
    <Row
      title={
        <>
          <MachineIcon platform={platform === "mac" ? "macos" : platform} />
          {selfName()}
        </>
      }
      subtitle={`The RustCode server this app runs. ${open === 1 ? "One thread is open." : `${open} threads are open.`}`}
      badge={
        <Chip tone={running ? "ok" : "warn"}>
          {running ? "Connected" : status === "connecting" ? "Starting" : "Stopped"}
        </Chip>
      }
    />
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

function MachineRow({ machine, count, now }: { machine: Machine; count: number | undefined; now: number }) {
  const platform = platformName(machine.platform);
  const threads = threadLabel(count);
  return (
    <Row
      stacked
      title={
        <>
          <MachineIcon platform={machine.platform} />
          {machine.name}
        </>
      }
      subtitle={machineLine(machine)}
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

function AddMachine({ error }: { error: string | null }) {
  const [address, setAddress] = useState("");
  const discovered = useMachines((s) => s.discovered);
  const discovering = useMachines((s) => s.discovering);
  const discoverError = useMachines((s) => s.discoverError);
  const discover = useCallback(() => void discoverMachines(), []);
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
      title="Add machine"
      footer="Both computers need RustCode running with remote access on, and to be on the same network. A machine away from home is reached over Tailscale or a VPN by entering its address."
    >
      {error && <p className="m-0 px-4 py-4 text-callout text-warning">{error}</p>}
      <Row
        title="Machines on this network"
        subtitle="Found with Bonjour, the same way a phone finds this computer. A machine whose network is an overlay is not announced; enter its address instead."
      >
        <Action disabled={discovering} onClick={discover}>
          {discovering ? <Spinner /> : <Search size={13} />}
          {discovering ? "Looking…" : "Look for machines"}
        </Action>
      </Row>
      {discoverError && (
        <p className="m-0 px-4 py-3 text-callout text-warning leading-normal">
          Could not look for machines: {discoverError}
        </p>
      )}
      {(discovered ?? []).map((found) => (
        <DiscoveredRow key={found.gatewayId ?? found.name} found={found} />
      ))}
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

function DiscoveredRow({ found }: { found: DiscoveredMachine }) {
  return (
    <Row
      title={found.name}
      subtitle={found.address ? `At ${found.address}` : "It announced no address this computer can reach."}
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
