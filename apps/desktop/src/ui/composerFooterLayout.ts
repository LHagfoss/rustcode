// What the composer's strip says about where a thread runs (docs/design.md › Composer strip): the
// machine of a thread that is not on this computer, as one quiet chip beside the project's folder.
// A thread on this computer carries none, as in the sidebar (docs/desktop.md › Several machines).

import {
  type Machine,
  type MachineIcon,
  type MachineTint,
  machineGlyph,
  machineName,
  machineTint,
  platformIcon,
} from "../state/machines";
import { LOCAL_HOST } from "../state/threadKeys";

/** The chip the strip draws for a thread's machine. */
export interface ThreadMachine {
  name: string;
  glyph: MachineIcon;
  tint: MachineTint | null;
  /** The machine is not reachable now: the chip says so, the thread cannot take a turn there. */
  away: boolean;
  /** The line a hover says: the machine and, when it is away, why the thread waits. */
  title: string;
}

/**
 * The machine a thread runs on, or null for a thread on this computer. A host this app no longer
 * has paired (its machine was removed) still reads as another machine, by a generic name, so a
 * thread is never mistaken for one that runs here.
 */
export function threadMachine(host: string, machines: Machine[]): ThreadMachine | null {
  if (host === LOCAL_HOST) return null;
  const machine = machines.find((m) => m.id === host);
  if (!machine) {
    return {
      name: "Another machine",
      glyph: platformIcon(null),
      tint: null,
      away: true,
      title: "Runs on a machine that is no longer paired",
    };
  }
  const name = machineName(machine);
  const away = machine.state !== "connected";
  return {
    name,
    glyph: machineGlyph(machine),
    tint: machineTint(machine),
    away,
    title: away
      ? `Runs on ${name} · ${machine.state === "connecting" ? "connecting" : "offline"}`
      : `Runs on ${name}`,
  };
}
