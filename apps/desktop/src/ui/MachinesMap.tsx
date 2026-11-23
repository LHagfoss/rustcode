/**
 * Settings › Machines › Map: the machines and how this computer reaches each one, drawn as a graph
 * (`ui/machinesMapLayout.ts` builds and lays it out). Selecting a node shows that machine's list row
 * under the map, with the same actions the list has.
 */

import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { currentPlatform } from "../lib/platform";
import type { FleetEntry } from "../state/fleet";
import {
  type Machine,
  machineGlyph,
  machineName,
  machineTint,
  platformIcon,
  selfName,
  useLocalIdentity,
} from "../state/machines";
import { useApp } from "../state/store";
import { Maximize2, Minus, Plus, Search } from "./icons";
import { DeviceGlyphIcon } from "./MachineGlyph";
import { machineStatus } from "./machineLabels";
import {
  type Box,
  buildMachinesGraph,
  layoutMachinesGraph,
  mapSummary,
  matchesQuery,
  type WireState,
  wireStateLabel,
} from "./machinesMapLayout";
import { ChromeButton, cx, Tooltip } from "./primitives";

export const SELF_NODE = "self";

const ZOOM = { min: 0.5, max: 2, step: 0.25 };

const WIRE_STYLE: Record<WireState | "link", { className: string; width: number; dash?: string }> = {
  "in-use": { className: "text-success", width: 1.75 },
  standby: { className: "text-fg-faint", width: 1 },
  connecting: { className: "text-warning", width: 1.25, dash: "1 4" },
  offline: { className: "text-fg-faint", width: 1, dash: "4 4" },
  off: { className: "text-fg-faint opacity-50", width: 1, dash: "1 4" },
  link: { className: "text-fg-muted", width: 1.25 },
};

const PILL_STYLE: Record<WireState, string> = {
  "in-use": "border-success/70 font-medium text-fg",
  standby: "border-border text-fg-muted",
  connecting: "border-warning/60 text-fg-muted",
  offline: "border-dashed border-border text-fg-faint",
  off: "border-dotted border-border text-fg-faint",
};

const STATUS_DOT: Record<string, string> = {
  ok: "bg-success",
  warn: "bg-warning",
  bad: "bg-danger",
  idle: "bg-fg-faint",
};

export function MachinesMap({
  machines,
  known,
  selected,
  onSelect,
  footer,
}: {
  machines: readonly Machine[];
  known: readonly FleetEntry[];
  selected: string | null;
  onSelect: (id: string | null) => void;
  footer?: ReactNode;
}) {
  const graph = useMemo(() => buildMachinesGraph(machines, known), [machines, known]);
  const layout = useMemo(() => layoutMachinesGraph(graph), [graph]);
  const [query, setQuery] = useState("");
  const [zoom, setZoom] = useState<number | null>(null);
  const viewport = useRef<HTMLDivElement>(null);
  const width = useViewportWidth(viewport);
  const fit = width ? Math.min(1, width / layout.width) : 1;
  const scale = zoom ?? fit;
  const engine = useApp((s) => s.engine.phase);
  const local = useLocalIdentity();
  const platform = currentPlatform();

  const dim = (name: string) => query.trim() !== "" && !matchesQuery(name, query);
  const zoomBy = (delta: number) =>
    setZoom(Math.min(ZOOM.max, Math.max(ZOOM.min, Math.round((scale + delta) / ZOOM.step) * ZOOM.step)));

  const drawn = new Set(layout.wires.map((wire) => wire.state));
  const searchable = graph.machines.length + graph.known.length + 1 >= SEARCH_FROM;

  return (
    <div className="flex flex-col" data-machines-map>
      {searchable && (
        <div className="rule-b flex items-center gap-2 px-4 py-3">
          <label className="flex h-7 min-w-0 max-w-64 flex-1 items-center gap-1.5 rounded-md bg-fill-subtle px-2 text-fg-muted">
            <Search size={13} />
            <input
              type="search"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Find a machine"
              aria-label="Find a machine"
              className="min-w-0 flex-1 bg-transparent text-callout text-fg outline-none placeholder:text-fg-faint"
            />
          </label>
        </div>
      )}

      <div
        ref={viewport}
        role="group"
        aria-label={`Machines map: ${mapSummary(graph)}`}
        className="relative overflow-auto"
        style={{ maxHeight: 560 }}
      >
        <div style={{ width: layout.width * scale, height: layout.height * scale }}>
          <div
            className="relative origin-top-left"
            style={{ width: layout.width, height: layout.height, transform: `scale(${scale})` }}
          >
            <svg
              className="pointer-events-none absolute inset-0"
              width={layout.width}
              height={layout.height}
              aria-hidden="true"
            >
              <defs>
                <marker
                  id="machines-map-arrow"
                  viewBox="0 0 8 8"
                  refX="7"
                  refY="4"
                  markerWidth="6"
                  markerHeight="6"
                  orient="auto"
                >
                  <path d="M0,0 L8,4 L0,8 z" fill="currentColor" />
                </marker>
              </defs>
              {layout.wires.map((wire) => {
                const style = WIRE_STYLE[wire.state];
                return (
                  <path
                    key={wire.id}
                    d={wire.d}
                    className={style.className}
                    fill="none"
                    stroke="currentColor"
                    strokeWidth={style.width}
                    strokeDasharray={style.dash}
                    markerEnd={wire.state === "link" ? "url(#machines-map-arrow)" : undefined}
                  />
                );
              })}
            </svg>

            {layout.columns.map((column) => (
              <span
                key={column.label}
                className="absolute text-caption text-fg-faint"
                style={{ left: column.x, top: 24 }}
              >
                {column.label}
              </span>
            ))}

            <Node
              box={layout.self}
              selected={selected === SELF_NODE}
              dimmed={dim(selfName())}
              onClick={() => onSelect(selected === SELF_NODE ? null : SELF_NODE)}
              label={`${selfName()}, this computer`}
            >
              <DeviceGlyphIcon
                glyph={local.icon ?? platformIcon(platform === "mac" ? "macos" : platform)}
                tint={local.tint}
                size={16}
              />
              <span className="min-w-0 flex-1">
                <span className="block truncate font-medium text-callout text-fg">{selfName()}</span>
                <span className="block text-caption text-fg-faint">
                  {engine === "ready" ? "This computer" : engine === "connecting" ? "Connecting" : "Offline"}
                </span>
              </span>
            </Node>

            {graph.machines.map((node) => {
              const box = layout.machines.get(node.id);
              if (!box) return null;
              const name = machineName(node.machine);
              const status = machineStatus(node.machine);
              return (
                <Node
                  key={node.id}
                  box={box}
                  selected={selected === node.id}
                  dimmed={dim(name)}
                  onClick={() => onSelect(selected === node.id ? null : node.id)}
                  label={`${name}, ${status.label}`}
                >
                  <span className="relative">
                    <DeviceGlyphIcon
                      glyph={machineGlyph(node.machine)}
                      tint={machineTint(node.machine)}
                      size={16}
                    />
                    <span
                      className={cx(
                        "absolute -right-0.5 -bottom-0.5 size-2 rounded-full ring-2 ring-surface-2",
                        STATUS_DOT[status.tone],
                      )}
                    />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate font-medium text-callout text-fg">{name}</span>
                    <span className="block truncate text-caption text-fg-faint">{status.label}</span>
                  </span>
                </Node>
              );
            })}

            {graph.machines.flatMap((node) => [
              ...node.routes.map((route) => {
                const box = layout.pills.get(route.id);
                if (!box) return null;
                return (
                  <Pill
                    key={route.id}
                    box={box}
                    className={PILL_STYLE[route.state]}
                    tip={route.title}
                    dimmed={dim(machineName(node.machine))}
                  >
                    {route.label}
                  </Pill>
                );
              }),
              node.hidden > 0
                ? (() => {
                    const box = layout.pills.get(`${node.id}:more`);
                    return box ? (
                      <Pill
                        key={`${node.id}:more`}
                        box={box}
                        className={PILL_STYLE.standby}
                        tip={`${node.hidden} more routes`}
                        dimmed={dim(machineName(node.machine))}
                      >
                        +{node.hidden} more
                      </Pill>
                    ) : null;
                  })()
                : null,
            ])}

            {graph.known.map((node) => {
              const box = layout.known.get(node.id);
              if (!box) return null;
              return (
                <Node
                  key={node.id}
                  box={box}
                  selected={selected === node.id}
                  dimmed={dim(node.entry.name)}
                  onClick={() => onSelect(selected === node.id ? null : node.id)}
                  label={`${node.entry.name}, not paired on this computer`}
                  dashed
                >
                  <DeviceGlyphIcon glyph={platformIcon(node.entry.platform)} size={16} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate font-medium text-callout text-fg">{node.entry.name}</span>
                    <span className="block truncate text-caption text-fg-faint">Not paired here</span>
                  </span>
                </Node>
              );
            })}
          </div>
        </div>
      </div>

      <div className="rule-t flex flex-wrap items-center gap-x-4 gap-y-1 py-2 pr-2 pl-4 text-caption text-fg-muted">
        {LEGEND_ORDER.filter((state) => drawn.has(state)).map((state) => (
          <Legend key={state} state={state} />
        ))}
        <span className="ml-auto flex items-center">
          <ChromeButton label="Zoom out" onClick={() => zoomBy(-ZOOM.step)} disabled={scale <= ZOOM.min}>
            <Minus size={13} />
          </ChromeButton>
          <ChromeButton label="Zoom in" onClick={() => zoomBy(ZOOM.step)} disabled={scale >= ZOOM.max}>
            <Plus size={13} />
          </ChromeButton>
          <ChromeButton label="Fit to view" onClick={() => setZoom(null)} disabled={zoom === null}>
            <Maximize2 size={13} />
          </ChromeButton>
        </span>
      </div>
      {footer && <div className="rule-t">{footer}</div>}
    </div>
  );
}

function Node({
  box,
  selected,
  dimmed,
  onClick,
  label,
  dashed,
  children,
}: {
  box: Box;
  selected: boolean;
  dimmed: boolean;
  onClick: () => void;
  label: string;
  dashed?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      aria-pressed={selected}
      onClick={onClick}
      className={cx(
        "absolute flex items-center gap-2 rounded-lg border bg-surface-2 px-3 text-left transition-opacity hover:bg-fill",
        dashed ? "border-dashed border-border" : "border-border",
        selected && "border-accent ring-2 ring-accent/20",
        dimmed && "opacity-30",
      )}
      style={{ left: box.x, top: box.y, width: box.w, height: box.h }}
    >
      {children}
    </button>
  );
}

function Pill({
  box,
  className,
  tip,
  dimmed,
  children,
}: {
  box: Box;
  className: string;
  tip: string;
  dimmed: boolean;
  children: ReactNode;
}) {
  return (
    <Tooltip label={tip}>
      <span
        className={cx(
          "absolute flex items-center justify-center truncate rounded-full border bg-surface-2 px-2 text-caption transition-opacity",
          className,
          dimmed && "opacity-30",
        )}
        style={{ left: box.x, top: box.y, width: box.w, height: box.h }}
      >
        {children}
      </span>
    </Tooltip>
  );
}

/** A map with at least this many machines (this one included) gets a search box. */
const SEARCH_FROM = 6;

const LEGEND_ORDER = ["in-use", "standby", "connecting", "offline", "link"] as const;

function Legend({ state }: { state: WireState | "link" }) {
  const style = WIRE_STYLE[state];
  return (
    <span className="flex items-center gap-1.5">
      <svg width="20" height="6" aria-hidden="true" className={style.className}>
        <line
          x1="0"
          y1="3"
          x2={state === "link" ? 15 : 20}
          y2="3"
          stroke="currentColor"
          strokeWidth={style.width}
          strokeDasharray={style.dash}
        />
        {state === "link" && <path d="M14,0 L20,3 L14,6 z" fill="currentColor" />}
      </svg>
      {state === "link" ? "Seen by another" : wireStateLabel(state)}
    </span>
  );
}

function useViewportWidth(ref: React.RefObject<HTMLDivElement | null>): number | null {
  const [width, setWidth] = useState<number | null>(null);
  useEffect(() => {
    const element = ref.current;
    if (!element || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(([entry]) => setWidth(entry?.contentRect.width ?? null));
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref]);
  return width;
}
