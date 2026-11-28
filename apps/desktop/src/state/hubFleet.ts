import { obj, str } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { ownHostId } from "./fleet";
import { type HubChange, refreshHub, setHub } from "./hub";
import { LOCAL_MACHINE_ID, type Machine, machineName, useMachines } from "./machines";
import { errorText, toast } from "./store";
import { connectionFor } from "./threadMachines";

const EXEC_SECS = 30;

/** What one machine is once the fleet's hub is chosen: the hub, linked to it, or neither. */
export type FleetHubRole = { kind: "hub" } | { kind: "member"; hub: string } | { kind: "none" };

/** The role `machine` takes when `hub` is the fleet's hub, or none is. */
export function roleFor(machine: string, hub: string | null): FleetHubRole {
  if (hub === null) return { kind: "none" };
  return machine === hub ? { kind: "hub" } : { kind: "member", hub };
}

/**
 * The hub changes that put a machine in `role`, in order. A hub links to no other hub, and a
 * member joins at Full access without asking: these are the person's own machines.
 */
export function roleChanges(role: FleetHubRole): HubChange[] {
  switch (role.kind) {
    case "hub":
      return [{ action: "leave" }, { action: "on" }];
    case "member":
      return [{ action: "off" }, { action: "join", machine: role.hub, access: "admin", ask: false }];
    case "none":
      return [{ action: "off" }, { action: "leave" }];
  }
}

/** The same changes as one `rc machines hub` command line, for another machine to run. */
export function roleCommand(role: FleetHubRole): string {
  return roleChanges(role)
    .map((change) =>
      change.action === "join"
        ? `rc machines hub join "${change.machine}" --access ${change.access} --ask ${change.ask}`
        : `rc machines hub ${change.action}`,
    )
    .join(" && ");
}

/** The machines a hub choice reaches now: connected, so their engine can run the change. */
export function reachableMachines(machines: Machine[]): Machine[] {
  return machines.filter((machine) => machine.state === "connected" && connectionFor(machine.id) !== null);
}

async function applyLocal(role: FleetHubRole): Promise<boolean> {
  for (const change of roleChanges(role)) {
    if (!(await setHub(change))) return false;
  }
  return true;
}

async function applyRemote(machine: Machine, role: FleetHubRole): Promise<void> {
  const connection = connectionFor(machine.id);
  if (!connection) throw new Error("it is not connected");
  const result = obj(
    await connection.request(
      Method.machineExec,
      { command: roleCommand(role), timeoutSecs: EXEC_SECS },
      (EXEC_SECS + 15) * 1000,
    ),
  );
  if (result?.success === true) return;
  const output = str(result?.output) ?? "";
  if (/unrecognized subcommand/i.test(output)) throw new Error("its RustCode is too old; update it there");
  throw new Error(output.trim().split("\n").filter(Boolean).pop() ?? "the change failed there");
}

let changing = false;

/**
 * Make `hub` the fleet's hub (`LOCAL_MACHINE_ID` for this computer), or end the hub with null:
 * the hub turns it on, and this computer and every connected machine link to it. A machine that
 * is offline keeps its old role until it is chosen again.
 */
export async function chooseHub(hub: string | null): Promise<boolean> {
  if (changing) return false;
  changing = true;
  try {
    const machines = reachableMachines(useMachines.getState().machines);
    const hubMachine = hub && hub !== LOCAL_MACHINE_ID ? machines.find((m) => m.id === hub) : undefined;
    if (hub && hub !== LOCAL_MACHINE_ID && !hubMachine) {
      toast("That machine is not connected. Connect to it first, then make it the hub.", "error");
      return false;
    }
    const hubId = hub === LOCAL_MACHINE_ID ? ((await ownHostId()) ?? null) : hub;
    if (hub === LOCAL_MACHINE_ID && !hubId) {
      toast("Turn on remote access here first: the other machines link to it.", "error");
      return false;
    }
    try {
      if (hubMachine) await applyRemote(hubMachine, { kind: "hub" });
    } catch (error) {
      toast(`The hub did not change: ${errorText(error)}`, "error");
      return false;
    }
    if (!(await applyLocal(hub === LOCAL_MACHINE_ID ? { kind: "hub" } : roleFor(LOCAL_MACHINE_ID, hub))))
      return false;
    const failed: string[] = [];
    for (const machine of machines) {
      if (machine.id === hub) continue;
      try {
        await applyRemote(machine, roleFor(machine.id, hubId));
      } catch (error) {
        failed.push(`${machineName(machine)} (${errorText(error)})`);
      }
    }
    await refreshHub();
    const name = hubMachine ? machineName(hubMachine) : "This computer";
    const done =
      hub === null ? "No machine is the hub now." : `${name} is the hub; your connected machines link to it.`;
    if (failed.length) toast(`${done} Not changed: ${failed.join(", ")}.`, "error");
    else toast(done, "success");
    return true;
  } finally {
    changing = false;
  }
}

/** The machine to suggest as the hub: one that likely stays on, such as a Linux server. */
export function suggestedHub(machines: Machine[]): Machine | null {
  return reachableMachines(machines).find((machine) => machine.platform === "linux") ?? null;
}
