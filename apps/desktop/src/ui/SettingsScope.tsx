// **Applying to** at the top of a Settings page with server settings, and
// **Showing** on a machine-only page (docs/desktop.md › Settings on one
// machine or all). The choice is one for the whole panel; All machines offers
// every connected machine and leaves the offline ones alone.
//
// A row whose connected machines disagree draws `MixedChip` instead of one
// value, with each machine's value in its hover card.

import { Layers } from "lucide-react";
import type { ReactNode } from "react";
import type { ScopeBreakdownEntry } from "../state/settingsRegistry";
import { scopeLabel, useScope } from "../state/settingsScope";
import { Tooltip } from "./primitives";
import { Select } from "./settingsKit";

const THIS = "this";
const ALL = "all";

/** The scope picker; `machineOnly` drops All machines and says Showing. */
export function SettingsScopeBar({ machineOnly = false }: { machineOnly?: boolean }) {
  const { scope, setScope, machines } = useScope();
  const value = machineOnly
    ? scope.mode === "machine"
      ? `machine:${scope.host}`
      : THIS
    : scope.mode === "all"
      ? ALL
      : scope.mode === "machine"
        ? `machine:${scope.host}`
        : THIS;
  const offline = scope.mode === "all" ? machines.filter((machine) => !machine.online) : [];
  const shown =
    scope.mode === "machine" ? machines.find((machine) => machine.host === scope.host) : undefined;
  const options = [
    { value: THIS, label: "This machine" },
    ...machines
      .filter((machine) => !machine.local)
      .map((machine) => ({
        value: `machine:${machine.host}`,
        label: machine.online ? machine.name : `${machine.name} · offline`,
      })),
    ...(machineOnly ? [] : [{ value: ALL, label: "All machines" }]),
  ];
  return (
    <div className="mb-6 flex flex-col gap-2 rounded-xl bg-surface px-4 py-3">
      <div className="flex flex-wrap items-center gap-3">
        <div className="min-w-0 flex-1">
          <div className="font-medium text-body text-fg">{machineOnly ? "Showing" : "Applying to"}</div>
          <div className="max-w-[70ch] text-callout text-fg-muted">
            {machineOnly
              ? "These settings belong to the machine you pick. Credentials, accounts and paths stay on it."
              : "Server settings on this page change on the machine you pick. App preferences stay on this computer."}
          </div>
        </div>
        <Select
          label={machineOnly ? "Showing machine" : "Applying to"}
          value={value}
          options={options}
          onChange={(next) => {
            if (next === ALL) setScope({ mode: "all" });
            else if (next === THIS) setScope({ mode: "this" });
            else setScope({ mode: "machine", host: next.slice("machine:".length) });
          }}
        />
      </div>
      {offline.length > 0 && (
        <p className="m-0 text-caption text-fg-muted">
          {offline.map((machine) => `${machine.name} is offline`).join(", ")} —{" "}
          {offline.length === 1 ? "it keeps its value." : "they keep their values."}
        </p>
      )}
      {shown && !shown.online && (
        <p className="m-0 text-caption text-warning">
          {shown.name} is offline. Its last values are shown; it keeps them until it connects, and a change
          here is not sent.
        </p>
      )}
    </div>
  );
}

/** `Mixed` with the layers mark: the connected machines disagree, and the hover card says how. */
export function MixedChip({ breakdown }: { breakdown: ScopeBreakdownEntry[] }) {
  return (
    <Tooltip
      side="top"
      label={
        <span className="flex flex-col gap-0.5">
          {breakdown.map((entry) => (
            <span key={entry.host || "local"} className="block">
              <span className="font-medium">{entry.name}:</span> {entry.label}
            </span>
          ))}
        </span>
      }
    >
      <span className="inline-flex h-6 shrink-0 items-center gap-1 rounded-full bg-warning/12 px-2 font-medium text-caption text-warning">
        <Layers size={11} />
        Mixed
      </span>
    </Tooltip>
  );
}

/** The value a row shows: the control, or the Mixed chip above it. */
export function ScopedValue({
  mixed,
  breakdown,
  children,
}: {
  mixed: boolean;
  breakdown: ScopeBreakdownEntry[];
  children: ReactNode;
}) {
  return (
    <div className="flex shrink-0 items-center gap-2">
      {mixed && <MixedChip breakdown={breakdown} />}
      {children}
    </div>
  );
}

/** Where a page's rows are shown, for a title ("Lars's MacBook Pro"). */
export function scopeName(value: string | undefined, machines: { host: string; name: string }[]): string {
  return machines.find((machine) => machine.host === value)?.name ?? "this machine";
}

export { scopeLabel };
