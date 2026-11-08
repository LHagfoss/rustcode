// Remote access (`docs/remote.md`): switch it on for this Mac, read the
// gateway's address, pair a phone and list the devices that are paired. The
// engine owns the sharing (`_rustcode/remote*`): every thread this app has
// open, on any harness, appears on a paired device while the switch is on, and
// a prompt or an approval sent from the phone acts on the thread here.
//
// The pairing dialog shows the same challenge as the terminal's `/remote`
// panel: the QR code (drawn here from the module rows the engine sends, so no
// QR encoder ships with the app), the address, the manual code, the countdown
// and the wrong codes left.

import { Check, Copy, RefreshCw, Smartphone, Wifi, WifiOff } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { RemoteOffer, RemoteStatus } from "../acp/types";
import { errorText, pairRemoteDevice, refreshRemote, setRemoteAccess, useApp } from "../state/store";
import { cx, Pill, Spinner, Switch, Tooltip } from "./primitives";
import { Action, Group, Row } from "./settingsKit";

/** How often the panel re-reads the gateway while it is open. */
const REFRESH_MS = 5_000;
/** How often it re-reads while a pairing challenge is on screen. */
const PAIRING_REFRESH_MS = 1_000;

function stateOf(remote: RemoteStatus | null): {
  on: boolean;
  label: string;
  detail: string;
} {
  if (!remote) return { on: false, label: "Checking", detail: "Reading the engine's answer…" };
  if (!remote.available)
    return { on: false, label: "Unavailable", detail: "Remote access runs on macOS and Linux." };
  if (remote.error && !remote.enabled) return { on: false, label: "Off", detail: remote.error };
  if (!remote.enabled)
    return {
      on: false,
      label: "Off",
      detail: "No device can see anything on this Mac. Paired devices stay paired.",
    };
  if (!remote.gateway)
    return {
      on: true,
      label: "Starting",
      detail: "Remote access is on; the gateway is starting.",
    };
  return {
    on: true,
    label: "On",
    detail: remote.gateway.loopbackOnly
      ? "The gateway listens on this machine only: a phone cannot reach it."
      : "Every open thread is shared with paired devices.",
  };
}

export function RemoteAccessPage() {
  const remote = useApp((s) => s.remote);
  const remoteError = useApp((s) => s.remoteError);
  const [busy, setBusy] = useState(false);
  // The challenge on screen: its code and address come from the pairing
  // answer; its countdown, wrong codes left and end come from the polled
  // status, so a pairing, a lockout or an expiry shows up as it happens.
  const [pairing, setPairing] = useState<{ offer: RemoteOffer | null; paired: number } | null>(null);
  const state = stateOf(remote);

  const load = useCallback(() => {
    void refreshRemote();
  }, []);
  useEffect(() => {
    load();
    const timer = setInterval(load, pairing ? PAIRING_REFRESH_MS : REFRESH_MS);
    return () => clearInterval(timer);
  }, [load, pairing]);

  const toggle = (on: boolean) => {
    setBusy(true);
    void setRemoteAccess(on).finally(() => setBusy(false));
  };
  const pair = () => {
    setBusy(true);
    void pairRemoteDevice()
      .then((status) => {
        if (!status) return;
        setPairing({ offer: status.offer ?? null, paired: status.pairedDevices });
      })
      .finally(() => setBusy(false));
  };
  const notShared = typeof remote?.notShared === "number" ? remote.notShared : 0;

  const gateway = remote?.gateway;
  const address = gateway?.advertisedAddress;
  const paired = remote?.pairedDevices ?? 0;
  return (
    <>
      <Group title="Remote access">
        <Row
          title="Share this Mac's threads with a phone"
          subtitle={state.detail}
          status={
            <span className="inline-flex items-center gap-1.5">
              {state.on ? <Wifi size={12} /> : <WifiOff size={12} />}
              {state.label}
              {busy && <Spinner size={11} />}
            </span>
          }
        >
          <Switch
            label="Remote access"
            checked={state.on}
            disabled={busy || remote?.available === false}
            onChange={toggle}
          />
        </Row>
        <Row
          title="Pair a device"
          subtitle="Shows a code for the RustCode app on your phone. Scanning it pairs the device with this Mac; the code lasts two minutes."
        >
          <Action prominent disabled={busy || remote?.available === false} onClick={pair}>
            <Smartphone size={14} />
            Pair a device…
          </Action>
        </Row>
      </Group>

      <Group title="Gateway">
        <Row
          title="Address"
          subtitle="What a device dials. Pair on the same network, or over NetBird away from home."
          status={
            address
              ? `${address}${gateway?.loopbackOnly ? " · this machine only" : ""}`
              : gateway
                ? "No address yet"
                : "Not running"
          }
        >
          {address && <CopyButton text={address} label="address" />}
        </Row>
        <Row
          title="Paired devices"
          subtitle={
            paired === 0
              ? "None yet. Pair a device to see this Mac's threads on it."
              : "Each paired device sees every open thread while remote access is on. Revoke one with `rustcode remote revoke` in a terminal."
          }
          status={paired === 1 ? "1 device" : `${paired} devices`}
        >
          {paired > 0 && (
            <span className="flex max-w-[16rem] flex-wrap justify-end gap-1">
              {remote?.devices.map((device) => (
                <span
                  key={device.id}
                  className="inline-flex items-center gap-1 rounded-md bg-fill-subtle px-2 py-1 text-caption text-fg-muted"
                >
                  <Smartphone size={11} />
                  {device.name}
                </span>
              ))}
            </span>
          )}
        </Row>
        {remote?.error && <Row title="Last error" subtitle={remote.error} />}
        {(remoteError || (remote && !remote.available)) && (
          <Row
            title="Unreachable"
            subtitle={remoteError ?? "The engine does not answer for remote access."}
          />
        )}
        {notShared > 0 && (
          <Row
            title="Not shared"
            subtitle={
              notShared === 1
                ? "One open thread is held back — the host shares at most 32 at once, most recently used first. Its header says why."
                : `${notShared} open threads are held back — the host shares at most 32 at once, most recently used first. Their headers say why.`
            }
            status={notShared === 1 ? "1 thread" : `${notShared} threads`}
          />
        )}
      </Group>

      {pairing && (
        <PairDialog
          offer={pairing.offer}
          pairedAtOpen={pairing.paired}
          status={remote}
          onClose={() => setPairing(null)}
        />
      )}
    </>
  );
}

/** A row's value, copied in one press. */
function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Tooltip label={copied ? "Copied" : `Copy the ${label}`}>
      <Pill
        size="sm"
        aria-label={`Copy the ${label}`}
        onClick={() => {
          void navigator.clipboard?.writeText(text).then(
            () => {
              setCopied(true);
              setTimeout(() => setCopied(false), 1200);
            },
            () => {},
          );
        }}
      >
        {copied ? <Check size={13} /> : <Copy size={13} />}
      </Pill>
    </Tooltip>
  );
}

/**
 * The QR code the phone's camera reads, drawn from the module rows the engine
 * sends: `0` is light, `1` is dark, and the four-module quiet zone is already
 * part of the rows. Black on white on purpose — a scanner needs that pairing,
 * and no theme may recolour it (engine `remote_gateway/qr.rs`).
 */
function RemoteQr({ rows }: { rows: string[] }) {
  const size = rows[0]?.length ?? 0;
  // A scanner needs dark modules on a light ground, so the code keeps its own
  // colours whatever theme the app runs in (engine `remote_gateway/qr.rs`).
  // design-lint: allow (a QR code is black on white for the camera, never a theme colour)
  const paper = "size-[232px] shrink-0 rounded-lg bg-white p-1";
  // design-lint: allow (a QR code is black on white for the camera, never a theme colour)
  const ink = "#000";
  const path = useMemo(() => {
    // One rectangle per run of dark modules, so the path stays short.
    const parts: string[] = [];
    rows.forEach((row, y) => {
      let start = -1;
      for (let x = 0; x <= row.length; x += 1) {
        const dark = row[x] === "1";
        if (dark && start < 0) start = x;
        if (!dark && start >= 0) {
          const width = x - start;
          parts.push(`M${start} ${y}h${width}v1h-${width}z`);
          start = -1;
        }
      }
    });
    return parts.join("");
  }, [rows]);
  return (
    <svg
      viewBox={`0 0 ${size} ${size}`}
      role="img"
      aria-label="Pairing QR code"
      className={paper}
      shapeRendering="crispEdges"
    >
      <path d={path} fill={ink} />
    </svg>
  );
}

function useCountdown(seconds: number): number {
  const [left, setLeft] = useState(seconds);
  useEffect(() => {
    setLeft(seconds);
    if (seconds <= 0) return;
    const timer = setInterval(() => setLeft((value) => Math.max(0, value - 1)), 1_000);
    return () => clearInterval(timer);
  }, [seconds]);
  return left;
}

function clock(seconds: number): string {
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  return `${minutes}:${rest.toString().padStart(2, "0")}`;
}

/**
 * The pairing challenge: what to scan or type, and how long it lasts. The code
 * and address are the ones the pairing call returned; everything that moves —
 * the countdown, the wrong codes left, a lockout, the end of the challenge and
 * whether a device paired — comes from the status the page polls, so the
 * dialog never shows a frozen answer.
 */
function PairDialog({
  offer,
  pairedAtOpen,
  status,
  onClose,
}: {
  offer: RemoteOffer | null;
  pairedAtOpen: number;
  status: RemoteStatus | null;
  onClose: () => void;
}) {
  const pairing = status?.gateway?.pairing;
  const seconds = useCountdown(pairing?.expiresInSecs ?? offer?.expiresInSecs ?? 0);
  const [error, setError] = useState<string | null>(null);
  const [retrying, setRetrying] = useState(false);
  const attempt = pairing?.attemptsLeft;
  const lockedFor = pairing?.lockedForSecs ?? offer?.lockedForSecs ?? 0;
  const pairedNow = (status?.pairedDevices ?? pairedAtOpen) > pairedAtOpen;
  // The gateway reports the challenge closed; that is either a pairing, an
  // expiry, a lockout or a newer challenge replacing this one.
  const closed = status?.gateway?.pairingOpen === false || seconds <= 0;
  const expired = closed && !pairedNow;
  const pairAgain = () => {
    setRetrying(true);
    setError(null);
    void pairRemoteDevice()
      .then((next) => {
        if (next?.pairingError) setError(next.pairingError);
      })
      .catch((e) => setError(errorText(e)))
      .finally(() => setRetrying(false));
  };
  // Esc closes it, like every other dialog in the app.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  return (
    <div role="dialog" aria-modal="true" aria-label="Pair a device" className="fixed inset-0 z-40">
      <div aria-hidden className="absolute inset-0 bg-bg/60" onClick={onClose} />
      <div className="absolute inset-0 flex items-center justify-center p-4">
        <div className="relative flex w-full max-w-[34rem] flex-col gap-4 rounded-2xl bg-surface p-5 shadow-2">
          <div className="flex items-start gap-3">
            <div className="min-w-0 flex-1">
              <h2 className="font-semibold text-fg text-title3">Pair a device</h2>
              <p className="pt-1 text-callout text-fg-muted leading-normal">
                Open the RustCode app on your phone and tap <strong>Scan QR</strong>. The address and code
                below work when the camera cannot scan.
              </p>
            </div>
            <Action quiet onClick={onClose}>
              Done
            </Action>
          </div>

          {offer && !closed ? (
            <div className="flex flex-wrap items-start gap-4">
              {offer.qrRows?.length ? (
                <RemoteQr rows={offer.qrRows} />
              ) : (
                <div className="flex size-[232px] items-center justify-center rounded-lg bg-fill-subtle text-caption text-fg-faint">
                  No QR code for this address
                </div>
              )}
              <div className="flex min-w-[12rem] flex-1 flex-col gap-3">
                <div>
                  <div className="text-caption text-fg-faint">Address</div>
                  <div className="flex items-center gap-1.5">
                    <code className="min-w-0 flex-1 truncate font-mono text-body text-fg">
                      {offer.address ?? status?.gateway?.advertisedAddress ?? "unknown"}
                    </code>
                    {(offer.address ?? status?.gateway?.advertisedAddress) && (
                      <CopyButton
                        text={offer.address ?? status?.gateway?.advertisedAddress ?? ""}
                        label="address"
                      />
                    )}
                  </div>
                </div>
                <div>
                  <div className="text-caption text-fg-faint">Code</div>
                  <div className="flex items-center gap-1.5">
                    <code className="min-w-0 flex-1 font-mono text-fg text-title3 tracking-wider">
                      {offer.code ?? "—"}
                    </code>
                    {offer.code && <CopyButton text={offer.code} label="code" />}
                  </div>
                </div>
                <div className="text-callout text-fg-muted leading-normal">
                  <div className={cx(seconds <= 20 && "text-warn")}>
                    Expires in {clock(seconds)}
                    {typeof attempt === "number" &&
                      ` · ${attempt} wrong ${attempt === 1 ? "code" : "codes"} left`}
                  </div>
                  {lockedFor > 0 && (
                    <div className="pt-1 text-danger">
                      Pairing is locked for {clock(lockedFor)} after too many wrong codes: even the right code
                      is refused until then.
                    </div>
                  )}
                  {offer.loopbackOnly && (
                    <div className="pt-1 text-warn">
                      This address is this machine only: a phone cannot reach it. Set a `[remote]` bind in
                      `config.toml` or run `rustcode remote serve --bind auto`.
                    </div>
                  )}
                </div>
              </div>
            </div>
          ) : (
            <div className="flex flex-col gap-3 rounded-xl bg-fill-subtle p-4">
              <div className="text-body text-fg">
                {status?.pairingError
                  ? status.pairingError
                  : pairedNow
                    ? "Paired. The device sees this Mac's open threads now."
                    : expired
                      ? "This code has expired or was used up. A device pairs with a fresh one."
                      : "Waiting for the gateway to issue a code…"}
              </div>
              <div>
                <Action onClick={pairAgain} disabled={retrying}>
                  {retrying ? <Spinner size={12} /> : <RefreshCw size={14} />}
                  New code
                </Action>
              </div>
            </div>
          )}
          {error && <div className="text-callout text-danger">{error}</div>}
        </div>
      </div>
    </div>
  );
}

/**
 * A thread's own share, in its header: a small chip while the thread is shared
 * (with the number of devices attached to it), and a muted one saying the
 * host cannot share this thread right now.
 *
 * It polls the thread's own status, so it does not depend on Settings being
 * open to learn that remote access was switched on in a terminal.
 */
export function RemoteThreadIndicator({ sessionId }: { sessionId?: string }) {
  const remote = useApp((s) => (sessionId ? s.remoteByThread[sessionId] : undefined));
  const load = useCallback(() => {
    if (sessionId) void refreshRemote(sessionId);
  }, [sessionId]);
  useEffect(() => {
    if (!sessionId) return;
    load();
    const timer = setInterval(load, REFRESH_MS);
    return () => clearInterval(timer);
  }, [load, sessionId]);
  if (!sessionId || !remote) return null;
  if (!remote.shared) {
    if (!remote.notShared) return null;
    return (
      <Tooltip label={`Not shared with paired devices: ${remote.notShared}`}>
        <span
          role="status"
          aria-label={`Not shared: ${remote.notShared}`}
          className="inline-flex shrink-0 items-center gap-1 rounded-md bg-fill-subtle px-1.5 py-0.5 text-caption text-fg-faint"
        >
          <WifiOff size={11} />
        </span>
      </Tooltip>
    );
  }
  const count = (remote.gateway?.attachedDevices ?? []).length;
  return (
    <Tooltip
      label={
        count === 0
          ? "Remote access is on: a paired device can open this thread."
          : `${count === 1 ? "A device is" : `${count} devices are`} attached to this thread.`
      }
    >
      <span
        role="status"
        aria-label={count === 0 ? "Shared with paired devices" : `${count} device attached`}
        className={cx(
          "inline-flex shrink-0 items-center gap-1 rounded-md px-1.5 py-0.5 text-caption",
          count > 0 ? "bg-accent/15 text-accent" : "bg-fill-subtle text-fg-faint",
        )}
      >
        <Smartphone size={11} />
        {count > 0 ? count : null}
      </span>
    </Tooltip>
  );
}
