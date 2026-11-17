import type { RemoteDevice, RemoteOffer, RemoteStatus } from "../acp/remote";

export interface PairingChallenge {
  offer: RemoteOffer;
  expiresAt: number;
  baselineIds: string[];
}
export function makeChallenge(
  offer: RemoteOffer,
  receivedAt: number,
  baselineIds: string[] = [],
): PairingChallenge {
  return { offer, expiresAt: receivedAt + offer.expiresInSecs * 1000, baselineIds };
}
export function activeChallenge(challenge: PairingChallenge | null, now: number): PairingChallenge | null {
  return challenge && now < challenge.expiresAt ? challenge : null;
}
export function remainingPairingSeconds(expiresAt: number, now: number): number {
  return Math.max(0, Math.ceil((expiresAt - now) / 1000));
}
export function newPairedDevice(baseline: string[], devices: RemoteDevice[]): RemoteDevice | undefined {
  return devices.find((device) => !baseline.includes(device.id));
}
export function challengeIsValid(challenge: PairingChallenge, status: RemoteStatus, now: number): boolean {
  const gateway = status.gateway;
  const currentId = gateway?.pairing?.challenge;
  const offerId = challenge.offer.challenge;
  return (
    now < challenge.expiresAt &&
    status.enabled &&
    status.available &&
    !status.error &&
    !status.pairingError &&
    !status.pairingExpired &&
    !!gateway?.connected &&
    gateway.pairingOpen !== false &&
    !(
      challenge.offer.instanceId &&
      gateway.instanceId &&
      challenge.offer.instanceId !== gateway.instanceId
    ) &&
    gateway.pairing?.attemptsLeft !== 0 &&
    (!status.offer || status.offer.qrPayload === challenge.offer.qrPayload) &&
    !(currentId !== undefined && offerId !== undefined && currentId !== offerId)
  );
}

export type PhonePairing =
  | { phase: "idle" }
  | { phase: "asking"; askedAt: number; baselineIds: string[]; inFlight: boolean }
  | { phase: "showing"; challenge: PairingChallenge }
  | { phase: "paired"; name: string }
  | { phase: "expired" }
  | { phase: "failed"; error: string };

export type PairingEvent =
  | { type: "ask"; now: number; baselineIds: string[] }
  | { type: "dismiss" | "off" }
  | { type: "tick"; now: number }
  | { type: "failure"; error: string }
  | { type: "status"; status: RemoteStatus; now: number; answer?: boolean };

/** The only interpretation of pairing status and actions. */
export function transitionPairing(state: PhonePairing, event: PairingEvent): PhonePairing {
  switch (event.type) {
    case "ask":
      return { phase: "asking", askedAt: event.now, baselineIds: event.baselineIds, inFlight: true };
    case "dismiss":
    case "off":
      return { phase: "idle" };
    case "tick":
      return state.phase === "showing" && event.now >= state.challenge.expiresAt
        ? { phase: "expired" }
        : state;
    case "failure":
      return { phase: "failed", error: event.error };
    case "status": {
      const { status, now } = event;
      // A poll while a request is in flight cannot finish that request.
      if (state.phase === "asking" && state.inFlight && !event.answer) return state;
      if (status.pairingError) return { phase: "failed", error: status.pairingError };
      if (!status.enabled) return { phase: "idle" };
      const baseline =
        state.phase === "asking"
          ? state.baselineIds
          : state.phase === "showing"
            ? state.challenge.baselineIds
            : null;
      const fresh = baseline && newPairedDevice(baseline, status.devices);
      if (fresh) return { phase: "paired", name: fresh.name };
      if (status.pairingExpired) return { phase: "expired" };
      if (state.phase === "asking" && event.answer && status.offer) {
        const challenge = makeChallenge(status.offer, now, state.baselineIds);
        return challengeIsValid(challenge, status, now) ? { phase: "showing", challenge } : { phase: "idle" };
      }
      if (state.phase === "showing") {
        if (now >= state.challenge.expiresAt) return { phase: "expired" };
        return challengeIsValid(state.challenge, status, now) ? state : { phase: "idle" };
      }
      if (status.pairingPending)
        return {
          phase: "asking",
          askedAt: state.phase === "asking" ? state.askedAt : now,
          baselineIds: status.devices.map((d) => d.id),
          inFlight: false,
        };
      return state.phase === "asking" ? { phase: "idle" } : state;
    }
  }
}
