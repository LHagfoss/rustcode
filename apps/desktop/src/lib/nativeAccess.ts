// The two native checks the shell answers (src-tauri `microphone.rs`, `url_scheme.rs`):
// whether the app may use the microphone before dictation records, and whether this copy of the
// app is the one that opens `rustcode://` links. The pure rules are here, testable without the
// app; `ui/Dictation.tsx` and Settings › General show what they find.

import { isTauri } from "../acp/transport";
import { shellCall } from "./shell";

// ---------------------------------------------------------------- the microphone

/** The macOS authorization status; `unsupported` everywhere there is none to check. */
export type MicAccess = "granted" | "denied" | "restricted" | "not-determined" | "unsupported";

const MIC_ACCESS: readonly MicAccess[] = ["granted", "denied", "restricted", "not-determined", "unsupported"];

/** A status from the shell, or `unsupported` for anything it would not say. */
export function parseMicAccess(raw: unknown): MicAccess {
  return MIC_ACCESS.includes(raw as MicAccess) ? (raw as MicAccess) : "unsupported";
}

/** Whether recording would get nothing: the system refuses the microphone to the app. */
export function micBlocked(access: MicAccess): access is "denied" | "restricted" {
  return access === "denied" || access === "restricted";
}

/** What the composer says when the microphone is refused, and whether the settings button helps. */
export function micBlockedNotice(access: "denied" | "restricted"): {
  text: string;
  canOpenSettings: boolean;
} {
  if (access === "restricted")
    return {
      text: "This Mac's settings do not allow RustCode to use the microphone. Ask whoever manages it.",
      canOpenSettings: false,
    };
  return {
    text: "Microphone access is off for RustCode. Turn it on in System Settings › Privacy & Security › Microphone, then dictate again.",
    canOpenSettings: true,
  };
}

/**
 * The status, asking the user first when they have never been asked. Outside the app, and when the
 * shell cannot say, the page records as before and its own error explains a refusal.
 */
export async function checkMicrophone(): Promise<MicAccess> {
  if (!isTauri()) return "unsupported";
  try {
    return parseMicAccess(await shellCall<unknown>("microphone_access", { request: true }));
  } catch {
    return "unsupported";
  }
}

export function openMicrophoneSettings(): Promise<void> {
  return shellCall<void>("open_microphone_settings");
}

// ---------------------------------------------------------------- the link scheme

export type SchemeOwner = "this" | "other" | "none" | "unknown";

export interface SchemeStatus {
  owner: SchemeOwner;
  /** The app that opens the links instead, where the system says. */
  handler: string | null;
  canFix: boolean;
}

const OWNERS: readonly SchemeOwner[] = ["this", "other", "none", "unknown"];

export function parseSchemeStatus(raw: unknown): SchemeStatus {
  const value = (raw ?? {}) as Partial<Record<keyof SchemeStatus, unknown>>;
  const owner = OWNERS.includes(value.owner as SchemeOwner) ? (value.owner as SchemeOwner) : "unknown";
  return {
    owner,
    handler: typeof value.handler === "string" && value.handler ? value.handler : null,
    canFix: value.canFix === true,
  };
}

/**
 * The warning Settings shows, or null when links open here or it cannot be told (a development
 * build, a failed check): nothing is said unless something is wrong.
 */
export function schemeWarning(
  status: SchemeStatus,
): { title: string; text: string; fix: string | null } | null {
  if (status.owner === "this" || status.owner === "unknown") return null;
  const fix = status.canFix ? "Open Links Here" : null;
  const manual = status.canFix ? "" : " Quit the other copy, move it to the Trash, and open this one again.";
  if (status.owner === "none")
    return {
      title: "rustcode:// links do not open RustCode",
      text: `No app is registered for thread and prompt links.${manual}`,
      fix,
    };
  const where = status.handler
    ? `another copy of the app (${status.handler})`
    : "another app or an older copy";
  return {
    title: "rustcode:// links open in another copy",
    text: `Thread and prompt links open in ${where}, not this one.${manual}`,
    fix,
  };
}

/** This copy's ownership of the scheme; null outside the app or when the check fails. */
export async function readSchemeStatus(): Promise<SchemeStatus | null> {
  if (!isTauri()) return null;
  try {
    return parseSchemeStatus(await shellCall<unknown>("url_scheme_status"));
  } catch {
    return null;
  }
}

/** Makes this copy the one that opens the links; the status afterwards. */
export async function claimScheme(): Promise<SchemeStatus> {
  return parseSchemeStatus(await shellCall<unknown>("claim_url_scheme"));
}
