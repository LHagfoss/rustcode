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
