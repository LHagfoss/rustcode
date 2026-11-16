// A saved machine's routes, edited by hand (docs/remote.md › SSH routes): add an SSH route or an
// address, turn one off, remove one. Each answers the shell's refreshed machine list.

import { shellCallWithin } from "../lib/shell";
import { type Machine, PROBE_MS, SHELL_READ_MS, useMachines } from "./machines";
import { errorText, toast } from "./store";

/** What Add route sends: an SSH host, or a network address. */
export type NewRoute =
  | { kind: "ssh"; destination: string; gatewayPort?: number | null }
  | { kind: "address"; address: string };

/**
 * Add a route to a saved machine and test it. Answers the error to show beside the form, or null
 * when it was added.
 */
export async function addMachineRoute(machine: Pick<Machine, "id">, route: NewRoute): Promise<string | null> {
  try {
    const machines = await shellCallWithin<Machine[]>(
      PROBE_MS,
      "machines_route_add",
      route.kind === "ssh"
        ? {
            id: machine.id,
            ssh: route.destination.trim(),
            sshPort: null,
            identity: null,
            gatewayPort: route.gatewayPort ?? null,
            address: null,
          }
        : {
            id: machine.id,
            ssh: null,
            sshPort: null,
            identity: null,
            gatewayPort: null,
            address: route.address.trim(),
          },
    );
    useMachines.setState({ machines });
    return null;
  } catch (error) {
    return errorText(error);
  }
}

/** Remove one route of a machine. */
export async function removeMachineRoute(machine: Pick<Machine, "id">, address: string): Promise<void> {
  try {
    const machines = await shellCallWithin<Machine[]>(SHELL_READ_MS, "machines_route_remove", {
      id: machine.id,
      address,
    });
    useMachines.setState({ machines });
  } catch (error) {
    toast(errorText(error), "error");
  }
}

/** Turn one route of a machine on or off; at least one stays on. */
export async function setMachineRouteOff(
  machine: Pick<Machine, "id">,
  address: string,
  off: boolean,
): Promise<void> {
  try {
    const machines = await shellCallWithin<Machine[]>(SHELL_READ_MS, "machines_route_set_off", {
      id: machine.id,
      address,
      off,
    });
    useMachines.setState({ machines });
  } catch (error) {
    toast(errorText(error), "error");
  }
}
