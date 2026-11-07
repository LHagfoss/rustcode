// Timed approvals ("Allow for 1 hour"): the engine keeps them with the other
// user approvals and answers `_rustcode/permissions` with the ones still in
// force. This file reads that answer and says how long is left, so the thread
// details can show "Allowed for 42m · Revoke". Pure.

import { arr, num, obj, str } from "../acp/jsonrpc";

export interface TimedGrant {
  /** The engine's own id for the rule, what a revoke names. */
  id: string;
  /** What it allows, as the user sees it: "cargo test --lib". */
  prefix: string;
  /** Unix seconds when it stops covering calls. */
  expiresAt: number;
  /** Seconds left when the engine answered. */
  remainingSecs: number;
}

/** `_rustcode/permissions` as `{grants: [...]}`. */
export function parseGrants(value: unknown): TimedGrant[] {
  const root = obj(value);
  return arr(root?.grants).flatMap((item) => {
    const grant = obj(item);
    const id = str(grant?.id);
    const prefix = str(grant?.prefix);
    const expiresAt = num(grant?.expiresAt);
    const remainingSecs = num(grant?.remainingSecs);
    if (!id || !prefix || expiresAt === undefined) return [];
    return [{ id, prefix, expiresAt, remainingSecs: remainingSecs ?? 0 }];
  });
}

/** How long a grant still allows: "42m", "58s", "1h 5m" once it runs longer than an hour (never). */
export function grantRemaining(grant: TimedGrant, now = Date.now()): string {
  const seconds = Math.max(0, grant.expiresAt - Math.floor(now / 1000));
  if (seconds <= 0) return "expired";
  const minutes = Math.floor(seconds / 60);
  if (minutes >= 1) return `${minutes}m`;
  return `${seconds}s`;
}

/** Whether a grant is still in force, so a list does not show a stale row. */
export function grantActive(grant: TimedGrant, now = Date.now()): boolean {
  return grant.expiresAt > Math.floor(now / 1000);
}

/** The grants still in force, soonest to end first. */
export function activeGrants(grants: readonly TimedGrant[], now = Date.now()): TimedGrant[] {
  return grants.filter((grant) => grantActive(grant, now)).sort((a, b) => a.expiresAt - b.expiresAt);
}
