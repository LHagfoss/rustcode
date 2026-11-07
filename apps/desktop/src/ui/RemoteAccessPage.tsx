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

/**
 * How long the page waits for the gateway's pairing answer before it says the
 * gateway did not answer. The engine's own wait is 15 s (`acp/remote.rs`).
 */
const PAIR_DEADLINE_MS = 20_000;

/** Wrong codes one challenge tolerates (`remote_gateway/pairing.rs`). */
const WRONG_CODES_ALLOWED = 5;

interface Reading {
  status: RemoteStatus;
  readAt: number;
}

interface OpenThread {
  id: string;
  title: string;
  /** The harness the thread runs on; only the built-in one is an ACP session. */
  harness: string;
}

export function RemoteAccessPage() {
  const engine = useApp((s) => s.status);
  const sessions = useApp((s) => s.sessions);
  const threads = useApp((s) => s.threads);
  const [readings, setReadings] = useState<Record<string, Reading>>({});
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [paired, setPaired] = useState<{ name: string } | null>(null);
  const [pendingSince, setPendingSince] = useState<number | null>(null);
  const readToken = useRef(0);

  // The app's open threads, with the harness each runs on. Only the built-in
  // harness keeps an ACP session (`server.sessions`); an external harness
  // thread lives in `server.external`, which `_rustcode/remote/start` does
  // not share.
  const open = useMemo<OpenThread[]>(
    () =>
      sessions
        .filter((session) => threads[session.id]?.loaded)
        .map((session) => ({
          id: session.id,
          title: session.title || "Untitled thread",
          harness: threads[session.id]?.harness?.id ?? session.harness ?? "",
        })),
    [sessions, threads],
  );
  const shareable = open.filter((thread) => thread.harness === "rustcode");
  const external = open.filter((thread) => thread.harness !== "rustcode");
  // With no thread open, the newest saved one still answers the machine-wide
  // part of the status (the gateway, the devices); it shares nothing.
  const ids = open.length ? open.map((thread) => thread.id) : sessions.slice(0, 1).map((s) => s.id);
  const idsKey = ids.join("|");

  const refresh = useCallback(async () => {
    const asking = idsKey ? idsKey.split("|") : [];
    if (!asking.length) {
      setReadings({});
      setError(null);
      return;
    }
    const token = ++readToken.current;
    try {
      const entries = await Promise.all(
        asking.map(async (id) => {
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
  const target = shared[0]?.status.sessionId ?? shareable[0]?.id;

  // A used offer stays in the engine's answer until it expires, so the page
  // watches for a device it did not know when the offer arrived: that is the
  // phone pairing. A new offer starts the known list again.
  const devicesKey = devices.map((device) => device.id).join("|");
  const offerKey = offer ? String(offer.challenge ?? offer.qrPayload) : "";
  const knownDevices = useRef<{ offer: string; ids: string[] } | null>(null);
  // The devices just before a code is asked for: a phone that pairs at once
  // is new against that list, not against the answer that already has it.
  const baselineDevices = useRef<string[] | null>(null);
  useEffect(() => {
    if (!offer) {
      knownDevices.current = null;
      return;
    }
    const known = devicesKey ? devicesKey.split("|") : [];
    if (knownDevices.current?.offer !== offerKey) {
      knownDevices.current = { offer: offerKey, ids: baselineDevices.current ?? known };
      baselineDevices.current = null;
      setPaired(null);
      return;
    }
    const fresh = known.find((id) => !knownDevices.current!.ids.includes(id));
    if (!fresh) return;
    knownDevices.current.ids.push(fresh);
    const name = devices.find((device) => device.id === fresh)?.name ?? fresh;
    setPaired((current) => (current?.name === name ? current : { name }));
  }, [offer, offerKey, devices, devicesKey]);

  useEffect(() => {
    setPendingSince((current) => (pending ? (current ?? Date.now()) : null));
  }, [pending]);

  // Remote access can end from a terminal too: drop a "Paired" note then.
  useEffect(() => {
    if (!enabled && !on) setPaired(null);
  }, [enabled, on]);

  const run = async (label: string, action: () => Promise<unknown>) => {
    setBusy(label);
    try {
      await action();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      await refresh();
      setBusy(null);
    }
  };

  /**
   * Share each thread. The first start also brings the gateway up (it can take
   * seconds), so it goes alone; the rest register on the running gateway in
   * parallel, and one refusal does not hide the others.
   */
  const start = async (sessionIds: string[], pair: boolean) => {
    const [first, ...rest] = sessionIds;
    if (!first) return;
    await request(RemoteMethod.start, { sessionId: first, pair }, 30_000);
    const settled = await Promise.allSettled(
      rest.map((sessionId) => request(RemoteMethod.start, { sessionId, pair: false }, 30_000)),
    );
    const failed = settled.find((result): result is PromiseRejectedResult => result.status === "rejected");
    if (failed) throw failed.reason;
  };

  // `_rustcode/remote/stop` is machine-wide; the session id is only the
  // answer's own label, so a saved thread is enough when none is shared.
  const stopId = target ?? ids[0];
  const stop = () => {
    if (!stopId) return;
    void run("stop", () => request(RemoteMethod.stop, { sessionId: stopId }, 30_000));
    setPaired(null);
  };

  const toggle = (next: boolean) => {
    if (next)
      void run("start", () =>
        start(
          shareable.map((thread) => thread.id),
          false,
        ),
      );
    else stop();
  };

  const pair = () => {
    if (!target) return;
    setPaired(null);
    baselineDevices.current = devices.map((device) => device.id);
    setPendingSince(Date.now());
    void run("pair", async () => {
      const value = await request<Json>(RemoteMethod.pair, { sessionId: target }, 30_000);
      const answer = parseRemoteStatus(value);
      // A refusal is an answer, not an error: `{available, shared: false, error}`.
      if (answer.error) throw new Error(answer.error);
      setReadings((current) => ({
        ...current,
        [target]: { status: answer, readAt: Date.now() },
      }));
    });
  };

  if (engine !== "ready" || (!list.length && ids.length > 0)) {
    return <Loading error={error} />;
  }

  const status = unavailable
    ? "Remote access is not available on this host."
    : ended
      ? ended
      : on
        ? shared.length === shareable.length
          ? `Sharing ${shared.length === 1 ? "this thread" : `these ${shared.length} threads`} with paired devices.`
          : `Sharing ${shared.length} of ${shareable.length} shareable threads.`
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
          subtitle="Starts the gateway and lists the threads this app has open on the built-in harness in the phone app. Turning it off stops remote access for this Mac, including a share a terminal started; paired devices stay paired."
          status={status}
        >
          <Switch
            label="Remote access"
            checked={on}
            disabled={!shareable.length || unavailable || busy !== null}
            onChange={toggle}
          />
        </Row>
        {enabled && !on && (
          <Row
            title="Remote access is on for this Mac"
            subtitle="A terminal turned it on and still shares there. Turning it off stops the gateway for the whole machine."
          >
            <Action disabled={busy !== null || !stopId} onClick={stop}>
              Turn off
            </Action>
          </Row>
        )}
        <Row
          title="Gateway address"
          subtitle="The address a phone on this network dials. The terminal's `/remote status` shows the same one."
          status={
            gateway?.advertisedAddress ? (
              <span className="font-mono">
                {gateway.advertisedAddress}
                {gateway.connected
                  ? ` · ${gateway.connectedDevices} device${gateway.connectedDevices === 1 ? "" : "s"} connected`
                  : ""}
              </span>
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
          subtitle={`Shows a QR code and an eight-digit code for the phone's Scan QR. The code lasts two minutes and works once; a new one replaces it. ${WRONG_CODES_ALLOWED} wrong codes are allowed.`}
          status={
            unavailable
              ? "Remote access needs the remote gateway, which this host does not build."
              : !shareable.length
                ? "Open a built-in harness thread first: pairing happens through a shared thread."
                : pending
                  ? "Asking the gateway for a code…"
                  : expired
                    ? "The last code expired. Pair again for a fresh one."
                    : undefined
          }
        >
          <Action prominent disabled={!shareable.length || unavailable || busy !== null} onClick={pair}>
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
              // The request itself can wait for the gateway: say so from the press.
              pending={pending || busy === "pair"}
              error={pairingError}
              now={now}
              readAt={offerReading?.readAt ?? now}
              overdue={pending && pendingSince !== null && now - pendingSince > PAIR_DEADLINE_MS}
              busy={busy === "pair"}
              onRenew={pair}
              onStop={stop}
            />
          </Group>
        )
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
          open.map((thread) => {
            const reading = readings[thread.id]?.status;
            const isShared = reading?.shared === true;
            const attached = reading?.gateway?.attachedDevices ?? 0;
            const shares = thread.harness === "rustcode";
            return (
              <Row
                key={thread.id}
                title={thread.title}
                status={reading?.error ?? undefined}
                subtitle={
                  !shares
                    ? `Runs on ${thread.harness || "another harness"}, which keeps no ACP session: only threads on the built-in harness can be shared.`
                    : isShared
                      ? attached > 0
                        ? `${attached} device${attached === 1 ? "" : "s"} attached.`
                        : "Visible in the phone app; no device is attached right now."
                      : "Not shared with paired devices."
                }
              >
                {!shares ? (
                  <Chip>Not shareable</Chip>
                ) : isShared ? (
                  <>
                    <Chip tone="ok">Shared</Chip>
                    {attached > 0 && <Chip>{attached} attached</Chip>}
                  </>
                ) : (
                  <Action
                    disabled={unavailable || busy !== null}
                    onClick={() => void run(`start:${thread.id}`, () => start([thread.id], false))}
                  >
                    {busy === `start:${thread.id}` ? <Spinner /> : null}
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

      {external.length > 0 && shareable.length === 0 && (
        <p className="m-0 text-caption text-fg-faint">
          This app has no thread on the built-in harness open, so there is nothing for the remote gateway to
          share yet.
        </p>
      )}
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
 * camera, the address and eight-digit code for typing, and the countdown.
 * The gateway publishes no live wrong-codes count through ACP, so the page
 * states the fixed allowance and never counts a lockout down.
 */
function PairingCard({
  offer,
  pending,
  error,
  now,
  readAt,
  overdue,
  busy,
  onRenew,
  onStop,
}: {
  offer?: RemoteOffer;
  pending: boolean;
  error?: string;
  now: number;
  readAt: number;
  overdue: boolean;
  busy: boolean;
  onRenew: () => void;
  onStop: () => void;
}) {
  const left = offer ? Math.max(0, Math.ceil((readAt + offer.expiresInSecs * 1000 - now) / 1000)) : 0;
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
              <span>{WRONG_CODES_ALLOWED} wrong codes allowed</span>
              {offer.lockedForSecs !== undefined && <Chip tone="warn">Pairing locked</Chip>}
              {offer.loopbackOnly && <Chip tone="warn">This Mac only</Chip>}
              <Action quiet size="sm" disabled={busy} onClick={onRenew}>
                {busy ? <Spinner /> : <RefreshCw size={12} />}
                New code
              </Action>
            </div>
            {offer.lockedForSecs !== undefined && (
              <p className="m-0 pt-2 text-caption text-warning">
                Too many wrong codes on this host: pairing is locked, and even the right code is refused until
                the lockout ends.
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
        overdue ? (
          <div className="flex flex-col gap-3 text-callout text-fg-muted">
            <p className="m-0">
              The gateway did not answer in time. It may be starting, or another program holds it. Try again,
              or turn remote access off for this Mac.
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
