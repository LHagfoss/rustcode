// Naming a computer here and choosing its glyph (docs/desktop.md › Several
// machines). A name is shared: every paired computer shows the newest rename
// (state/fleet.ts), and Reset returns to the name the machine reports. The glyph is one of a small fixed set plus a tint from the
// app's chart palette, so a list of machines reads at a glance and no value is
// ever a colour literal.

import { RotateCcw } from "lucide-react";
import { Popover } from "radix-ui";
import { type ReactElement, useState } from "react";
import {
  cleanMachineName,
  MACHINE_ICONS,
  MACHINE_TINTS,
  type MachineIcon,
  type MachineIdentity,
  type MachineTint,
  platformIcon,
} from "../state/machines";
import { DeviceGlyphIcon } from "./MachineGlyph";
import { noTextHelpers } from "./primitives";
import { Action, fieldClass } from "./settingsKit";

/**
 * The name and glyph editor for one computer. `identity` is what is shown now;
 * `reportedName` is the machine's own name, which Reset returns to.
 */
export function MachineIdentityEditor({
  label,
  reportedName,
  platform,
  identity,
  onName,
  onIcon,
  trigger,
}: {
  /** What the control is for, in the accessible name and the panel's lead. */
  label: string;
  /** The name the machine reports, shown when no local name is set. */
  reportedName: string;
  platform: string;
  identity: MachineIdentity;
  onName: (name: string | null) => void;
  onIcon: (icon: MachineIcon | null, tint: MachineTint | null) => void;
  trigger: ReactElement;
}) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState(identity.displayName ?? "");

  const commitName = () => {
    const next = cleanMachineName(draft);
    if (next !== identity.displayName) onName(next);
  };

  return (
    <Popover.Root
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (next) setDraft(identity.displayName ?? "");
        else commitName();
      }}
    >
      <Popover.Trigger asChild>{trigger}</Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="end"
          sideOffset={6}
          aria-label={label}
          className="elevated z-[60] flex w-[300px] flex-col gap-3 p-3 rc-in"
        >
          <div className="flex flex-col gap-1">
            <label className="font-medium text-callout text-fg" htmlFor={`machine-name-${label}`}>
              Name
            </label>
            <input
              id={`machine-name-${label}`}
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              onBlur={commitName}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  commitName();
                  setOpen(false);
                }
              }}
              {...noTextHelpers}
              placeholder={reportedName}
              className={fieldClass}
            />
            <div className="flex items-center justify-between gap-2">
              <span className="min-w-0 truncate text-caption text-fg-muted">
                Blank shows {reportedName}. Every paired computer shows this name.
              </span>
              {identity.displayName && (
                <button
                  type="button"
                  className="flex shrink-0 items-center gap-1 rounded-sm px-1 text-caption text-fg-muted hover:text-fg"
                  onClick={() => {
                    setDraft("");
                    onName(null);
                  }}
                >
                  <RotateCcw size={11} />
                  Reset
                </button>
              )}
            </div>
          </div>

          <div className="flex flex-col gap-1.5">
            <span className="font-medium text-callout text-fg">Icon</span>
            <div className="grid grid-cols-9 gap-1">
              {MACHINE_ICONS.map((icon) => {
                const chosen = identity.icon === icon;
                return (
                  <button
                    key={icon}
                    type="button"
                    aria-label={icon}
                    aria-pressed={chosen}
                    onClick={() => onIcon(chosen ? null : icon, chosen ? null : identity.tint)}
                    className={
                      chosen
                        ? "flex size-7 items-center justify-center rounded-md bg-fill-strong text-fg"
                        : "flex size-7 items-center justify-center rounded-md text-fg-muted hover:bg-fill"
                    }
                  >
                    <DeviceGlyphIcon glyph={icon} tint={chosen ? identity.tint : null} size={15} />
                  </button>
                );
              })}
            </div>
            <div className="flex items-center gap-1.5">
              <span className="text-caption text-fg-muted">Tint</span>
              {MACHINE_TINTS.map((tint) => (
                <button
                  key={tint}
                  type="button"
                  aria-label={`Tint ${tint}`}
                  aria-pressed={identity.tint === tint}
                  onClick={() =>
                    onIcon(identity.icon ?? platformIcon(platform), identity.tint === tint ? null : tint)
                  }
                  className={
                    identity.tint === tint
                      ? "size-4 rounded-full ring-2 ring-border-strong"
                      : "size-4 rounded-full"
                  }
                  style={{ background: `var(--rc-series-${tint})` }}
                />
              ))}
            </div>
          </div>

          <div className="flex items-center justify-between gap-2 border-border border-t pt-2">
            <span className="min-w-0 truncate text-caption text-fg-muted">{label}</span>
            <Action quiet size="sm" onClick={() => setOpen(false)}>
              Done
            </Action>
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
