// Add environment: one dialog for every way to bring another computer in (docs/desktop.md ›
// Several machines), after T3 Code's "Add Environment". A method switch at the top (connect code,
// SSH, an address on the same network or Tailscale); under it the method's own fields; once it
// runs, its steps as they go; and when a step fails, a plain message with the next thing to
// try and a button that tries again with what was typed. When the environment shows up in the
// list, the dialog says so and closes on Done.
//
// The work is `state/machines.ts`'s (the shell holds every connection); this file only asks and
// shows. `ui/environments.ts` holds the words.

import { Check, Copy, KeyRound, Minus, Search, X } from "lucide-react";
import { Dialog } from "radix-ui";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  askMachine,
  type ConnectOffer,
  cancelConnectCode,
  cancelPairing,
  clearConnectError,
  codeCheckOverdue,
  codeComplete,
  codeDigits,
  connectOverSsh,
  connectWithCode,
  createConnectCode,
  type DiscoveredMachine,
  discoverMachines,
  dismissSsh,
  followConnectCode,
  type Machine,
  machineName,
  type Pairing,
  pairMachine,
  platformIcon,
  platformName,
  routeName,
  useMachines,
  versionSkew,
} from "../state/machines";
import { toast } from "../state/store";
import {
  ADD_METHODS,
  type AddMethod,
  addFailure,
  addressSteps,
  codeSteps,
  environmentIds,
  newEnvironment,
  type ProgressStep,
  sshProgress,
} from "./environments";
import { DeviceGlyphIcon } from "./MachineGlyph";
import { Callout, copyText, cx, noTextHelpers, Segments, Spinner } from "./primitives";
import { Action, Chip, fieldClass } from "./settingsKit";

/** How often an open connect code asks the gateway whether a computer used it. */
const POLL_MS = 3000;

export function AddEnvironmentDialog({
  open,
  onOpenChange,
  local,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** This app's version, for the found list's "older version" lines. */
  local: string | null;
}) {
  const machines = useMachines((s) => s.machines);
  const [method, setMethod] = useState<AddMethod>("code");
  // The environments there were when the dialog opened: the one that is not among them is the new one.
  const before = useRef<Set<string>>(new Set());
  const [added, setAdded] = useState<Machine | null>(null);
  const machinesNow = useRef(machines);
  machinesNow.current = machines;

  useEffect(() => {
    if (!open) return;
    before.current = environmentIds(machinesNow.current);
    setAdded(null);
    clearConnectError();
    // Reopened on a request already running (or Pair again): land where it is.
    const state = useMachines.getState();
    if (state.pairing) setMethod("address");
    else if (state.ssh?.running) setMethod("ssh");
  }, [open]);

  useEffect(() => {
    if (!open || added) return;
    const fresh = newEnvironment(machines, before.current);
    if (fresh) setAdded(fresh);
  }, [machines, open, added]);

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center">
          <Dialog.Content
            aria-describedby="add-environment-description"
            className="elevated pointer-events-auto flex max-h-[min(680px,calc(100vh-48px))] w-[min(560px,calc(100vw-48px))] flex-col gap-4 overflow-y-auto p-5"
          >
            <div className="flex items-start gap-3">
              <div className="min-w-0 flex-1">
                <Dialog.Title className="font-semibold text-title3 text-fg">Add environment</Dialog.Title>
                <Dialog.Description
                  id="add-environment-description"
                  className="m-0 pt-1 text-callout text-fg-muted"
                >
                  Another computer running RustCode.
                </Dialog.Description>
              </div>
              <Dialog.Close asChild>
                <Action quiet size="sm" aria-label="Close">
                  <X size={13} />
                </Action>
              </Dialog.Close>
            </div>

            {added ? (
              <Added machine={added} onDone={() => onOpenChange(false)} />
            ) : (
              <>
                <div className="flex">
                  <Segments value={method} options={ADD_METHODS} onChange={setMethod} label="Connect with" />
                </div>
                {method === "code" && <CodePane />}
                {method === "ssh" && <SshPane />}
                {method === "address" && <AddressPane local={local} />}
              </>
            )}
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** The environment is saved: say so, and what it can do now. */
function Added({ machine, onDone }: { machine: Machine; onDone: () => void }) {
  return (
    <div className="flex flex-col gap-3" data-testid="environment-added">
      <div className="flex items-center gap-3 rounded-xl bg-surface px-4 py-3">
        <DeviceGlyphIcon glyph={platformIcon(machine.platform)} size={18} />
        <div className="min-w-0 flex-1">
          <div className="truncate font-medium text-body text-fg">{machineName(machine)}</div>
          <div className="text-callout text-fg-muted">
            Added. Choose it under Run on when you start a thread.
          </div>
        </div>
        <Chip tone="ok">{machine.state === "connected" ? "Online" : "Saved"}</Chip>
      </div>
      <div className="flex justify-end">
        <Action prominent onClick={onDone}>
          Done
        </Action>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- shared pieces

/** A method's steps, one line each: done, running, failed or still to do. */
export function ProgressSteps({ steps, label }: { steps: ProgressStep[]; label: string }) {
  if (steps.length === 0) return null;
  return (
    <ol className="m-0 flex list-none flex-col gap-1.5 p-0" aria-label={label}>
      {steps.map((step) => (
        <li key={step.key} className="flex items-start gap-2 text-callout" data-state={step.state}>
          <span className="mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center">
            {step.state === "running" ? (
              <Spinner />
            ) : step.state === "done" ? (
              <Check size={13} className="text-success" />
            ) : step.state === "failed" ? (
              <X size={13} className="text-warning" />
            ) : (
              <Minus size={13} className="text-fg-muted" />
            )}
          </span>
          <span className="min-w-0">
            <span className={step.state === "todo" || step.state === "skipped" ? "text-fg-muted" : "text-fg"}>
              {step.label}
              {step.state === "skipped" ? " · not needed" : ""}
            </span>
            {step.detail ? <span className="block text-fg-muted leading-normal">{step.detail}</span> : null}
          </span>
        </li>
      ))}
    </ol>
  );
}

/** What went wrong, what to try, and the button that tries again with what was typed. */
function Failure({
  method,
  error,
  onRetry,
  onClose,
}: {
  method: AddMethod;
  error: string;
  onRetry: () => void;
  onClose?: () => void;
}) {
  const failure = addFailure(method, error);
  return (
    <Callout
      tone="warning"
      action={
        <>
          <Action size="sm" onClick={onRetry}>
            Try again
          </Action>
          {onClose && (
            <Action quiet size="sm" onClick={onClose}>
              Close
            </Action>
          )}
        </>
      }
    >
      <span className="block font-medium text-fg">{failure.title}</span>
      <span className="block pt-0.5">{failure.detail}</span>
      <span className="block pt-0.5">{failure.hint}</span>
    </Callout>
  );
}

function Field({ title, hint, children }: { title: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-2">
      <div>
        <div className="font-medium text-body text-fg">{title}</div>
        {hint && <p className="m-0 pt-0.5 text-callout text-fg-muted leading-normal">{hint}</p>}
      </div>
      {children}
    </div>
  );
}

// ---------------------------------------------------------------- connect code

/**
 * Pair two computers by carrying one string: paste one created there, or create one here for
 * the other. No request to accept; the code carries the address when it can.
 */
function CodePane() {
  const offer = useMachines((s) => s.connectOffer);
  const busy = useMachines((s) => s.connectBusy);
  const error = useMachines((s) => s.connectError);
  const [code, setCode] = useState("");
  const [address, setAddress] = useState("");
  // The address is for a short code that carries none, so it is asked for only on request.
  const [withAddress, setWithAddress] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const open = offer !== null && !offer.pairedWith && !offer.usedUp && offer.expiresAt > now;

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  useEffect(() => {
    if (!open) return;
    const timer = setInterval(() => void followConnectCode(), POLL_MS);
    return () => clearInterval(timer);
  }, [open]);

  const connect = () => {
    if (busy || !code.trim()) return;
    void connectWithCode(code, address).then((paired) => {
      if (!paired) return;
      setCode("");
      setAddress("");
      setWithAddress(false);
    });
  };
  const connecting = busy && !offer;
  return (
    <div className="flex flex-col gap-4">
      <Field title="I have a code" hint="Created on the other computer.">
        <div className="flex w-full flex-wrap items-center gap-2">
          <input
            value={code}
            onChange={(event) => setCode(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") connect();
            }}
            placeholder="Code or link"
            aria-label="Connect code"
            disabled={connecting}
            {...noTextHelpers}
            className={cx(fieldClass, "min-w-0 flex-1 font-mono")}
          />
          {withAddress || address ? (
            <input
              value={address}
              onChange={(event) => setAddress(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") connect();
              }}
              placeholder="Address"
              aria-label="Address of the other computer"
              disabled={connecting}
              {...noTextHelpers}
              className={cx(fieldClass, "w-40 min-w-0 font-mono")}
            />
          ) : (
            <Action quiet disabled={connecting} onClick={() => setWithAddress(true)}>
              Add address
            </Action>
          )}
          <Action prominent disabled={busy || !code.trim()} onClick={connect}>
            {connecting ? <Spinner /> : null}
            Connect
          </Action>
        </div>
        {connecting && <ProgressSteps steps={codeSteps(true, false)} label="Connect code progress" />}
        {error && !busy && (
          <Failure method="code" error={error} onRetry={connect} onClose={clearConnectError} />
        )}
      </Field>

      <div className="border-border border-t pt-4">
        <Field title="Let another computer in" hint="Shows a code that works once, for ten minutes.">
          {offer ? (
            <OfferView offer={offer} now={now} />
          ) : (
            <div>
              <Action disabled={busy} onClick={() => void createConnectCode()}>
                {busy ? <Spinner /> : <KeyRound size={13} />}
                Create connect code
              </Action>
            </div>
          )}
        </Field>
      </div>
    </div>
  );
}

/** The code this computer shows: copy it, see how long it lasts, and who used it. */
function OfferView({ offer, now }: { offer: ConnectOffer; now: number }) {
  const left = Math.max(0, Math.ceil((offer.expiresAt - now) / 1000));
  const countdown = `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}`;
  if (offer.pairedWith) {
    return (
      <div className="flex flex-wrap items-center gap-3 rounded-xl bg-surface px-4 py-3">
        <div className="min-w-0 flex-1">
          <div className="font-medium text-body text-fg">Paired with {offer.pairedWith}</div>
          <div className="text-callout text-fg-muted">
            It now sees and runs this computer's threads. Remove it under Settings › Remote access › Devices
            to take that back.
          </div>
        </div>
        <Chip tone="ok">Used</Chip>
        <Action onClick={() => void cancelConnectCode()}>Done</Action>
      </div>
    );
  }
  if (offer.usedUp || left === 0) {
    return (
      <div className="flex flex-wrap items-center gap-3 rounded-xl bg-surface px-4 py-3">
        <div className="min-w-0 flex-1">
          <div className="font-medium text-body text-fg">
            {offer.usedUp ? "The code was refused too often" : "The code has expired"}
          </div>
          <div className="text-callout text-fg-muted">
            Nothing was paired with it. Create a new one when the other computer is ready.
          </div>
        </div>
        <Action onClick={() => void createConnectCode()}>Create a new code</Action>
        <Action quiet onClick={() => void cancelConnectCode()}>
          Close
        </Action>
      </div>
    );
  }
  const where = offer.addressInCode
    ? `It carries ${offer.address}; the link carries every address this computer answers on.`
    : `It carries no address: type ${offer.address} next to it on the other computer, or paste the link instead.`;
  return (
    <div className="flex flex-col gap-2 rounded-xl bg-surface px-4 py-3">
      <div className="flex flex-wrap items-center gap-3">
        <code className="select-all font-mono text-title text-fg tabular tracking-wider">{offer.code}</code>
        <Action size="sm" onClick={() => void copyText(offer.code)}>
          <Copy size={12} /> Copy
        </Action>
        <Action quiet size="sm" onClick={() => void copyText(offer.link)}>
          Copy link
        </Action>
      </div>
      <p className="m-0 text-callout text-fg-muted leading-normal">
        On the other computer: Add environment › Connect code, or{" "}
        <code className="font-mono">rustcode machines connect {offer.code}</code>. {where}
      </p>
      {offer.loopbackOnly && (
        <p className="m-0 text-callout text-warning leading-normal">
          Remote access listens on this computer only, so no other computer can reach it.
        </p>
      )}
      <div className="flex flex-wrap items-center gap-3 text-callout text-fg-muted">
        <Spinner />
        <span className="tabular">Waiting for a computer · expires in {countdown}</span>
        <Action quiet size="sm" onClick={() => void cancelConnectCode()}>
          Cancel code
        </Action>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- SSH

/**
 * `user@host` or an alias from `~/.ssh/config`. The native side signs in without prompting (the
 * key must be in the agent), installs RustCode there at this version when allowed and needed, runs
 * `rustcode up` there and pairs with the code it printed, which never leaves SSH.
 */
function SshPane() {
  const run = useMachines((s) => s.ssh);
  const error = useMachines((s) => s.sshError);
  const [target, setTarget] = useState("");
  const [allowInstall, setAllowInstall] = useState(true);
  const running = run?.running === true;
  const connect = () => {
    if (running || !target.trim()) return;
    void connectOverSsh(target, allowInstall).then((paired) => {
      if (paired) setTarget("");
    });
  };
  return (
    <div className="flex flex-col gap-3">
      <Field
        title="A computer you reach with ssh"
        hint="A Linux box, a VPS, a Mac or a Windows PC with OpenSSH. Installs RustCode there if it is missing or older than this app, runs its server in the background and pairs the two. Your own ssh, its config and its agent sign in; the pairing secret travels inside SSH only."
      >
        <div className="flex w-full flex-wrap items-center gap-2">
          <input
            value={target}
            onChange={(event) => setTarget(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") connect();
            }}
            placeholder="lars@box or an alias from ~/.ssh/config"
            aria-label="SSH host"
            disabled={running}
            {...noTextHelpers}
            className={cx(fieldClass, "min-w-0 flex-[2] font-mono")}
          />
          <Action prominent disabled={running || !target.trim()} onClick={connect}>
            {running ? <Spinner /> : null}
            Connect
          </Action>
        </div>
        <label className="flex items-center gap-2 text-callout text-fg-muted">
          <input
            type="checkbox"
            checked={allowInstall}
            disabled={running}
            onChange={(event) => setAllowInstall(event.target.checked)}
          />
          Install or update RustCode there when needed (in ~/.rustcode/bin, with a background service)
        </label>
      </Field>
      {run && <ProgressSteps steps={sshProgress(run.steps, running)} label="Connect over SSH progress" />}
      {error && !running && <Failure method="ssh" error={error} onRetry={connect} onClose={dismissSsh} />}
      <p className="m-0 text-caption text-fg-muted leading-normal">
        A key with a passphrase must be in the agent (ssh-add), and a new host key must be accepted once in a
        terminal. The two computers still need a network between them for RustCode itself (the same LAN, or
        NetBird or Tailscale).
      </p>
    </div>
  );
}

// ---------------------------------------------------------------- an address

/**
 * An address on this network or a tailnet: found automatically (Bonjour, and the overlay
 * addresses saved environments reported) or typed. Asking puts a request on that computer; its
 * owner accepts and the code it then shows is typed here.
 */
function AddressPane({ local }: { local: string | null }) {
  const pairing = useMachines((s) => s.pairing);
  const pairingError = useMachines((s) => s.pairingError);
  const discovered = useMachines((s) => s.discovered);
  const discovering = useMachines((s) => s.discovering);
  const discoverError = useMachines((s) => s.discoverError);
  const [address, setAddress] = useState("");
  // What was asked last, so Try again repeats it after the field was cleared.
  const [asked, setAsked] = useState<{ address: string; name?: string } | null>(null);
  const discover = useCallback(() => void discoverMachines(), []);

  useEffect(() => {
    if (discovered === null) void discoverMachines();
  }, [discovered]);

  const ask = (target: string, name?: string) => {
    const typed = target.trim();
    if (!typed) {
      toast("Enter the address of the computer to add.", "error");
      return;
    }
    setAsked({ address: typed, name });
    setAddress("");
    void askMachine(typed, name);
  };

  if (pairing) return <PairingView pairing={pairing} />;
  return (
    <div className="flex flex-col gap-4">
      {pairingError && (
        <Failure
          method="address"
          error={pairingError}
          onRetry={() => asked && ask(asked.address, asked.name)}
        />
      )}
      <Field
        title="Found on this network"
        hint="Bonjour finds a computer on this network; a saved environment's NetBird, Tailscale or ZeroTier address appears once it has been paired. Both computers need RustCode running with remote access on."
      >
        <div className="flex flex-col divide-y divide-border overflow-hidden rounded-xl bg-surface">
          {(discovered ?? []).map((found) => (
            <FoundRow
              key={found.gatewayId ?? found.name}
              found={found}
              local={local}
              onAsk={() => found.address && ask(found.address, found.name)}
            />
          ))}
          {discovered?.length === 0 && (
            <div className="px-4 py-3 text-callout text-fg-muted leading-normal">
              Nothing found yet. A computer on another network appears here after it has been paired once, or
              by entering its address below.
            </div>
          )}
          {discovered === null && !discoverError && (
            <div className="flex items-center gap-2 px-4 py-3 text-callout text-fg-muted">
              <Spinner /> Looking…
            </div>
          )}
          {discoverError && (
            <div className="px-4 py-3 text-callout text-warning leading-normal">{discoverError}</div>
          )}
        </div>
        <div>
          <Action quiet disabled={discovering} onClick={discover}>
            {discovering ? <Spinner /> : <Search size={13} />}
            {discovering ? "Looking…" : "Look again"}
          </Action>
        </div>
      </Field>
      <Field
        title="Enter an address"
        hint="The address this computer reaches it on: an IP address or name, with the gateway's port when it is not 17879. An address on the public internet is refused: the gateway does not encrypt, so the pairing code and the token would travel in the clear."
      >
        <div className="flex w-full items-center gap-2">
          <input
            value={address}
            onChange={(event) => setAddress(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") ask(address);
            }}
            placeholder="192.168.1.20 or studio.local:17879"
            aria-label="Environment address"
            {...noTextHelpers}
            className={cx(fieldClass, "min-w-0 flex-1 font-mono")}
          />
          <Action prominent disabled={!address.trim()} onClick={() => ask(address)}>
            Ask to connect
          </Action>
        </div>
      </Field>
    </div>
  );
}

/** One found computer: what is known of it, its address, and the button that asks it. */
function FoundRow({
  found,
  local,
  onAsk,
}: {
  found: DiscoveredMachine;
  local: string | null;
  onAsk: () => void;
}) {
  const platform = found.platform ? platformName(found.platform) : undefined;
  const facts = [
    platform,
    found.version ? `RustCode ${found.version}` : undefined,
    routeName(null, found.routeKind),
  ];
  const skew = versionSkew(found.version, local, found.platform);
  return (
    <div className="flex items-center gap-3 px-4 py-3">
      {found.platform && <DeviceGlyphIcon glyph={platformIcon(found.platform)} size={16} />}
      <div className="min-w-0 flex-1">
        <div className="truncate font-medium text-body text-fg">{found.name}</div>
        <div className="text-callout text-fg-muted leading-normal">
          <span className="block">{facts.filter(Boolean).join(" · ")}</span>
          <span className="block">
            {found.address ? (
              <span className="selectable font-mono">{found.address}</span>
            ) : (
              "It announced no address this computer can reach."
            )}
          </span>
          {skew && <span className="mt-1 block text-warning">{skew}</span>}
          {found.hint && <span className="mt-1 block">{found.hint}</span>}
        </div>
      </div>
      {found.saved ? (
        <Chip tone="ok">Saved</Chip>
      ) : (
        <Action disabled={!found.address} onClick={onAsk}>
          Ask to connect
        </Action>
      )}
    </div>
  );
}

/** The request in flight: its four steps, and the one control the current step needs. */
function PairingView({ pairing }: { pairing: Pairing }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  const overdue = codeCheckOverdue(pairing, now);
  return (
    <div className="flex flex-col gap-3">
      <ProgressSteps
        steps={addressSteps(pairing.stage, overdue, pairing.host)}
        label="Add by address progress"
      />
      {pairing.stage === "waiting" && (
        <Field
          title={`Waiting for ${pairing.host} to accept…`}
          hint="The request is on that computer's screen. Accept it there, and the code to type appears here. Deny it there to stop, or wait two minutes."
        >
          <div>
            <Action onClick={cancelPairing}>Cancel</Action>
          </div>
        </Field>
      )}
      {pairing.stage === "pairing" &&
        (overdue ? (
          <Callout
            tone="warning"
            action={
              <>
                <Action size="sm" onClick={() => void askMachine(pairing.address, pairing.host)}>
                  Try again
                </Action>
                <Action quiet size="sm" onClick={cancelPairing}>
                  Cancel
                </Action>
              </>
            }
          >
            <span className="block font-medium text-fg">{pairing.host} did not answer</span>
            <span className="block pt-0.5">
              The code was sent, and nothing came back. If that computer accepted it, it now lists this
              computer under its paired devices: pair again, or remove this computer there first. Nothing was
              saved here.
            </span>
          </Callout>
        ) : (
          <div className="flex items-center gap-3 text-callout text-fg-muted">
            <Spinner /> Checking the code with {pairing.host}…
            <Action quiet size="sm" onClick={cancelPairing}>
              Cancel
            </Action>
          </div>
        ))}
      {pairing.stage === "code" && (
        <CodeEntry host={pairing.host} length={pairing.codeLength} expiresAt={pairing.expiresAt} />
      )}
    </div>
  );
}

/** The digits the other computer shows, typed in their own boxes. */
function CodeEntry({ host, length, expiresAt }: { host: string; length: number; expiresAt: number }) {
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
    <Field
      title={`The code ${host} shows`}
      hint={
        left > 0
          ? `${length} digits, on that computer's screen now. It works once and expires in ${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}.`
          : "The code has expired; cancel and ask again for a new one."
      }
    >
      <div className="flex flex-wrap items-center gap-3">
        <CodeInput length={length} value={code} onChange={setCode} onComplete={send} />
        <Action prominent disabled={!complete} onClick={send}>
          Connect
        </Action>
        <Action quiet onClick={cancelPairing}>
          Cancel
        </Action>
      </div>
    </Field>
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
