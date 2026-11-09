import { Dialog } from "radix-ui";
import { useState } from "react";
import { askCustomSnooze, snoozeThread, useApp } from "../state/store";
import { DateTimePicker } from "./DateTimePicker";
import { Pill } from "./primitives";

export function CustomSnooze() {
  const id = useApp((s) => s.snoozeAsk);
  const [value, setValue] = useState<Date | null>(() => new Date(Date.now() + 3600000));
  const valid = value !== null && value.getTime() > Date.now();
  function submit() {
    if (id && value && value.getTime() > Date.now()) {
      snoozeThread(id, value.getTime());
      askCustomSnooze(null);
    }
  }
  return (
    <Dialog.Root
      open={!!id}
      onOpenChange={(open) => {
        if (!open) askCustomSnooze(null);
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <div className="fixed inset-0 z-50 flex items-center justify-center pointer-events-none">
          <Dialog.Content
            aria-describedby={undefined}
            className="elevated pointer-events-auto flex w-[min(360px,calc(100vw-48px))] flex-col gap-3 p-5"
            onKeyDown={(e) => {
              if (e.key === "Enter" && e.target instanceof HTMLInputElement) {
                e.preventDefault();
                submit();
              }
            }}
          >
            <Dialog.Title className="font-semibold text-body">Snooze until</Dialog.Title>
            <DateTimePicker value={value ?? new Date()} onChange={setValue} />
            <p aria-live="polite" className="text-fg-muted">
              {value
                ? `Wakes ${value.toLocaleString(undefined, { weekday: "short", day: "numeric", month: "short", hour: "numeric", minute: "2-digit" })} (in ${Math.max(0, Math.ceil((value.getTime() - Date.now()) / 86400000))} days)`
                : "Enter a valid time"}
            </p>
            <div className="flex justify-end gap-2">
              <Pill onClick={() => askCustomSnooze(null)}>Cancel</Pill>
              <Pill prominent disabled={!valid} onClick={submit}>
                Snooze
              </Pill>
            </div>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
