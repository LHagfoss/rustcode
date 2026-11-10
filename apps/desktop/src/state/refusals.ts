// A connection the gateway refused, in the words the owner acts on
// (docs/remote.md › Refused connections). The engine decides the reason and
// keeps the entry; this is the copy Settings › Remote access shows, so a phone
// whose only message is "cannot connect" is answered on the Mac.

import type { RemoteRefusedConnection } from "../acp/remote";

/** The noun for a kind of device, to build a sentence the owner reads. */
function subject(kind: RemoteRefusedConnection["deviceKind"]): string {
  switch (kind) {
    case "phone":
      return "phone";
    case "desktop":
      return "desktop app";
    case "cli":
      return "computer";
    default:
      return "device";
  }
}

/** `just now`, `3 min ago`: the gateway keeps an entry for a quarter of an hour. */
export function refusalAgo(seconds: number): string {
  if (seconds < 45) return "just now";
  const minutes = Math.round(seconds / 60);
  return minutes <= 1 ? "1 min ago" : `${minutes} min ago`;
}

/**
 * A refused attempt in the words the owner acts on: what happened, then what to
 * do about it. Every reason names something on this Mac, so a refusal is never
 * shown as only "a device could not connect".
 */
export function refusalCopy(refused: RemoteRefusedConnection): { title: string; detail: string } {
  const who = refused.deviceName ?? `A ${subject(refused.deviceKind)}`;
  switch (refused.reason) {
    case "unsupported_version": {
      const spoken = refused.supportedVersions.join(" and ") || "another one";
      const sent = refused.protocolVersion;
      const side =
        sent === undefined
          ? "a different app version"
          : sent < Number(refused.supportedVersions[0] ?? sent)
            ? "an older app version"
            : "a newer app version";
      return {
        title: `${who} tried to connect with ${side}`,
        detail:
          sent === undefined
            ? `This computer speaks remote protocol version ${spoken}. Update RustCode on the device or on this computer so both match, then connect again.`
            : `It speaks remote protocol version ${sent}, and this computer speaks version ${spoken}. Update RustCode on the device or on this computer so both match, then connect again.`,
      };
    }
    case "unauthorized":
      return {
        title: `${who} tried to connect with a pairing this computer does not accept`,
        detail:
          "Its pairing was revoked or has expired. Pair it again: Pair a device, then scan the code on the device or type it.",
      };
    case "token_replayed":
      return {
        title: `${who} used a key this computer had already replaced`,
        detail:
          "Two copies of the device's key were in use, so its pairing was removed to keep it safe. If that was your device, pair it again.",
      };
    case "pairing_failed":
      return {
        title: `${who} tried to pair with a code that did not work`,
        detail: "The code was wrong, already used, or had run out. Show a fresh code with Pair a device.",
      };
    case "rate_limited":
      return {
        title: "Pairing was refused: too many wrong codes on this computer",
        detail:
          "Pairing is locked for a few minutes, and even the right code is refused until then. Try again with a fresh code after that.",
      };
    case "unexpected_origin":
      return {
        title: "A device was refused before it could say what it was",
        detail:
          "Its app sent a browser-style origin, which this computer does not accept. That is a RustCode app from before this computer required the native app origin, or a web page trying the port. Update RustCode on the phone; nothing was shared either way.",
      };
    default:
      return {
        title: `${who} was refused`,
        detail: "This computer did not let the device in. Pair it again if it should have access.",
      };
  }
}
