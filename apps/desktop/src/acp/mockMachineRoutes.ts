// The mock shell's edits to the saved machines: Forget, and the route edits (docs/remote.md › SSH
// routes) where an added route answers at once and the last route on stays on, as the shell's own
// `machines_remove` and `machines_route_*` commands do.

import type { JsonObject } from "./jsonrpc";

/** Apply one `machines_remove` or `machines_route_*` command to the mock's machine list and answer the new list. */
export function mockRouteCommand(
  command: string,
  args: Record<string, unknown>,
  machines: JsonObject[],
): JsonObject[] {
  let list = machines;
  if (command === "machines_remove") return list.filter((machine) => machine.id !== String(args.id));
  if (command === "machines_route_add") {
    const at = new Date().toISOString();
    const ssh = typeof args.ssh === "string" ? args.ssh.trim() : "";
    const address = typeof args.address === "string" ? args.address.trim() : "";
    if (!ssh && !address) throw new Error("Type an SSH host or an address.");
    const port = typeof args.gatewayPort === "number" ? args.gatewayPort : 17879;
    const route: JsonObject = ssh
      ? {
          kind: "ssh",
          address: `ssh://${ssh}`,
          provider: null,
          encrypted: true,
          off: false,
          manual: true,
          sshDestination: ssh,
          sshGatewayPort: port,
          probe: { outcome: "reachable", latencyMs: 180, detail: null, at },
        }
      : {
          kind: /^100\./.test(address) ? "vpn" : "lan",
          address,
          provider: null,
          encrypted: /^100\./.test(address),
          off: false,
          manual: true,
          probe: { outcome: "reachable", latencyMs: 4, detail: null, at },
        };
    list = list.map((machine) => {
      if (machine.id !== String(args.id)) return machine;
      const routes = (machine.routes as JsonObject[]).filter((saved) => saved.address !== route.address);
      return { ...machine, routes: [...routes, route] };
    });
    return list;
  }
  if (command === "machines_route_remove") {
    list = list.map((machine) =>
      machine.id === String(args.id)
        ? {
            ...machine,
            routes: (machine.routes as JsonObject[]).filter((route) => route.address !== args.address),
          }
        : machine,
    );
    return list;
  }
  if (command === "machines_route_set_off") {
    const off = args.off === true;
    const target = list.find((machine) => machine.id === String(args.id));
    const routes = (target?.routes as JsonObject[] | undefined) ?? [];
    if (off && !routes.some((route) => route.address !== args.address && route.off !== true)) {
      throw new Error("turn another route on first, or the machine cannot be reached");
    }
    list = list.map((machine) =>
      machine.id === String(args.id)
        ? {
            ...machine,
            routes: (machine.routes as JsonObject[]).map((route) =>
              route.address === args.address ? { ...route, off } : route,
            ),
          }
        : machine,
    );
    return list;
  }
  return list;
}
