/**
 * The Machines map (Settings › Machines › Map): this computer on the left, every route to a paired
 * machine as a labelled wire in the middle, the paired machines on the right, and the computers only
 * known through one of them past it. Pure functions over `state/machines.ts` and `state/fleet.ts`
 * values, so the graph and its layout are testable without a page.
 */

import type { FleetEntry } from "../state/fleet";
import { routeName } from "../state/machineLabels";
import type { Machine, MachineRoute } from "../state/machines";

export type WireState = "in-use" | "standby" | "connecting" | "offline" | "off";

export interface MapRoute {
  id: string;
  label: string;
  title: string;
  state: WireState;
}

export interface MapMachine {
  id: string;
  machine: Machine;
  routes: MapRoute[];
  hidden: number;
}

export interface MapKnown {
  id: string;
  entry: FleetEntry;
  viaId: string | null;
}

export interface MachinesGraph {
  machines: MapMachine[];
  known: MapKnown[];
}

/** At most this many route pills per machine; the rest fold into one `+N` pill. */
export const MAX_ROUTES = 3;

const sameRoute = (left: MachineRoute | null, right: MachineRoute) =>
  left !== null && left.kind === right.kind && left.address === right.address;

/** How one route's wire is drawn: the route the machine is on stands out, the rest stay quiet. */
export function wireState(machine: Pick<Machine, "state" | "route">, route: MachineRoute): WireState {
  if (route.off) return "off";
  const current = sameRoute(machine.route, route);
  if (machine.state === "connected") return current ? "in-use" : "standby";
  if (machine.state === "connecting" && current) return "connecting";
  return "offline";
}

const STATE_WORDS: Record<WireState, string> = {
  "in-use": "In use",
  standby: "Standby",
  connecting: "Connecting",
  offline: "Not connected",
  off: "Turned off",
};

export function wireStateLabel(state: WireState): string {
  return STATE_WORDS[state];
}

function mapRoute(machine: Machine, route: MachineRoute, index: number): MapRoute {
  const state = wireState(machine, route);
  const latency =
    state === "in-use" && route.probe?.latencyMs != null ? ` · ${route.probe.latencyMs} ms` : "";
  return {
    id: `${machine.id}:${index}`,
    label: routeName(route),
    title: `${route.address} · ${route.encrypted ? "Encrypted" : "Unencrypted"} · ${wireStateLabel(state)}${latency}`,
    state,
  };
}

const RANK: Record<WireState, number> = { "in-use": 0, connecting: 1, standby: 2, offline: 3, off: 4 };

export function buildMachinesGraph(
  machines: readonly Machine[],
  known: readonly FleetEntry[],
): MachinesGraph {
  const paired = new Set(machines.map((machine) => machine.id));
  const knownIds = new Set(known.map((entry) => entry.hostId));
  return {
    machines: machines.map((machine) => {
      const routes = machine.routes
        .map((route, index) => mapRoute(machine, route, index))
        .sort((left, right) => RANK[left.state] - RANK[right.state]);
      return {
        id: machine.id,
        machine,
        routes: routes.slice(0, MAX_ROUTES),
        hidden: Math.max(0, routes.length - MAX_ROUTES),
      };
    }),
    known: known.map((entry) => ({
      id: entry.hostId,
      entry,
      viaId: entry.via && (paired.has(entry.via) || knownIds.has(entry.via)) ? entry.via : null,
    })),
  };
}

export function mapSummary(graph: MachinesGraph): string {
  const routes = graph.machines.reduce((sum, node) => sum + node.routes.length + node.hidden, 0);
  const parts = [
    plural(graph.machines.length + 1, "machine"),
    plural(routes, "route"),
    graph.known.length > 0 ? `${graph.known.length} known through another` : null,
  ];
  return parts.filter(Boolean).join(", ");
}

const plural = (count: number, noun: string) => `${count} ${noun}${count === 1 ? "" : "s"}`;

export function matchesQuery(name: string, query: string): boolean {
  const needle = query.trim().toLowerCase();
  return needle === "" || name.toLowerCase().includes(needle);
}

export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Wire {
  id: string;
  d: string;
  state: WireState | "link";
}

export interface MachinesLayout {
  width: number;
  height: number;
  self: Box;
  pills: Map<string, Box>;
  machines: Map<string, Box>;
  known: Map<string, Box>;
  wires: Wire[];
  columns: { label: string; x: number }[];
}

const PAD = 24;
const HEADER = 28;
const SELF = { w: 168, h: 56 };
const PILL = { w: 116, h: 24, gap: 12 };
const MACHINE = { w: 220, h: 56, gap: 20 };
const KNOWN = { w: 200, h: 56, gap: 12 };
const X = {
  self: PAD,
  pill: PAD + SELF.w + 56,
  machine: PAD + SELF.w + 56 + PILL.w + 64,
  known: PAD + SELF.w + 56 + PILL.w + 64 + MACHINE.w + 72,
};

const curve = (x1: number, y1: number, x2: number, y2: number) => {
  const mid = (x1 + x2) / 2;
  return `M${x1},${y1} C${mid},${y1} ${mid},${y2} ${x2},${y2}`;
};

const centerY = (box: Box) => box.y + box.h / 2;

/** Lays the graph out left to right in fixed logical units; the canvas scales it to fit. */
export function layoutMachinesGraph(graph: MachinesGraph): MachinesLayout {
  const pills = new Map<string, Box>();
  const machines = new Map<string, Box>();
  const known = new Map<string, Box>();
  const wires: Wire[] = [];
  const top = PAD + HEADER;

  let y = top;
  for (const node of graph.machines) {
    const count = Math.max(1, node.routes.length + (node.hidden > 0 ? 1 : 0));
    const block = Math.max(MACHINE.h, count * PILL.h + (count - 1) * PILL.gap);
    machines.set(node.id, { x: X.machine, y: y + (block - MACHINE.h) / 2, w: MACHINE.w, h: MACHINE.h });
    for (let index = 0; index < count; index++) {
      const id = node.routes[index]?.id ?? `${node.id}:more`;
      const pillsHeight = count * PILL.h + (count - 1) * PILL.gap;
      pills.set(id, {
        x: X.pill,
        y: y + (block - pillsHeight) / 2 + index * (PILL.h + PILL.gap),
        w: PILL.w,
        h: PILL.h,
      });
    }
    y += block + MACHINE.gap;
  }
  const machinesBottom = graph.machines.length > 0 ? y - MACHINE.gap : top + SELF.h;

  const placedBelow = new Map<string, number>();
  let knownY = top;
  for (const node of graph.known) {
    const anchor = node.viaId ? (machines.get(node.viaId) ?? known.get(node.viaId)) : undefined;
    const wanted = anchor ? Math.max(anchor.y, placedBelow.get(node.viaId ?? "") ?? anchor.y) : knownY;
    const at = Math.max(wanted, knownY);
    known.set(node.id, { x: X.known, y: at, w: KNOWN.w, h: KNOWN.h });
    knownY = at + KNOWN.h + KNOWN.gap;
    if (node.viaId) placedBelow.set(node.viaId, knownY);
  }
  const knownBottom = graph.known.length > 0 ? knownY - KNOWN.gap : top;

  const contentBottom = Math.max(machinesBottom, knownBottom);
  const self: Box = {
    x: X.self,
    y: top + (contentBottom - top - SELF.h) / 2,
    w: SELF.w,
    h: SELF.h,
  };

  for (const node of graph.machines) {
    const target = machines.get(node.id);
    if (!target) continue;
    const routes: { id: string; state: WireState }[] = node.routes.map((route) => ({
      id: route.id,
      state: route.state,
    }));
    if (node.hidden > 0) routes.push({ id: `${node.id}:more`, state: "standby" });
    if (routes.length === 0) {
      wires.push({
        id: `${node.id}:none`,
        d: curve(self.x + self.w, centerY(self), target.x, centerY(target)),
        state: "offline",
      });
      continue;
    }
    for (const route of routes) {
      const pill = pills.get(route.id);
      if (!pill) continue;
      wires.push({
        id: `${route.id}:in`,
        d: curve(self.x + self.w, centerY(self), pill.x, centerY(pill)),
        state: route.state,
      });
      wires.push({
        id: `${route.id}:out`,
        d: curve(pill.x + pill.w, centerY(pill), target.x, centerY(target)),
        state: route.state,
      });
    }
  }

  for (const node of graph.known) {
    const target = known.get(node.id);
    const from = node.viaId ? (machines.get(node.viaId) ?? known.get(node.viaId)) : undefined;
    if (!target || !from) continue;
    const middle = from.x + from.w / 2;
    const d =
      from.x !== X.known
        ? curve(from.x + from.w, centerY(from), target.x - 4, centerY(target))
        : from.y < target.y
          ? `M${middle},${from.y + from.h} L${middle},${target.y - 4}`
          : `M${middle},${from.y} L${middle},${target.y + target.h + 4}`;
    wires.push({ id: `${node.id}:link`, d, state: "link" });
  }

  const hasKnown = graph.known.length > 0;
  const columns = [
    { label: "This app", x: X.self },
    ...(graph.machines.length > 0 ? [{ label: "Routes", x: X.pill }] : []),
    ...(graph.machines.length > 0 ? [{ label: "Paired machines", x: X.machine }] : []),
    ...(hasKnown ? [{ label: "Known through another", x: X.known }] : []),
  ];
  const right = hasKnown
    ? X.known + KNOWN.w
    : graph.machines.length > 0
      ? X.machine + MACHINE.w
      : X.self + SELF.w;
  return {
    width: right + PAD,
    height: Math.max(contentBottom, self.y + self.h) + PAD,
    self,
    pills,
    machines,
    known,
    wires,
    columns,
  };
}
