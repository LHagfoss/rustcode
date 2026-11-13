// The environment selector: where a new thread runs (docs/desktop.md › Several machines), after
// T3 Code's "Run on" selector. This computer first, then each paired environment with its live
// status and the address it is reached on; one that is away is listed, so the user sees why it
// cannot be picked, but is not selectable. Its last rows leave the list: "Add environment…" opens
// the Add environment dialog on Settings › Environments, and "Manage environments" opens the page.
//
// Hidden until a second computer is paired: a selector with one choice is noise. A thread that
// exists already has its machine, so this is the new-thread strip's own control. The composer
// mounts it and nothing else (`ui/Composer.tsx`).

import { Check, ChevronDown, Plus, Settings2 } from "lucide-react";
import { appPlatform } from "../lib/platform";
import {
  type Machine,
  type MachineIdentity,
  machineGlyph,
  machineName,
  machineTint,
  platformIcon,
  selfName,
  setAddEnvironmentOpen,
  useLocalIdentity,
  useMachines,
} from "../state/machines";
import { openSettings, setNewThreadHost, useApp } from "../state/store";
import { CardMenuItem, CardPopover, menuKeys } from "./cardPopover";
import { type EnvironmentChoice, environmentChoices } from "./environments";
import { DeviceGlyphIcon } from "./MachineGlyph";
import { cx } from "./primitives";

// The composer footer's button (`footerButton` in ui/Composer.tsx): quiet until hovered, and its
// fill is its open state.
const footerButton =
  "flex h-7 items-center gap-1.5 whitespace-nowrap rounded-full px-2 text-callout text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong";

const DOT: Record<EnvironmentChoice["status"]["kind"], string> = {
  online: "bg-success",
  connecting: "animate-pulse bg-fg-muted",
  offline: "bg-fg-faint",
  attention: "bg-warning",
};

export function EnvironmentSelector() {
  const machines = useMachines((s) => s.machines);
  const local = useLocalIdentity();
  const host = useApp((s) => s.newThreadHost);
  return <EnvironmentSelectorView machines={machines} local={local} host={host} />;
}

/** The selector over what it is given: the store's machines and this computer's own identity. */
export function EnvironmentSelectorView({
  machines,
  local,
  host,
}: {
  machines: Machine[];
  local: MachineIdentity;
  /** `newThreadHost`: `""` for this computer, else a machine's id. */
  host: string;
}) {
  const platform = appPlatform();
  if (!machines.length) return null;
  const here = platformIcon(platform === "mac" ? "macos" : platform);
  const current = machines.find((machine) => machine.id === host);
  const choices = environmentChoices(machines, { name: local.displayName ?? selfName() }, machineName);
  const label = current ? machineName(current) : choices[0]!.name;
  const glyphOf = (id: string) => {
    const machine = machines.find((entry) => entry.id === id);
    return machine ? machineGlyph(machine) : (local.icon ?? here);
  };
  const tintOf = (id: string) => {
    const machine = machines.find((entry) => entry.id === id);
    return machine ? machineTint(machine) : local.tint;
  };
  return (
    <CardPopover
      role="menu"
      label="Run on"
      tooltip={`Run on ${label}`}
      onKeyDown={menuKeys}
      side="top"
      className="min-w-[260px]"
      trigger={
        <button
          type="button"
          aria-label={`Run on: ${label}`}
          className={cx(footerButton, "min-w-0 shrink data-[state=open]:bg-fill")}
        >
          <DeviceGlyphIcon glyph={glyphOf(current?.id ?? "")} tint={tintOf(current?.id ?? "")} size={14} />
          <span className="min-w-0 max-w-[200px] truncate @max-lg/composer:max-w-[96px] @max-sm/composer:hidden">
            {label}
          </span>
          <ChevronDown size={12} className="shrink-0 text-fg-faint" />
        </button>
      }
    >
      <div role="menu" aria-label="Run on">
        <div className="px-2 pt-1.5 pb-1 text-caption text-fg-faint">Run on</div>
        {choices.map((choice) => {
          const selected = choice.id === host;
          return (
            <CardMenuItem
              key={choice.id || "local"}
              role="menuitemradio"
              aria-checked={selected}
              disabled={!choice.available}
              onSelect={() => void setNewThreadHost(choice.id)}
              className={cx("gap-2", selected && "bg-fill-subtle", !choice.available && "opacity-60")}
            >
              <span
                aria-hidden
                data-status={choice.status.kind}
                className={cx("size-1.5 shrink-0 rounded-full", DOT[choice.status.kind])}
              />
              <DeviceGlyphIcon glyph={glyphOf(choice.id)} tint={tintOf(choice.id)} size={14} />
              <span className="flex min-w-0 flex-1 flex-col">
                <span className="truncate text-body text-fg">{choice.name}</span>
                <span className="truncate text-caption text-fg-faint">{choice.detail}</span>
              </span>
              {!choice.available && choice.id && (
                <span className="shrink-0 text-caption text-fg-faint">{choice.status.label}</span>
              )}
              {selected && <Check size={13} className="shrink-0 text-fg-muted" />}
            </CardMenuItem>
          );
        })}
        <div role="separator" className="mx-2 my-1 h-px shrink-0 bg-border" />
        <CardMenuItem
          className="gap-2"
          onSelect={() => {
            openSettings("machines");
            setAddEnvironmentOpen(true);
          }}
        >
          <Plus size={14} className="shrink-0 text-fg-muted" />
          <span className="text-body text-fg">Add environment…</span>
        </CardMenuItem>
        <CardMenuItem className="gap-2" onSelect={() => openSettings("machines")}>
          <Settings2 size={14} className="shrink-0 text-fg-muted" />
          <span className="text-body text-fg">Manage environments</span>
        </CardMenuItem>
      </div>
    </CardPopover>
  );
}
