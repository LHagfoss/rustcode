import type { RemoteDevice, RemoteOffer } from "../acp/remote";

export interface PairingChallenge {
  offer: RemoteOffer;
  expiresAt: number;
}
export function makeChallenge(offer: RemoteOffer, receivedAt: number): PairingChallenge {
  return { offer, expiresAt: receivedAt + offer.expiresInSecs * 1000 };
}
export function activeChallenge(challenge: PairingChallenge | null, now: number): PairingChallenge | null {
  return challenge && now < challenge.expiresAt ? challenge : null;
}
export function newPairedDevice(baseline: string[], devices: RemoteDevice[]): RemoteDevice | undefined {
  return devices.find((device) => !baseline.includes(device.id));
}
