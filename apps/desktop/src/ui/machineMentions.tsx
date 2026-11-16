// The `@` drawer's machines (state/mentionDrawer.ts): where the agent should run the work, so it
// need not look for one. Listed once another computer is paired, as the Run on selector is.

import { useMemo } from "react";
import { appPlatform } from "../lib/platform";
import {
  machineGlyph,
  machineName,
  machineTint,
  platformIcon,
  selfName,
  useLocalIdentity,
  useMachines,
} from "../state/machines";
import type { MachineMentionChoice } from "../state/mentionDrawer";
import { DRAWER_NAME, Marked } from "./drawerMarks";
import { DeviceGlyphIcon } from "./MachineGlyph";
import { machineChoices } from "./machineLabels";
import { cx } from "./primitives";

/**
 * The machines the drawer offers, this computer first. Each says what the thread's own engine
 * calls it: `local` is the computer the thread runs on (`host`, `""` for this one), and a remote
 * thread's engine keeps its own list of machines, so there the others go by name.
 */
export function useMachineMentions(host: string): MachineMentionChoice[] {
  const machines = useMachines((s) => s.machines);
  const local = useLocalIdentity();
  return useMemo(() => {
    if (!machines.length) return [];
    const platform = appPlatform();
    const here = local.icon ?? platformIcon(platform === "mac" ? "macos" : platform);
    return machineChoices(machines, { name: local.displayName ?? selfName() }, machineName).map(
      (choice): MachineMentionChoice => {
        const machine = machines.find((entry) => entry.id === choice.id);
        return {
          id: choice.id === host ? "local" : host ? choice.name : choice.id,
          name: choice.name,
          detail: choice.detail,
          available: choice.available,
          glyph: machine ? machineGlyph(machine) : here,
          tint: machine ? machineTint(machine) : local.tint,
        };
      },
    );
  }, [machines, local, host]);
}

/** A machine's row: its glyph, its name (the letters `query` matched marked) and its address. */
export function MachineMentionRow({
  machine,
  query,
  row,
  onPick,
}: {
  machine: MachineMentionChoice;
  query: string;
  row: React.ButtonHTMLAttributes<HTMLButtonElement> & {
    className: string;
    ref?: React.Ref<HTMLButtonElement>;
  };
  onPick: () => void;
}) {
  return (
    <button {...row} onClick={onPick} className={cx(row.className, !machine.available && "opacity-60")}>
      <DeviceGlyphIcon glyph={machine.glyph} tint={machine.tint} size={16} />
      <span className={DRAWER_NAME}>
        <Marked text={machine.name} query={query} />
      </span>
      <span className="min-w-0 flex-1 truncate text-fg-muted text-sub">{machine.detail}</span>
    </button>
  );
}
