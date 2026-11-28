// Settings › Machines › Hub: this machine's role in the fleet's hub
// (docs/remote.md › The hub). The shell runs `rc machines hub …`
// (`src-tauri/src/hub.rs`); this keeps what it answers.

import { create } from "zustand";
import { OUTSIDE_SCOPES, type OutsideScope } from "../acp/outside";
import { hasShell, shellCallWithin } from "../lib/shell";
import { errorText, toast } from "./store";

/** The CLI's own limit is 20 s; this waits a little longer for its answer. */
const HUB_CALL_MS = 25_000;

/** The hub this machine joined, and what it may do here. */
export interface HubMembership {
  /** The hub's host id, as Settings › Machines lists it. */
  machine: string;
  name: string;
  access: OutsideScope;
  ask: boolean;
}

/** A machine that joined this hub, as the hub's gateway reports it. */
export interface HubMember {
  hostId: string;
  name: string;
  access: OutsideScope;
  ask: boolean;
  online: boolean;
  lastSeenAt: string | null;
  queued: number;
}

export interface HubStatus {
  hub: boolean;
  /** This hub also runs a relay its members register with. */
  relay: boolean;
  member: HubMembership | null;
  /** Null unless this is the hub and its gateway answers. */
  members: HubMember[] | null;
}

/**
 * This machine's hub role as one value (issue 4338). `transitionHub` is the one way between
 * phases:
 *
 * - `unread`: nothing answered yet.
 * - `failed`: nothing answered yet and the last read failed. A failed read never replaces a
 *   role that is on screen.
 * - `read`: the role, as the last read or change answered.
 * - `changing`: a change is out; the role shown is the one from before it.
 */
export type HubRole =
  | { phase: "unread" }
  | { phase: "failed"; error: string }
  | { phase: "read"; status: HubStatus }
  | { phase: "changing"; status: HubStatus };

/**
 * `epoch` counts the starts and ends of changes. A read carries the epoch it left at, and one
 * that left before or during a change is dropped: its answer describes the role from before.
 */
export interface HubState {
  role: HubRole;
  epoch: number;
}

export type HubEvent =
  | { type: "read"; epoch: number; status: HubStatus }
  | { type: "readFailed"; epoch: number; error: string }
  | { type: "change" }
  | { type: "changed"; status: HubStatus }
  | { type: "changeFailed" };

export const HUB_UNREAD: HubState = { role: { phase: "unread" }, epoch: 0 };

/** The pure step between hub phases; an event that does not apply answers `state` itself. */
export function transitionHub(state: HubState, event: HubEvent): HubState {
  const { role, epoch } = state;
  switch (event.type) {
    case "read":
      return event.epoch === epoch && role.phase !== "changing"
        ? { role: { phase: "read", status: event.status }, epoch }
        : state;
    case "readFailed":
      return event.epoch === epoch && (role.phase === "unread" || role.phase === "failed")
        ? { role: { phase: "failed", error: event.error }, epoch }
        : state;
    case "change":
      return role.phase === "read"
        ? { role: { phase: "changing", status: role.status }, epoch: epoch + 1 }
        : state;
    case "changed":
      return role.phase === "changing"
        ? { role: { phase: "read", status: event.status }, epoch: epoch + 1 }
        : state;
    case "changeFailed":
      return role.phase === "changing"
        ? { role: { phase: "read", status: role.status }, epoch: epoch + 1 }
        : state;
  }
}

export const hubStatus = (state: HubState): HubStatus | null =>
  state.role.phase === "read" || state.role.phase === "changing" ? state.role.status : null;

/** Why the role could not be read, while there is none to show. */
export const hubError = (state: HubState): string | null =>
  state.role.phase === "failed" ? state.role.error : null;

export const hubChanging = (state: HubState): boolean => state.role.phase === "changing";

export const useHub = create<HubState>(() => HUB_UNREAD);

function send(event: HubEvent): boolean {
  const before = useHub.getState();
  const after = transitionHub(before, event);
  if (after === before) return false;
  useHub.setState(after, true);
  return true;
}

function scope(value: unknown): OutsideScope {
  return OUTSIDE_SCOPES.find((level) => level === value) ?? "read_only";
}

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

/** `machines hub status --json`, in the app's shape. */
export function parseHubStatus(value: unknown): HubStatus {
  const raw = record(value) ?? {};
  const member = record(raw.member);
  const members = Array.isArray(raw.members) ? raw.members : null;
  return {
    hub: raw.hub === true,
    relay: raw.relay === true,
    member:
      member && typeof member.machine === "string"
        ? {
            machine: member.machine,
            name: typeof member.name === "string" ? member.name : member.machine,
            access: scope(member.access),
            ask: member.ask !== false,
          }
        : null,
    members:
      members?.flatMap((entry) => {
        const item = record(entry);
        if (!item || typeof item.host_id !== "string") return [];
        return [
          {
            hostId: item.host_id,
            name: typeof item.name === "string" ? item.name : item.host_id,
            access: scope(item.access),
            ask: item.ask === true,
            online: item.online === true,
            lastSeenAt: typeof item.last_seen_at === "string" ? item.last_seen_at : null,
            queued: typeof item.queued === "number" ? item.queued : 0,
          },
        ];
      }) ?? null,
  };
}

export async function refreshHub(): Promise<void> {
  if (!hasShell()) return;
  const { epoch } = useHub.getState();
  try {
    const status = parseHubStatus(await shellCallWithin<unknown>(HUB_CALL_MS, "hub_status"));
    send({ type: "read", epoch, status });
  } catch (error) {
    send({ type: "readFailed", epoch, error: errorText(error) });
  }
}

export type HubChange =
  | { action: "on" | "off" | "leave" | "relay_on" | "relay_off" }
  | { action: "join"; machine: string; access: OutsideScope; ask: boolean };

/** Change the role; answers whether it changed, and toasts why not. */
export async function setHub(change: HubChange): Promise<boolean> {
  if (!send({ type: "change" })) return false;
  try {
    const status = parseHubStatus(await shellCallWithin<unknown>(HUB_CALL_MS, "hub_set", { ...change }));
    send({ type: "changed", status });
    return true;
  } catch (error) {
    send({ type: "changeFailed" });
    toast(errorText(error), "error");
    return false;
  }
}
