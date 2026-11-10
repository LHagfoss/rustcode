// The Harnesses group when the page edits another machine, or all of them
// (docs/desktop.md › Settings on one machine or all). One compact row per
// harness, with the switch on each connected machine; where they disagree the
// row says Mixed and the switch, like every other server setting, writes the
// chosen value to all of them. Sign-in, accounts, Check and Remove stay on the
// machine itself, so they are absent here.

import { useMemo, useState } from "react";
import type { Harness } from "../acp/types";
import { type ScopeMachine, useMachineHarnessList, writeHarnessEnabled } from "../state/settingsScope";
import { errorText } from "../state/store";
import { HarnessIcon } from "./HarnessIcon";
import { cx, Spinner, Switch, Tooltip } from "./primitives";
import { MixedChip } from "./SettingsScope";
import { Row } from "./settingsKit";

export function ScopedHarnessList({ targets, writes }: { targets: ScopeMachine[]; writes: ScopeMachine[] }) {
  const snapshots = useMachineHarnessList(targets.map((machine) => machine.host));
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const rows = useMemo(() => {
    const byId = new Map<string, { harness: Harness; byHost: Map<string, Harness> }>();
    for (const entry of snapshots) {
      for (const harness of entry.snapshot?.harnesses ?? []) {
        const row = byId.get(harness.id) ?? { harness, byHost: new Map<string, Harness>() };
        row.byHost.set(entry.host, harness);
        byId.set(harness.id, row);
      }
    }
    return [...byId.values()];
  }, [snapshots]);
  if (snapshots.every((entry) => !entry.snapshot && !entry.loading && !entry.error)) {
    return <Row title="No harnesses read" subtitle="The machine did not answer." />;
  }
  const reading = snapshots.some((entry) => entry.loading);
  return (
    <div className="divide-y divide-border">
      {rows.map(({ harness, byHost }) => {
        const online = targets.filter((machine) => machine.online);
        const values = online.map((machine) => byHost.get(machine.host)?.enabled);
        const known = values.filter((value) => value !== undefined);
        const mixed = known.length > 1 && known.some((value) => value !== known[0]);
        const checked = mixed ? values.some(Boolean) : (known[0] ?? false);
        const offline = targets.some((machine) => !machine.online && !byHost.has(machine.host));
        const toggle = async (enabled: boolean) => {
          setBusy(harness.id);
          setError(null);
          try {
            for (const machine of writes) await writeHarnessEnabled(machine.host, harness.id, enabled);
          } catch (failure) {
            setError(errorText(failure));
          } finally {
            setBusy(null);
          }
        };
        return (
          <div key={harness.id} className="flex min-h-12 items-center gap-3 px-4 py-2">
            <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-fill">
              <HarnessIcon id={harness.id} name={harness.name} size={17} />
            </span>
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="truncate font-medium text-body">{harness.name}</span>
                {mixed && <MixedChip breakdown={breakdown(targets, byHost)} />}
              </div>
              <div className="truncate text-caption text-fg-muted">
                {reading
                  ? "Reading…"
                  : offline
                    ? "An offline machine keeps its value"
                    : mixed
                      ? "The connected machines disagree"
                      : checked
                        ? "On"
                        : "Off"}
              </div>
            </div>
            {busy === harness.id ? (
              <Spinner size={12} />
            ) : (
              <Tooltip label={writes.length === 0 ? "No connected machine to change" : undefined}>
                <span>
                  <Switch
                    label={`Use ${harness.name}`}
                    checked={checked}
                    disabled={writes.length === 0}
                    onChange={(value) => void toggle(value)}
                  />
                </span>
              </Tooltip>
            )}
          </div>
        );
      })}
      {error && <p className={cx("px-4 py-2 text-callout text-warning")}>{error}</p>}
    </div>
  );
}

function breakdown(targets: ScopeMachine[], byHost: Map<string, Harness>) {
  return targets.map((machine) => ({
    host: machine.host,
    name: machine.name,
    online: machine.online,
    value: byHost.get(machine.host)?.enabled,
    label: machine.online ? (byHost.get(machine.host)?.enabled ? "On" : "Off") : "offline, keeps its value",
  }));
}
