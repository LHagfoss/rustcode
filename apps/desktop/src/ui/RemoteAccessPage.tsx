// Settings › Remote access (docs/remote.md): the gateway, the threads this app
// shares with a paired phone, and device pairing. The engine owns the gateway,
// the shares and pairing, and answers `_rustcode/remote*`; the page only asks,
// polls while it is open, and draws the offer the engine hands back.

import { Copy, RefreshCw, Smartphone } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Json } from "../acp/jsonrpc";
import { parseRemoteStatus, RemoteMethod, type RemoteOffer, type RemoteStatus } from "../acp/remote";
import { encodeQr } from "../lib/qr";
import { errorText, request, toast, useApp } from "../state/store";
import { copyText, cx, Spinner, Switch } from "./primitives";
import { Action, Chip, Group, Loading, Row } from "./settingsKit";

/** The page keeps itself current while it is open: pairing counts down in the engine. */
const POLL_MS = 2500;

interface Reading {
  status: RemoteStatus;
  readAt: number;
}

export function RemoteAccessPage() {
  const engine = useApp((s) => s.status);
  const sessions = useApp((s) => s.sessions);
  const threads = useApp((s) => s.threads);
  const [readings, setReadings] = useState<Record<string, Reading>>({});
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const readToken = useRef(0);

  // The app's open threads are ACP sessions, and only those can be shared;
  // their titles come from the session list the app already has.
  const open = useMemo(
    () =>
      sessions
        .filter((session) => threads[session.id]?.loaded)
        .map((session) => ({ id: session.id, title: session.title || "Untitled thread" })),
    [sessions, threads],
  );
  // With no thread open, the newest saved one still answers the machine-wide
  // part of the status (the gateway, the devices); it shares nothing.
  const ids = open.length ? open.map((session) => session.id) : sessions.slice(0, 1).map((s) => s.id);
  const idsKey = ids.join("|");

  const refresh = useCallback(async () => {
    const ids = idsKey ? idsKey.split("|") : [];
    if (!ids.length) {
      setReadings({});
      setError(null);
      return;
    }
    const token = ++readToken.current;
    try {
      const entries = await Promise.all(
        ids.map(async (id) => {
          const value = await request<Json>(RemoteMethod.status, { sessionId: id }, 10_000);
          return [id, { status: parseRemoteStatus(value), readAt: Date.now() }] as const;
        }),
      );
      if (token !== readToken.current) return;
      setReadings(Object.fromEntries(entries));
      setError(null);
    } catch (e) {
      if (token === readToken.current) setError(errorText(e));
    }
  }, [idsKey]);

  useEffect(() => {
    if (engine !== "ready") return;
    void refresh();
    const timer = setInterval(() => void refresh(), POLL_MS);
    return () => clearInterval(timer);
  }, [engine, refresh]);

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);

  const list = Object.values(readings);
  const shared = list.filter((reading) => reading.status.shared);
  const offerReading = shared.find((reading) => reading.status.offer) ?? list.find((r) => r.status.offer);
  const offer = offerReading?.status.offer;
  const gateway = list.map((reading) => reading.status.gateway).find(Boolean);
  const devices = list.find((reading) => reading.status.devices.length)?.status.devices ?? [];
  const pending = list.some((reading) => reading.status.pairingPending);
  const expired = list.some((reading) => reading.status.pairingExpired);
  const pairingError = list.map((reading) => reading.status.pairingError).find(Boolean);
  const ended = shared.map((reading) => reading.status.error).find(Boolean);
  const unavailable = list.length > 0 && list.every((reading) => !reading.status.available);
  const enabled = list.some((reading) => reading.status.enabled);
  const on = shared.length > 0;
  // Pairing answers on one shared session; a new offer replaces the previous one.
  const target = shared[0]?.status.sessionId ?? open[0]?.id;

  const run = async (label: string, action: () => Promise<unknown>) => {
    setBusy(label);
    try {
      await action();
      await refresh();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  const start = (ids: string[], pair: boolean) =>
    Promise.all(ids.map((sessionId) => request(RemoteMethod.start, { sessionId, pair }, 30_000)));

  const toggle = (next: boolean) => {
    if (next)
      void run("start", () =>
        start(
          open.map((session) => session.id),
          false,
        ),
      );
    else if (target) void run("stop", () => request(RemoteMethod.stop, { sessionId: target }, 30_000));
  };

  const pair = () => {
    if (!target) return;
    void run("pair", () =>
      shared.length ? request(RemoteMethod.pair, { sessionId: target }, 30_000) : start([target], true),
    );
  };

  if (engine !== "ready" || (!list.length && ids.length > 0)) {
    return <Loading error={error} />;
  }

  const status = unavailable
    ? "Remote access is not available on this host."
    : ended
      ? ended
      : on
        ? shared.length === open.length
          ? `Sharing ${shared.length === 1 ? "this thread" : `these ${shared.length} threads`} with paired devices.`
          : `Sharing ${shared.length} of ${open.length} open threads.`
        : enabled
          ? "The gateway is on for this Mac, but this app shares nothing yet."
          : "Nothing is shared from this app.";

  return (
    <>
      {error && list.length > 0 && (
        <p className="mb-6 text-callout text-warning">Could not read the gateway: {error}</p>
      )}

      <Group
        title="Remote access"
        footer="Shared with the RustCode app on your phone through the remote gateway (docs/remote.md). Plain LAN traffic is authenticated but not encrypted: use NetBird away from a trusted network."
      >
        <Row
          title="Share this app's threads"
          subtitle="Starts the gateway and lists the threads this app has open in the phone app. Threads opened later can be shared from the list below."
          status={status}
        >
          <Switch
            label="Remote access"
            checked={on}
            disabled={!open.length || unavailable || busy !== null}
            onChange={toggle}
          />
        </Row>
        <Row
          title="Gateway address"
          subtitle="The address a phone on this network dials. The terminal's `/remote status` shows the same one."
          status={
            gateway?.advertisedAddress ? (
              <span className="font-mono">{gateway.advertisedAddress}</span>
            ) : on ? (
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
          <Chip tone={gateway?.connected ? "ok" : on ? "warn" : "idle"}>
            {gateway?.connected ? "Connected" : on ? "Starting" : "Off"}
          </Chip>
        </Row>
        <Row
          title="Pair a device"
          subtitle="Shows a QR code and an eight-digit code for the phone's Scan QR. The code lasts two minutes and works once; a new one replaces it."
          status={
            unavailable
              ? "Remote access needs the remote gateway, which this host does not build."
              : !open.length
                ? "Open a thread first: pairing happens through a shared thread."
                : pending
                  ? "Asking the gateway for a code…"
                  : expired
                    ? "The last code expired. Pair again for a fresh one."
                    : undefined
          }
        >
          <Action prominent disabled={!open.length || unavailable || busy !== null} onClick={pair}>
            {busy === "pair" ? <Spinner /> : <Smartphone size={13} />}
            Pair a device
          </Action>
        </Row>
      </Group>

      {(offer || pending || pairingError) && (
        <Group title="Pairing">
          <PairingCard
            offer={offer}
            pending={pending}
            error={pairingError}
            now={now}
            readAt={offerReading?.readAt ?? now}
            busy={busy === "pair"}
            onRenew={pair}
          />
        </Group>
      )}

      <Group
        title={`Paired devices · ${devices.length}`}
        footer="Revoke a device in a terminal: `rustcode remote revoke <device>`. A pairing lasts 30 days, then the device pairs again."
      >
        {devices.length ? (
          devices.map((device) => (
            <Row key={device.id} title={device.name} status={<span className="font-mono">{device.id}</span>}>
              <Chip tone="ok">Paired</Chip>
            </Row>
          ))
        ) : (
          <Row
            title="No devices paired yet"
            subtitle="A phone appears here once it scans the QR code or enters the eight-digit code."
          />
        )}
      </Group>

      <Group title="Open threads">
        {open.length ? (
          open.map((session) => {
            const reading = readings[session.id]?.status;
            const isShared = reading?.shared === true;
            return (
              <Row
                key={session.id}
                title={session.title}
                status={reading?.error ?? undefined}
                subtitle={isShared ? "Visible in the phone app." : "Not shared with paired devices."}
              >
                {isShared ? (
                  <Chip tone="ok">Shared</Chip>
                ) : (
                  <Action
                    disabled={(!enabled && !on) || unavailable || busy !== null}
                    onClick={() => void run(`start:${session.id}`, () => start([session.id], false))}
                  >
                    {busy === `start:${session.id}` ? <Spinner /> : null}
                    Share
                  </Action>
                )}
              </Row>
            );
          })
        ) : (
          <Row
            title="No thread is open"
            subtitle="Open a thread and it can be shared with your phone. Remote access shares the threads this app has open."
          />
        )}
      </Group>
    </>
  );
}

function mmss(seconds: number): string {
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

/**
 * The pairing offer as the terminal panel shows it: the QR code for the
 * camera, the address and eight-digit code for typing, the countdown and the
 * wrong codes left, and the row's own error when asking failed.
 */
function PairingCard({
  offer,
  pending,
  error,
  now,
  readAt,
  busy,
  onRenew,
}: {
  offer?: RemoteOffer;
  pending: boolean;
  error?: string;
  now: number;
  readAt: number;
  busy: boolean;
  onRenew: () => void;
}) {
  const left = offer ? Math.max(0, Math.ceil((readAt + offer.expiresInSecs * 1000 - now) / 1000)) : 0;
  const locked = offer?.lockedForSecs;
  return (
    <div className="px-4 py-4">
      {offer ? (
        <div className="flex flex-col gap-5 sm:flex-row sm:items-start">
          <QrCode payload={offer.qrPayload} />
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
              {offer.lockedForSecs !== undefined && <Chip tone="warn">Pairing locked</Chip>}
              {offer.loopbackOnly && <Chip tone="warn">This Mac only</Chip>}
              <Action quiet size="sm" disabled={busy} onClick={onRenew}>
                {busy ? <Spinner /> : <RefreshCw size={12} />}
                New code
              </Action>
            </div>
            {locked !== undefined && (
              <p className="m-0 pt-2 text-caption text-warning">
                Too many wrong codes on this host: pairing is locked for {mmss(locked)}, and even the right
                code is refused until then.
              </p>
            )}
            {offer.loopbackOnly && (
              <p className="m-0 pt-2 text-caption text-fg-faint">
                The gateway is listening on loopback only, so a phone cannot reach this Mac. Give it a LAN or
                NetBird address (docs/remote.md › Automatic address selection).
              </p>
            )}
          </div>
        </div>
      ) : pending ? (
        <div className="flex items-center gap-3 text-callout text-fg-muted">
          <Spinner />
          Asking the remote gateway for a pairing code…
        </div>
      ) : null}
      {error && <p className={cx("m-0 text-callout text-warning", offer && "pt-3")}>{error}</p>}
    </div>
  );
}

// design-lint: allow (a QR code is scanned: it stays dark on light in every theme)
const QR_LIGHT = "#ffffff";
// design-lint: allow (same: the modules keep maximum contrast for the camera)
const QR_DARK = "#000000";

/** The QR code as one SVG path: no canvas, no image, and nothing leaves the app. */
function QrCode({ payload }: { payload: string }) {
  const path = useMemo(() => {
    const code = encodeQr(payload, "M");
    if (!code) return null;
    const quiet = 4;
    const side = code.size + quiet * 2;
    const parts: string[] = [];
    code.modules.forEach((row, y) => {
      row.forEach((dark, x) => {
        if (dark) parts.push(`M${x + quiet} ${y + quiet}h1v1h-1z`);
      });
    });
    return { d: parts.join(""), side };
  }, [payload]);
  if (!path) return null;
  return (
    <svg
      viewBox={`0 0 ${path.side} ${path.side}`}
      width={180}
      height={180}
      role="img"
      aria-label="Pairing QR code"
      shapeRendering="crispEdges"
      className="shrink-0 rounded-md p-2"
      style={{ background: QR_LIGHT }}
    >
      <path d={path.d} fill={QR_DARK} />
    </svg>
  );
}
