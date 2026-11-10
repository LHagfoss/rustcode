// Settings › Remote access (docs/remote.md): the switch that shares this app's
// open threads with paired phones, the gateway's address, pairing, the paired
// devices and each open thread's share. The engine owns the gateway, the
// shares and pairing, and answers `_rustcode/remote*`; the page asks, polls
// while it is open, and draws the offer the engine hands back.
//
// Every open thread is shared, whatever harness runs it (the built-in one, or
// Codex, Claude Code, Pi and any other external harness): a device sees them
// all while the switch is on, and a prompt, cancel, question or approval sent
// from the phone acts on the thread here. The host shares at most 32 at once,
// most recently used first, and says why it holds the rest back.

import { Copy, RefreshCw, Smartphone, WifiOff } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  heldBackReason,
  type RemoteOffer,
  type RemotePairing,
  type RemoteRefusedConnection,
  type RemoteRoute,
} from "../acp/remote";
import { encodeQr } from "../lib/qr";
import { deviceLabel, lastSeenLabel } from "../state/connectRequests";
import { installServer, refreshServerService, serverLine, useMachines } from "../state/machines";
import { refusalAgo, refusalCopy } from "../state/refusals";
import { activeChallenge, remainingPairingSeconds } from "../state/remotePairing";
import {
  askConfirm,
  dismissRemoteChallenge,
  errorText,
  pairRemoteDevice,
  refreshRemote,
  setRemoteAccess,
  toast,
  useApp,
} from "../state/store";
import { copyText, cx, Spinner, Switch } from "./primitives";
import { ConnectRequestsGroup, RevokeDeviceAction } from "./RemoteConnect";
import { Action, Chip, Group, Loading, Row } from "./settingsKit";

/** The page keeps itself current while it is open: pairing counts down in the engine. */
const POLL_MS = 2500;

/**
 * How long the page waits for the gateway's pairing answer before it says the
 * gateway did not answer. The engine's own wait is 15 s (`acp/remote.rs`).
 */
const PAIR_DEADLINE_MS = 20_000;

/** Wrong codes one challenge tolerates (`remote_gateway/pairing.rs`). */
const WRONG_CODES_ALLOWED = 5;

export function RemoteAccessPage() {
  const engine = useApp((s) => s.status);
  const remote = useApp((s) => s.remote);
  const remoteError = useApp((s) => s.remoteError);
  const [busy, setBusy] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const server = useMachines((s) => s.server);
  const installing = useMachines((s) => s.serverInstalling);
  const challenge = useApp((s) => s.remoteChallenge);
  const paired = useApp((s) => s.remotePaired);
  const [pairAskedAt, setPairAskedAt] = useState<number | null>(null);

  const refresh = useCallback(async () => {
    await refreshRemote();
  }, []);

  useEffect(() => {
    if (engine !== "ready") return;
    void refresh();
    // The background server is what keeps threads running with the app closed
    // once RustCode uses it, so its state is read with the gateway's.
    void refreshServerService();
    const timer = setInterval(() => void refresh(), POLL_MS);
    return () => clearInterval(timer);
  }, [engine, refresh]);

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);

  const devices = remote?.devices ?? [];
  const refused = remote?.refusedConnections ?? [];
  const gateway = remote?.gateway;
  const active = activeChallenge(challenge, now);
  const offer = active?.offer;
  const pairing = gateway?.pairing;
  const unavailable = remote?.available === false;
  const enabled = remote?.enabled === true;
  const pending = busy === "pair" || remote?.pairingPending === true;
  const expired = remote?.pairingExpired === true;
  const pairingError = remote?.pairingError;
  const ended = remote?.error;

  useEffect(() => {
    if (challenge && now >= challenge.expiresAt) dismissRemoteChallenge();
  }, [challenge, now]);

  const run = async (label: string, action: () => Promise<unknown>) => {
    setBusy(label);
    try {
      await action();
    } catch (e) {
      // `setRemoteAccess` and `pairRemoteDevice` also toast their own refusal.
      if (label !== "pair" && label !== "toggle") toast(errorText(e), "error");
    } finally {
      await refresh();
      setBusy(null);
    }
  };

  /**
   * Turning network access on also makes sure the RustCode server is
   * installed: it stays up when the app closes, so the threads that live in it
   * keep running once the app uses it. Nothing is installed before the owner
   * agrees, and a running turn is never ended for it.
   */
  const toggle = (next: boolean) => {
    if (!next) {
      void run("toggle", () => setRemoteAccess(false));
      return;
    }
    const turnOn = () => void run("toggle", () => setRemoteAccess(true));
    const server = useMachines.getState().server;
    if (server === null) {
      void refreshServerService().then(() => {
        if (useMachines.getState().server?.installed) turnOn();
        else askInstallThenTurnOn(turnOn);
      });
      return;
    }
    if (server.installed) {
      turnOn();
      return;
    }
    askInstallThenTurnOn(turnOn);
  };

  const pair = () => {
    setPairAskedAt(Date.now());
    void run("pair", () => pairRemoteDevice());
  };

  if (engine !== "ready" || (remote === null && !remoteError)) {
    return <Loading error={remoteError} />;
  }

  const sharedCount = gateway?.sessions ?? 0;
  const status = unavailable
    ? "Remote access is not available on this host."
    : ended
      ? ended
      : enabled
        ? sharedCount > 0
          ? `Sharing ${sharedCount === 1 ? "one thread" : `${sharedCount} threads`} with paired devices.`
          : "Remote access is on for this computer. Every thread on it is listed for paired devices."
        : "Nothing is shared from this app.";

  return (
    <>
      {remoteError && (
        <p className="mb-6 text-callout text-warning">Could not read the gateway: {remoteError}</p>
      )}

      <Group
        title="Remote access"
        footer="Shared with the RustCode app on your phone through the remote gateway (docs/remote.md). Plain LAN traffic is authenticated but not encrypted: use NetBird away from a trusted network."
      >
        <Row
          title="Network access"
          subtitle="Starts the gateway and lets paired devices reach every thread on this computer, across all projects, not only the ones open here. Turning it off stops remote access for this computer, including a share a terminal started; paired devices stay paired."
          status={status}
        >
          <Switch
            label="Network access"
            checked={enabled}
            disabled={unavailable || busy !== null}
            onChange={toggle}
          />
        </Row>
        <Row
          title="RustCode server"
          subtitle="Installs the RustCode server as a background service: it stays up when the app closes, and starts when you log in. Threads that live in it keep running; the threads this app starts still run in its own engine and end with it, until RustCode uses the server. Turning network access on installs it when it is not installed."
          status={serverLine(server)}
        >
          {server === null ? null : server.installed ? (
            <Chip tone={server.running ? "ok" : "warn"}>{server.running ? "Running" : "Not running"}</Chip>
          ) : (
            <Action disabled={busy !== null || installing} onClick={() => void installServer()}>
              {installing ? <Spinner /> : null}
              Install
            </Action>
          )}
        </Row>
        <Row
          title="Gateway address"
          subtitle="The address a phone on this network dials. The terminal's `/remote status` shows the same one."
          status={
            gateway?.advertisedAddress ? (
              <span className="font-mono">
                {gateway.advertisedAddress}
                {gateway.connected && gateway.connectedDevices.length > 0
                  ? ` · ${gateway.connectedDevices.length} device${gateway.connectedDevices.length === 1 ? "" : "s"} connected`
                  : ""}
                {gateway.loopbackOnly ? " · this computer only" : ""}
              </span>
            ) : enabled ? (
              "The gateway has not announced an address yet."
            ) : (
              "The gateway is not running."
            )
          }
        >
          {gateway?.advertisedAddress && (
            <Action
              aria-label="Copy the gateway address"
              onClick={() => void copyText(gateway.advertisedAddress!)}
            >
              <Copy size={12} /> Copy
            </Action>
          )}
          <Chip tone={gateway?.connected ? "ok" : enabled ? "warn" : "idle"}>
            {gateway?.connected ? "Connected" : enabled ? "Starting" : "Off"}
          </Chip>
        </Row>
        <Row
          title="Pair a device"
          subtitle={`Shows a QR code and an eight-digit code for the phone's Scan QR. The code lasts two minutes and works once; a new one replaces it. ${WRONG_CODES_ALLOWED} wrong codes are allowed.`}
          status={
            unavailable
              ? "Remote access needs the remote gateway, which this host does not build."
              : pending
                ? "Asking the gateway for a code…"
                : expired
                  ? "The last code expired. Pair again for a fresh one."
                  : undefined
          }
        >
          <Action prominent disabled={unavailable || busy !== null} onClick={pair}>
            {busy === "pair" ? <Spinner /> : <Smartphone size={13} />}
            Pair a device
          </Action>
        </Row>
      </Group>

      {paired ? (
        <Group title="Pairing">
          <div className="flex flex-wrap items-center gap-3 px-4 py-4">
            <Chip tone="ok">Paired</Chip>
            <p className="m-0 min-w-0 flex-1 text-callout text-fg">
              <span className="font-medium">{paired.name}</span> can now open the shared threads.
            </p>
            <Action disabled={busy !== null} onClick={pair}>
              {busy === "pair" ? <Spinner /> : <Smartphone size={12} />}
              Pair another device
            </Action>
          </div>
        </Group>
      ) : (
        (offer || pending || pairingError || busy === "pair") && (
          <Group title="Pairing">
            <PairingCard
              offer={offer}
              pairing={pairing}
              // The request itself can wait for the gateway: say so from the press.
              pending={pending}
              error={pairingError}
              now={now}
              expiresAt={active?.expiresAt}
              overdue={pending && pairAskedAt !== null && now - pairAskedAt > PAIR_DEADLINE_MS}
              busy={busy === "pair"}
              onRenew={pair}
              onStop={() => toggle(false)}
              onDismiss={dismissRemoteChallenge}
            />
          </Group>
        )
      )}

      <ConnectRequestsGroup />

      {refused.length > 0 && <RefusedConnectionsGroup refused={refused} />}

      <Group
        title={`Paired devices · ${devices.length}`}
        footer="A pairing stays until you revoke it here, so a device does not have to pair again. Revoke disconnects it at once."
      >
        {devices.length ? (
          devices.map((device) => (
            <Row
              key={device.id}
              title={device.name}
              subtitle={`${deviceLabel(device.kind, device.platform)} · ${lastSeenLabel(device, now)}`}
              status={<span className="font-mono">{device.id}</span>}
            >
              <Chip tone={device.connected ? "ok" : "idle"}>{device.connected ? "Connected" : "Paired"}</Chip>
              <RevokeDeviceAction device={device} />
            </Row>
          ))
        ) : (
          <Row
            title="No devices paired yet"
            subtitle="A phone or another computer's RustCode app appears here once it scans the QR code, enters the eight-digit code, or asks to connect and you accept."
          />
        )}
      </Group>

      <Group
        title="Routes"
        footer="Your phone tries these addresses in order and keeps using the first that answers. It learns them here, so pairing once is enough to reconnect on another network. Plain LAN traffic is authenticated but not encrypted: prefer the VPN or host name away from a trusted network."
      >
        {gateway?.routes && gateway.routes.length > 0 ? (
          gateway.routes.map((route) => (
            <Row
              key={route.address}
              title={routeLabel(route)}
              subtitle={routeHint(route)}
              status={<span className="font-mono">{route.address}</span>}
            >
              {route.address === gateway.advertisedAddress && <Chip tone="ok">Advertised</Chip>}
              <Action
                aria-label={`Copy the route ${route.address}`}
                onClick={() => void copyText(route.address)}
              >
                <Copy size={12} /> Copy
              </Action>
            </Row>
          ))
        ) : (
          <Row
            title="No route yet"
            subtitle={
              enabled
                ? "The gateway has not announced an address a phone can dial. Give it a LAN or NetBird address (docs/remote.md › Automatic address selection)."
                : "Turn network access on to start the gateway and announce this computer's addresses."
            }
          />
        )}
      </Group>
    </>
  );
}

/**
 * Ask before installing the server on the way to turning network access on,
 * in the words that explain it. A failed install does not stop network access:
 * the reason is a message of its own.
 */
function askInstallThenTurnOn(turnOn: () => void) {
  askConfirm({
    title: "Install the RustCode server too?",
    message:
      "Network access lets paired devices reach this computer's threads. RustCode also installs its server as a background service: it stays up when the app closes and starts when you log in, so the threads that live in it keep running. The threads this app starts itself still run in its own engine and end with it, until RustCode uses the server. Threads, settings and pairings stay as they are. Network access turns on either way.",
    action: "Turn on and install",
    run: () => {
      turnOn();
      void installServer();
    },
  });
}

/**
 * Connections the gateway turned away, newest first: the desktop half of "the
 * phone cannot connect". A phone that speaks another protocol version, sends a
 * code that does not work, or reconnects with a pairing this Mac no longer
 * accepts says so here, with what to do about it, instead of the phone only
 * saying it cannot connect.
 */
export function RefusedConnectionsGroup({ refused }: { refused: RemoteRefusedConnection[] }) {
  return (
    <Group
      title="Refused connections"
      footer="A device that could not get in is listed here for a quarter of an hour after it stopped trying. Nothing was shared with it: no thread, no prompt and no code was accepted."
    >
      {refused.map((entry) => {
        const copy = refusalCopy(entry);
        const when = refusalAgo(entry.refusedSecsAgo);
        return (
          <Row
            key={`${entry.reason}-${entry.deviceName ?? ""}-${entry.protocolVersion ?? ""}-${entry.remoteAddr}`}
            title={copy.title}
            subtitle={copy.detail}
            status={
              <span>
                {[
                  entry.attempts > 1 ? `${entry.attempts} tries` : undefined,
                  when,
                  entry.remoteAddr || undefined,
                ]
                  .filter(Boolean)
                  .join(" · ")}
              </span>
            }
          >
            <Chip tone="warn">{deviceLabel(entry.deviceKind, entry.platform)}</Chip>
          </Row>
        );
      })}
    </Group>
  );
}

/** The mesh product an overlay route runs on, in the spelling the product uses. */
const PROVIDER_NAMES: Record<NonNullable<RemoteRoute["provider"]>, string> = {
  netbird: "NetBird",
  tailscale: "Tailscale",
  zerotier: "ZeroTier",
};

/** A route, in the words the page shows. An overlay names its product when the engine says. */
function routeLabel(route: RemoteRoute): string {
  switch (route.kind) {
    case "lan":
      return "Local network";
    case "overlay":
    case "vpn":
      return (route.provider && PROVIDER_NAMES[route.provider]) ?? "Overlay network";
    case "hostname":
      return "Host name";
    case "public":
      return "Public";
    default:
      return "Address";
  }
}

/** What a route means for the phone. */
function routeHint(route: RemoteRoute): string {
  switch (route.kind) {
    case "lan":
      return "The phone's own Wi-Fi or Ethernet network. Not encrypted.";
    case "overlay":
    case "vpn":
      return route.provider
        ? `A ${PROVIDER_NAMES[route.provider]} address, reachable away from home and encrypted by the overlay network.`
        : "A NetBird, Tailscale or ZeroTier address, reachable away from home and encrypted by the tunnel.";
    case "hostname":
      return "A name that follows this computer when its address changes.";
    case "public":
      return "Reachable from the internet. Plain LAN traffic is not encrypted: avoid it.";
    default:
      return "An address the gateway reports.";
  }
}

function mmss(seconds: number): string {
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

/**
 * The pairing offer as the terminal panel shows it: the QR code for the
 * camera, the address and eight-digit code for typing, and the countdown.
 * The gateway reports the challenge's own countdown, wrong codes left and any
 * lockout, so the card counts down with the engine rather than on its own.
 */
export function PairingCard({
  offer,
  pairing,
  pending,
  error,
  now,
  expiresAt,
  overdue,
  busy,
  onRenew,
  onStop,
  onDismiss,
}: {
  offer?: RemoteOffer;
  pairing?: RemotePairing;
  pending: boolean;
  error?: string;
  now: number;
  expiresAt?: number;
  overdue: boolean;
  busy: boolean;
  onRenew: () => void;
  onStop: () => void;
  onDismiss: () => void;
}) {
  const left = expiresAt === undefined ? 0 : remainingPairingSeconds(expiresAt, now);
  const lockedFor = pairing?.lockedForSecs ?? offer?.lockedForSecs ?? 0;
  const attemptsLeft = pairing?.attemptsLeft;
  return (
    <div className="px-4 py-4">
      {offer ? (
        <div className="flex flex-col gap-5 sm:flex-row sm:items-start">
          <QrCode payload={offer.qrPayload} rows={offer.qrRows} />
          <div className="min-w-0 flex-1">
            <div className="font-medium text-body text-fg">Scan the code with the phone app</div>
            <p className="m-0 max-w-[52ch] pt-1 text-callout text-fg-muted leading-normal">
              Open the RustCode app, tap Scan QR, and point the camera here. When the camera cannot reach it,
              enter the address and code by hand.
            </p>
            <div className="flex flex-wrap items-baseline gap-x-6 gap-y-2 pt-3">
              <div>
                <div className="text-caption text-fg-faint">Address</div>
                <div className="font-mono text-callout text-fg">{offer.address}</div>
                {offer.alternates && offer.alternates.length > 0 && (
                  <div className="pt-0.5 text-caption text-fg-faint">
                    Also reachable at <span className="font-mono">{offer.alternates.join(", ")}</span>
                  </div>
                )}
              </div>
              <div>
                <div className="text-caption text-fg-faint">Code</div>
                <div className="flex items-baseline gap-2">
                  <span className="font-mono font-medium text-title3 text-fg tracking-[0.08em]">
                    {offer.code}
                  </span>
                  <Action
                    quiet
                    size="sm"
                    aria-label="Copy the pairing code"
                    onClick={() => void copyText(offer.code)}
                  >
                    <Copy size={12} /> Copy
                  </Action>
                </div>
              </div>
            </div>
            <div className="flex flex-wrap items-center gap-2 pt-3 text-caption text-fg-faint">
              {left > 0 ? <span className="tabular">Expires in {mmss(left)}</span> : <span>Expired</span>}
              <span>
                {typeof attemptsLeft === "number"
                  ? `${attemptsLeft} wrong ${attemptsLeft === 1 ? "code" : "codes"} left`
                  : `${WRONG_CODES_ALLOWED} wrong codes allowed`}
              </span>
              {lockedFor > 0 && <Chip tone="warn">Pairing locked</Chip>}
              {offer.loopbackOnly && <Chip tone="warn">This computer only</Chip>}
              <Action quiet size="sm" onClick={onDismiss}>
                Dismiss
              </Action>
              <Action quiet size="sm" disabled={busy} onClick={onRenew}>
                {busy ? <Spinner /> : <RefreshCw size={12} />}
                New code
              </Action>
            </div>
            {lockedFor > 0 && (
              <p className="m-0 pt-2 text-caption text-warning">
                Too many wrong codes on this host: pairing is locked for {mmss(lockedFor)}, and even the right
                code is refused until then.
              </p>
            )}
            {offer.loopbackOnly && (
              <p className="m-0 pt-2 text-caption text-fg-faint">
                The gateway is listening on loopback only, so a phone cannot reach this computer. Give it a
                LAN or NetBird address (docs/remote.md › Automatic address selection).
              </p>
            )}
          </div>
        </div>
      ) : pending ? (
        overdue ? (
          <div className="flex flex-col gap-3 text-callout text-fg-muted">
            <p className="m-0">
              The gateway did not answer in time. It may be starting, or another program holds it. Try again,
              or turn remote access off for this computer.
            </p>
            <div className="flex items-center gap-2">
              <Action onClick={onRenew}>
                <RefreshCw size={12} /> Pair again
              </Action>
              <Action onClick={onStop}>Turn off remote access</Action>
            </div>
          </div>
        ) : (
          <div className="flex items-center gap-3 text-callout text-fg-muted">
            <Spinner />
            Asking the remote gateway for a pairing code…
          </div>
        )
      ) : null}
      {error && <p className={cx("m-0 text-callout text-warning", offer && "pt-3")}>{error}</p>}
    </div>
  );
}

// design-lint: allow (a QR code is scanned: it stays dark on light in every theme)
const QR_LIGHT = "#ffffff";
// design-lint: allow (same: the modules keep maximum contrast for the camera)
const QR_DARK = "#000000";

/**
 * The QR code as one SVG path: no canvas, no image, and nothing leaves the
 * app. The engine sends the modules (`qrRows`, one string of `0` and `1` per
 * row, quiet zone included); an answer without them is encoded here from the
 * payload (`lib/qr.ts`).
 */
function QrCode({ payload, rows }: { payload: string; rows?: string[] }) {
  const code = useMemo(() => {
    if (rows?.length) {
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
      return { d: parts.join(""), side: rows[0]?.length ?? 0 };
    }
    const encoded = encodeQr(payload, "M");
    if (!encoded) return null;
    const quiet = 4;
    const side = encoded.size + quiet * 2;
    const parts: string[] = [];
    encoded.modules.forEach((row, y) => {
      row.forEach((dark, x) => {
        if (dark) parts.push(`M${x + quiet} ${y + quiet}h1v1h-1z`);
      });
    });
    return { d: parts.join(""), side };
  }, [payload, rows]);
  if (!code) return null;
  return (
    <svg
      viewBox={`0 0 ${code.side} ${code.side}`}
      width={180}
      height={180}
      role="img"
      aria-label="Pairing QR code"
      shapeRendering="crispEdges"
      className="shrink-0 rounded-md p-2"
      style={{ background: QR_LIGHT }}
    >
      <path d={code.d} fill={QR_DARK} />
    </svg>
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
  useEffect(() => {
    if (!sessionId) return;
    void refreshRemote(sessionId);
    const timer = setInterval(() => void refreshRemote(sessionId), POLL_MS);
    return () => clearInterval(timer);
  }, [sessionId]);
  if (!sessionId || !remote) return null;
  if (!remote.shared) {
    const reason = heldBackReason(remote);
    if (!reason) return null;
    return (
      <Chip tone="warn" title={`Not shared with paired devices: ${reason}`}>
        <WifiOff size={11} />
        <span className="sr-only">Not shared with paired devices: {reason}</span>
      </Chip>
    );
  }
  const count = remote.gateway?.attachedDevices.length ?? 0;
  const label =
    count === 0
      ? "Shared with paired devices"
      : `${count} ${count === 1 ? "device" : "devices"} attached to this thread`;
  return (
    <Chip tone={count > 0 ? "ok" : "idle"} title={label}>
      <Smartphone size={11} />
      {count > 0 ? count : <span className="sr-only">{label}</span>}
    </Chip>
  );
}
