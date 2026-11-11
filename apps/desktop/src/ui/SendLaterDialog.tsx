// Send later: the one-tap times, and a date and time of the user's own, for the message in the
// composer. A preset sends at the press; the picker's own time waits for Schedule, and never takes
// one that has already passed. The card that counts down appears over the composer once it is
// scheduled (ui/ScheduledMessages.tsx).

import { Dialog } from "radix-ui";
import { useEffect, useState } from "react";
import { countdown, presetTime, SEND_LATER_PRESETS, whenText } from "../state/schedule";
import { DateTimePicker } from "./DateTimePicker";
import { Pill } from "./primitives";

export function SendLaterDialog({
  open,
  onOpenChange,
  preview,
  onSchedule,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The message being scheduled, as its card will read it. */
  preview: string;
  onSchedule: (due: number) => void;
}) {
  const [value, setValue] = useState<Date | null>(() => new Date(Date.now() + 3_600_000));
  // Every opening starts an hour out: a window left open all day would otherwise offer a time that
  // has already passed.
  useEffect(() => {
    if (open) setValue(new Date(Date.now() + 3_600_000));
  }, [open]);
  const valid = value !== null && value.getTime() > Date.now() + 1000;
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <div className="fixed inset-0 z-50 flex items-center justify-center pointer-events-none">
          <Dialog.Content
            aria-describedby={undefined}
            className="elevated pointer-events-auto flex w-[min(380px,calc(100vw-48px))] flex-col gap-3 p-5"
          >
            <Dialog.Title className="font-semibold text-body">Send later</Dialog.Title>
            <p className="truncate text-fg-muted" title={preview}>
              {preview || "A message"}
            </p>
            <div className="flex flex-col gap-1">
              {SEND_LATER_PRESETS.map((preset) => {
                const due = presetTime(preset.id);
                return (
                  <Pill
                    key={preset.id}
                    quiet
                    size="lg"
                    className="w-full justify-between"
                    onClick={() => onSchedule(presetTime(preset.id))}
                  >
                    <span>{preset.label}</span>
                    <span className="font-normal text-caption text-fg-faint">{whenText(due)}</span>
                  </Pill>
                );
              })}
            </div>
            <div className="flex items-center gap-2 text-caption text-fg-faint">
              <span className="h-px flex-1 bg-border" />
              or a time
              <span className="h-px flex-1 bg-border" />
            </div>
            <DateTimePicker value={value ?? new Date()} onChange={setValue} />
            <p aria-live="polite" className="text-fg-muted">
              {valid && value
                ? `Sends ${countdown(value.getTime())} · ${whenText(value.getTime())}`
                : "Pick a time in the future"}
            </p>
            <div className="flex justify-end gap-2">
              <Pill onClick={() => onOpenChange(false)}>Cancel</Pill>
              <Pill prominent disabled={!valid} onClick={() => value && onSchedule(value.getTime())}>
                Schedule
              </Pill>
            </div>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
