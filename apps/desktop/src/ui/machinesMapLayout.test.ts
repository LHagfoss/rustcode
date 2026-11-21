import { expect, test } from "bun:test";
import type { FleetEntry } from "../state/fleet";
import type { Machine, MachineRoute } from "../state/machines";
import {
  buildMachinesGraph,
  layoutMachinesGraph,
  MAX_ROUTES,
  mapSummary,
  matchesQuery,
  wireState,
} from "./machinesMapLayout";

const lan: MachineRoute = { kind: "lan", address: "192.168.1.4:7777", encrypted: false };
const tail: MachineRoute = { kind: "tailscale", address: "100.64.0.4:7777", encrypted: true };

function machine(id: string, overrides: Partial<Machine> = {}): Machine {
  return {
    id,
    name: id,
    platform: "linux",
    version: "1.0.0",
    routes: [lan, tail],
    route: tail,
    state: "connected",
    detail: null,
    ...overrides,
  } as Machine;
}

function entry(hostId: string, via: string | null): FleetEntry {
  return {
    hostId,
    name: hostId,
    nameUpdatedAt: null,
    platform: "macos",
    version: null,
    lastSeenAt: null,
    via,
  };
}

test("the route a connected machine is on is in use, the rest stand by", () => {
  const studio = machine("studio");
  expect(wireState(studio, tail)).toBe("in-use");
  expect(wireState(studio, lan)).toBe("standby");
  expect(wireState({ ...studio, state: "connecting" }, tail)).toBe("connecting");
  expect(wireState({ ...studio, state: "offline" }, tail)).toBe("offline");
  expect(wireState(studio, { ...lan, off: true })).toBe("off");
});

test("routes sort in-use first and fold past the limit", () => {
  const many = Array.from({ length: MAX_ROUTES + 2 }, (_, index) => ({
    ...lan,
    address: `10.0.0.${index}:1`,
  }));
  const graph = buildMachinesGraph([machine("box", { routes: [...many, tail] })], []);
  expect(graph.machines[0]?.routes[0]?.state).toBe("in-use");
  expect(graph.machines[0]?.routes).toHaveLength(MAX_ROUTES);
  expect(graph.machines[0]?.hidden).toBe(3);
});

test("known machines link to the machine they came through, or stand alone", () => {
  const graph = buildMachinesGraph([machine("studio")], [entry("laptop", "studio"), entry("orphan", "gone")]);
  expect(graph.known.map((node) => node.viaId)).toEqual(["studio", null]);
  const layout = layoutMachinesGraph(graph);
  expect(layout.wires.filter((wire) => wire.state === "link").map((wire) => wire.id)).toEqual([
    "laptop:link",
  ]);
  expect(layout.wires.filter((wire) => wire.state !== "link")).toHaveLength(4);
  expect(layout.columns.map((column) => column.label)).toEqual([
    "This app",
    "Routes",
    "Paired machines",
    "Known through another",
  ]);
});

test("nodes never overlap within a column", () => {
  const graph = buildMachinesGraph([machine("a"), machine("b", { routes: [] }), machine("c")], []);
  const boxes = [...layoutMachinesGraph(graph).machines.values()].sort((l, r) => l.y - r.y);
  boxes.slice(1).forEach((box, index) => {
    const above = boxes[index];
    expect(box.y).toBeGreaterThanOrEqual((above?.y ?? 0) + (above?.h ?? 0));
  });
});

test("summary counts this computer, routes and known machines", () => {
  expect(mapSummary(buildMachinesGraph([], []))).toBe("1 machine, 0 routes");
  expect(mapSummary(buildMachinesGraph([machine("studio")], [entry("laptop", "studio")]))).toBe(
    "2 machines, 2 routes, 1 known through another",
  );
});

test("search matches names case-insensitively and an empty query matches all", () => {
  expect(matchesQuery("Studio Mac", "studio")).toBe(true);
  expect(matchesQuery("Studio Mac", "linux")).toBe(false);
  expect(matchesQuery("Studio Mac", "  ")).toBe(true);
});
