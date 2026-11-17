// Remote access: this computer's remote server, the devices paired with it, the QR challenge a
// pairing shows and the connect requests waiting on an answer. Split out of state/store.ts; the
// state it writes (`remote`, the challenge, the requests) still lives in the app store.

import type { JsonObject } from "../acp/jsonrpc";
import { parseRemoteStatus, RemoteMethod, type RemoteStatus } from "../acp/remote";
import { transitionPairing } from "./remotePairing";
import { errorText, request, toast, useApp } from "./store";

/** Ignore machine-status reads that started before a newer answer changed remote state. */
let remoteStatusRevision = 0;
let remoteStatusReadId = 0;
let latestRemoteStatusReadId = 0;

/** Test hook: every read starts again on a clean clock. */
export function resetRemoteStatusReads(): void {
  remoteStatusRevision += 1;
  remoteStatusReadId = 0;
  latestRemoteStatusReadId = 0;
}

// Every read of the machine's remote state runs on one clock. A change made here (pair, answer,
// revoke, the switch) or a dismissed QR code bumps the revision, so a read that started before it
// cannot put back what it changed; among the reads that started after, only the newest lands. All
// of them, and every mutation's answer, land through `applyRemoteStatus`, which keeps or clears the
// QR challenge and notices a device that just paired.
function invalidateRemoteStatusReads() {
  remoteStatusRevision += 1;
  latestRemoteStatusReadId = remoteStatusReadId;
}

function commitRemoteStatus(status: RemoteStatus) {
  invalidateRemoteStatusReads();
  applyRemoteStatus(status);
}

export function dismissRemoteChallenge() {
  invalidateRemoteStatusReads();
  useApp.setState((s) => ({ pairing: transitionPairing(s.pairing, { type: "dismiss" }) }));
}

export function applyRemoteStatus(status: RemoteStatus, now = Date.now()) {
  useApp.setState((s) => {
    return {
      remote: status,
      remoteError: null,
      pairing: transitionPairing(s.pairing, { type: "status", status, now }),
    };
  });
}

/**
 * Re-read the machine's remote access state. `sessionId` reads one thread's
 * share instead, for its header indicator. Never throws: Settings shows the
 * engine's refusal next to the switch.
 */
export async function refreshRemote(sessionId?: string): Promise<RemoteStatus | null> {
  const params: JsonObject = sessionId ? { sessionId } : {};
  const revision = remoteStatusRevision;
  const readId = sessionId ? 0 : ++remoteStatusReadId;
  try {
    const status = parseRemoteStatus(await request(RemoteMethod.status, params, 10_000));
    if (sessionId)
      useApp.setState((s) => ({
        remoteByThread: { ...s.remoteByThread, [sessionId]: status },
        remoteError: null,
      }));
    else if (revision === remoteStatusRevision && readId > latestRemoteStatusReadId) {
      latestRemoteStatusReadId = readId;
      applyRemoteStatus(status);
    }
    return sessionId || (revision === remoteStatusRevision && readId === latestRemoteStatusReadId)
      ? status
      : useApp.getState().remote;
  } catch (error) {
    // The engine answered nothing (a restart, a dropped request): say so and
    // keep what was last true, rather than showing remote access as off.
    if (sessionId || (revision === remoteStatusRevision && readId > latestRemoteStatusReadId)) {
      if (!sessionId) latestRemoteStatusReadId = readId;
      useApp.setState({ remoteError: errorText(error) });
    }
    return null;
  }
}

/**
 * Switch remote access on or off for every thread on this Mac. On starts the
 * gateway and shares every open thread; off stops the gateway, so no device
 * sees anything until it is switched on again. Pairings are kept.
 */
export async function setRemoteAccess(on: boolean): Promise<void> {
  invalidateRemoteStatusReads();
  if (!on) useApp.setState((s) => ({ pairing: transitionPairing(s.pairing, { type: "off" }) }));
  if (on) {
    useApp.setState({
      remote: {
        ...(useApp.getState().remote ?? {}),
        available: true,
        enabled: true,
        shared: false,
        pairedDevices: 0,
        devices: [],
      } as RemoteStatus,
    });
  }
  try {
    const status = parseRemoteStatus(await request(on ? RemoteMethod.start : RemoteMethod.stop, {}, 20_000));
    commitRemoteStatus(status);
    if (status.error) toast(status.error, "error");
    else {
      toast(
        on
          ? "Remote access is on. Every open thread is shared with paired devices."
          : "Remote access is off. Paired devices stay paired.",
        "success",
      );
    }
    // Every open thread's share changed with it.
    for (const threadId of Object.keys(useApp.getState().remoteByThread)) void refreshRemote(threadId);
  } catch (error) {
    toast(errorText(error), "error");
    void refreshRemote();
  }
}

/**
 * Ask the gateway for a pairing challenge. Remote access is switched on first
 * if it was off, as `/remote` does in a terminal.
 */
export async function pairRemoteDevice(): Promise<RemoteStatus | null> {
  const baselineIds = (useApp.getState().remote?.devices ?? []).map((device) => device.id);
  invalidateRemoteStatusReads();
  const revision = remoteStatusRevision;
  useApp.setState((s) => ({
    pairing: transitionPairing(s.pairing, { type: "ask", now: Date.now(), baselineIds }),
  }));
  try {
    const status = parseRemoteStatus(await request(RemoteMethod.pair, {}, 20_000));
    if (revision === remoteStatusRevision) {
      useApp.setState((s) => ({
        pairing: transitionPairing(s.pairing, { type: "status", status, now: Date.now(), answer: true }),
      }));
      commitRemoteStatus(status);
    } else {
      // Dismissed, switched off or answered meanwhile: the code is not shown, but the machine's
      // state did change, so read it again.
      void refreshRemote();
    }
    const pairing = useApp.getState().pairing;
    if (pairing.phase === "failed") toast(pairing.error, "error");
    return status;
  } catch (error) {
    if (revision === remoteStatusRevision)
      useApp.setState((s) => ({
        pairing: transitionPairing(s.pairing, { type: "failure", error: errorText(error) }),
      }));
    toast(errorText(error), "error");
    return null;
  }
}

/**
 * Accept or deny a phone that asked to connect. Accepting shows the code the phone needs
 * (`connectCode`); the request's own prompt goes with the next status either way.
 */
export async function answerConnectRequest(id: string, accept: boolean): Promise<boolean> {
  invalidateRemoteStatusReads();
  try {
    const status = parseRemoteStatus(await request(RemoteMethod.answer, { id, accept }, 15_000));
    commitRemoteStatus(status);
    if (status.connectError) {
      toast(status.connectError, "error");
      return false;
    }
    const answer = status.connectAnswer;
    if (accept && answer?.code) {
      useApp.setState({
        connectCode: {
          requestId: answer.id,
          deviceName: answer.deviceName,
          code: answer.code,
          instanceId: answer.instanceId ?? status.gateway?.instanceId,
          expiresAt: Date.now() + (answer.expiresInSecs ?? 120) * 1000,
          lockedForSecs: answer.lockedForSecs,
        },
      });
    }
    return true;
  } catch (error) {
    toast(errorText(error), "error");
    return false;
  }
}

/** Take the connection code off the screen. */
export function closeConnectCode() {
  useApp.setState({ connectCode: null });
}

/** Forget a paired device: its token stops working and its connections close. */
export async function revokeRemoteDevice(id: string): Promise<void> {
  invalidateRemoteStatusReads();
  try {
    const status = parseRemoteStatus(await request(RemoteMethod.revoke, { deviceId: id }, 15_000));
    commitRemoteStatus(status);
    if (status.revokeError) toast(status.revokeError, "error");
    else if (status.revoked) toast(`${status.revoked.name} can no longer connect.`, "success");
  } catch (error) {
    toast(errorText(error), "error");
  }
}

/** Advance the visible challenge deadline without invalidating newer reads. */
export function tickRemotePairing(now: number) {
  const pairing = useApp.getState().pairing;
  const next = transitionPairing(pairing, { type: "tick", now });
  if (next !== pairing) {
    invalidateRemoteStatusReads();
    useApp.setState({ pairing: next });
  }
}
