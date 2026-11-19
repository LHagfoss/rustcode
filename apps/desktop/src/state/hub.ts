// Settings › Machines › Hub: this machine's role in the fleet's hub
// (docs/remote.md › The hub). The shell runs `rustcode machines hub …`
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
  member: HubMembership | null;
  /** Null unless this is the hub and its gateway answers. */
  members: HubMember[] | null;
}

interface HubState {
  status: HubStatus | null;
  error: string | null;
  busy: boolean;
}

export const useHub = create<HubState>(() => ({ status: null, error: null, busy: false }));

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
  try {
    const status = parseHubStatus(await shellCallWithin<unknown>(HUB_CALL_MS, "hub_status"));
    useHub.setState({ status, error: null });
  } catch (error) {
    useHub.setState({ error: errorText(error) });
  }
}

export type HubChange =
  | { action: "on" | "off" | "leave" }
  | { action: "join"; machine: string; access: OutsideScope; ask: boolean };

/** Change the role; answers whether it changed, and toasts why not. */
export async function setHub(change: HubChange): Promise<boolean> {
  if (useHub.getState().busy) return false;
  useHub.setState({ busy: true });
  try {
    const status = parseHubStatus(await shellCallWithin<unknown>(HUB_CALL_MS, "hub_set", { ...change }));
    useHub.setState({ status, error: null });
    return true;
  } catch (error) {
    toast(errorText(error), "error");
    return false;
  } finally {
    useHub.setState({ busy: false });
  }
}
