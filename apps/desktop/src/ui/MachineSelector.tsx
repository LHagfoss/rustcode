// The machine selector: picks where a new thread runs ("Run on"), or moves an existing thread to
// another machine ("Move this thread to") with smart project & harness matching and one-click
// return (docs/desktop.md › Several machines).
//
// Hidden until a second computer is paired: a selector with one choice is noise. For an open thread
// on an unpaired host (machine removed), `ThreadMachineChip` in ComposerFooter still labels it.

import { type ReactElement, useMemo } from "react";
import { appPlatform } from "../lib/platform";
import {
  type Machine,
  type MachineIdentity,
  machineGlyph,
  machineName,
  machineTint,
  platformIcon,
  selfName,
  setAddMachineOpen,
  useLocalIdentity,
  useMachines,
} from "../state/machines";
import { registeredProjects } from "../state/projectBook";
import {
  moveMachineBlockedReason,
  moveThreadToMachine,
  openSettings,
  planMachineMoveDestination,
  setNewThreadHost,
  useApp,
  useMovingMachine,
} from "../state/store";
import { isRunning, type ThreadState } from "../state/thread";
import { splitThreadKey } from "../state/threadKeys";
import { CardMenuItem, CardPopover, type CardTriggerProps, menuKeys } from "./cardPopover";
import { Check, ChevronDown, Plus, Settings2 } from "./icons";
import { DeviceGlyphIcon } from "./MachineGlyph";
import { type MachineChoice, machineChoices } from "./machineLabels";
import { cx, Spinner } from "./primitives";

// The composer footer's button (`footerButton` in ui/ComposerFooter.tsx): quiet until hovered,
// and its fill is its open state.
const footerButton =
  "flex h-7 items-center gap-1.5 whitespace-nowrap rounded-full px-2 text-callout text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong";

const DOT: Record<MachineChoice["status"]["kind"], string> = {
  online: "bg-success",
  connecting: "animate-pulse bg-fg-muted",
  offline: "bg-fg-faint",
  attention: "bg-warning",
};

export function MachineSelector({
  thread,
  trigger,
  side = "top",
  align,
}: {
  thread?: ThreadState;
  /** Optional custom trigger (used by the chat header's machine pill). */
  trigger?: ReactElement<CardTriggerProps & Record<string, unknown>>;
  side?: "top" | "bottom";
  align?: "start" | "center" | "end";
}) {
  const machines = useMachines((s) => s.machines);
  const local = useLocalIdentity();
  const newThreadHost = useApp((s) => s.newThreadHost);
  const host = thread ? splitThreadKey(thread.id).host : newThreadHost;
  return (
    <MachineSelectorView
      machines={machines}
      local={local}
      host={host}
      thread={thread}
      trigger={trigger}
      side={side}
      align={align}
    />
  );
}

/** The selector over what it is given: the store's machines, this computer's identity, and an optional open thread. */
export function MachineSelectorView({
  machines,
  local,
  host,
  thread,
  trigger,
  side = "top",
  align,
}: {
  machines: Machine[];
  local: MachineIdentity;
  /** `""` for this computer, else a machine's id. */
  host: string;
  /** When set, moves this open thread between machines instead of choosing where a new thread starts. */
  thread?: ThreadState;
  trigger?: ReactElement<CardTriggerProps & Record<string, unknown>>;
  side?: "top" | "bottom";
  align?: "start" | "center" | "end";
}) {
  const platform = appPlatform();
  const sessions = useApp((s) => s.sessions);
  const movingTo = useMovingMachine(thread?.id);
  const localProjects = useMemo(() => (thread ? registeredProjects() : []), [thread]);

  if (!machines.length) return null;

  const here = platformIcon(platform === "mac" ? "macos" : platform);
  const current = machines.find((machine) => machine.id === host);
  const choices = machineChoices(machines, { name: local.displayName ?? selfName() }, machineName);
  const label = current ? machineName(current) : host ? "Another machine" : choices[0]!.name;
  const away = !!host && current?.state !== "connected";
  const glyphOf = (id: string) => {
    const machine = machines.find((entry) => entry.id === id);
    return machine ? machineGlyph(machine) : id ? platformIcon(null) : (local.icon ?? here);
  };
  const tintOf = (id: string) => {
    const machine = machines.find((entry) => entry.id === id);
    return machine ? machineTint(machine) : id ? null : local.tint;
  };

  const blocked = thread
    ? moveMachineBlockedReason({
        running: isRunning(thread),
        sourceHost: host,
        sourceConnected: !away,
        sourceMachineName: label,
        moving: !!movingTo,
      })
    : undefined;

  const sessionId = thread ? splitThreadKey(thread.id).id : "";
  const menuHeading = thread ? "Move this thread to" : "Run on";
  const triggerTooltip = thread
    ? blocked
      ? `Runs on ${label} · ${blocked}`
      : `Runs on ${label} · click to move to another machine`
    : `Run on ${label}`;

  const defaultTrigger = (
    <button
      type="button"
      aria-label={thread ? `Machine: ${label}` : `Run on: ${label}`}
      aria-busy={!!movingTo || undefined}
      className={cx(footerButton, "min-w-0 shrink data-[state=open]:bg-fill")}
    >
      <span className="relative flex shrink-0">
        {movingTo ? (
          <Spinner size={12} />
        ) : (
          <DeviceGlyphIcon glyph={glyphOf(host)} tint={tintOf(host)} size={14} />
        )}
        {away && !movingTo && (
          <span className="-right-0.5 -top-0.5 absolute size-1.5 rounded-full bg-warning ring-2 ring-[var(--rc-composer-footer-bg)]" />
        )}
      </span>
      <span className="min-w-0 max-w-[200px] truncate @max-lg/composer:max-w-[96px] @max-sm/composer:hidden">
        {movingTo ? "Moving…" : label}
      </span>
      <ChevronDown size={12} className="shrink-0 text-fg-faint" />
    </button>
  );

  return (
    <CardPopover
      role="menu"
      label={menuHeading}
      tooltip={triggerTooltip}
      onKeyDown={menuKeys}
      side={side}
      align={align}
      className="min-w-[270px]"
      trigger={trigger ?? defaultTrigger}
    >
      <div role="menu" aria-label={menuHeading}>
        <div className="px-2 pt-1.5 pb-1 text-caption text-fg-faint">{menuHeading}</div>
        {blocked && (
          <p role="note" className="px-2 pb-1.5 text-caption text-fg-muted">
            {blocked}
          </p>
        )}
        <div className="flex flex-col gap-1">
          {choices.map((choice) => {
            const selected = choice.id === host;
            // For moving an existing thread, any connected machine is a valid target (even if paused
            // for *new* threads), unless the thread itself is blocked or already on that machine.
            const machine = choice.id ? machines.find((entry) => entry.id === choice.id) : undefined;
            const reachable = !choice.id || machine?.state === "connected";
            const canSelect = thread ? !blocked && !selected && reachable : choice.available;

            let detail = choice.detail;
            if (thread) {
              if (selected) {
                detail = "Current machine";
              } else if (!reachable) {
                detail = choice.status.label;
              } else {
                const dest = planMachineMoveDestination({
                  sessionId,
                  fromHost: host,
                  fromCwd: thread.cwd,
                  toHost: choice.id,
                  sessions,
                  localProjects,
                });
                detail = dest.returning ? `Returns to ${dest.displayFolder}` : dest.displayFolder;
              }
            }

            return (
              <CardMenuItem
                key={choice.id || "local"}
                role="menuitemradio"
                aria-checked={selected}
                disabled={!canSelect}
                onSelect={() => {
                  if (thread) void moveThreadToMachine(thread.id, choice.id);
                  else void setNewThreadHost(choice.id);
                }}
                className={cx(
                  "gap-2 py-1.5",
                  selected && "bg-fill-subtle",
                  !canSelect && !selected && "opacity-60",
                )}
              >
                <DeviceGlyphIcon glyph={glyphOf(choice.id)} tint={tintOf(choice.id)} size={14} />
                <span className="flex min-w-0 flex-1 flex-col">
                  <span className="flex min-w-0 items-center gap-1.5">
                    <span className="truncate text-body text-fg">{choice.name}</span>
                    <span
                      aria-hidden
                      data-status={choice.status.kind}
                      className={cx("size-1.5 shrink-0 rounded-full", DOT[choice.status.kind])}
                    />
                  </span>
                  <span className="truncate text-caption text-fg-faint">{detail}</span>
                </span>
                {!reachable && choice.id && (
                  <span className="shrink-0 text-caption text-fg-faint">{choice.status.label}</span>
                )}
                {selected && <Check size={13} className="shrink-0 text-fg-muted" />}
              </CardMenuItem>
            );
          })}
        </div>
        <div role="separator" className="mx-2 my-1 h-px shrink-0 bg-border" />
        <CardMenuItem
          className="gap-2"
          onSelect={() => {
            openSettings("machines");
            setAddMachineOpen(true);
          }}
        >
          <Plus size={14} className="shrink-0 text-fg-muted" />
          <span className="text-body text-fg">Add machine…</span>
        </CardMenuItem>
        <CardMenuItem className="gap-2" onSelect={() => openSettings("machines")}>
          <Settings2 size={14} className="shrink-0 text-fg-muted" />
          <span className="text-body text-fg">Manage machines</span>
        </CardMenuItem>
      </div>
    </CardPopover>
  );
}
