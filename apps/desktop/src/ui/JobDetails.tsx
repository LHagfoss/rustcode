import { Dialog } from "radix-ui";
import { useRef } from "react";
import { projectName } from "../acp/types";
import { runThreadKey } from "../state/fleetLoops";
import { type Loop, nextRunText, scheduleInWords } from "../state/loops";
import { selfName, useLocalIdentity, useMachines } from "../state/machines";
import { FULL_RUN_FORMAT, isFailedRun, RunList } from "./JobRuns";
import { Pill } from "./primitives";

export function JobDetails({
  loop,
  now,
  machine,
  onClose,
  onEdit,
}: {
  loop: Loop;
  now: number;
  machine: string;
  onClose: () => void;
  onEdit: () => void;
}) {
  const returnFocus = useRef<HTMLElement | null>(null);
  const machines = useMachines((s) => s.machines);
  const local = useLocalIdentity();
  const localName = local.displayName ?? selfName();
  const next = !loop.paused && loop.schedule.kind !== "manual" ? loop.nextDueAt : undefined;
  const stats = [
    { label: "Recorded runs", value: loop.runs.length },
    { label: "Succeeded", value: loop.runs.filter((run) => run.state === "succeeded").length },
    { label: "Failed", value: loop.runs.filter(isFailedRun).length },
  ];

  return (
    <Dialog.Root open onOpenChange={(open) => !open && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center p-4">
          <Dialog.Content
            onOpenAutoFocus={() => {
              returnFocus.current =
                document.activeElement instanceof HTMLElement ? document.activeElement : null;
            }}
            onCloseAutoFocus={(event) => {
              event.preventDefault();
              if (document.activeElement === document.body) returnFocus.current?.focus();
            }}
            className="elevated pointer-events-auto flex max-h-full w-full max-w-xl flex-col overflow-hidden"
          >
            <header className="shrink-0 px-5 pt-5 pb-4">
              <Dialog.Title className="break-words font-semibold text-title2">{loop.name}</Dialog.Title>
              <Dialog.Description className="mt-1 text-callout text-fg-muted">
                {projectName(loop.workspace)} · {machine} · {scheduleInWords(loop.schedule)}
              </Dialog.Description>
              <p className="mt-1 text-caption text-fg-muted">
                {next === undefined ? nextRunText(loop, now) : `Next run ${FULL_RUN_FORMAT.format(next)}`}
              </p>
            </header>
            <div className="min-h-0 overflow-y-auto px-5 pb-4">
              <dl className="grid grid-cols-3 gap-3 rounded-lg bg-fill-subtle p-3">
                {stats.map(({ label, value }) => (
                  <div key={label} className="flex flex-col-reverse">
                    <dt className="text-caption text-fg-muted">{label}</dt>
                    <dd className="tabular font-semibold text-title2">{value}</dd>
                  </div>
                ))}
              </dl>
              <h3 className="mt-4 mb-1 font-semibold text-body">Run history</h3>
              <RunList
                runs={loop.runs}
                now={now}
                threadOf={(run) => runThreadKey(loop, run, machines, localName)}
                onOpenThread={onClose}
              />
            </div>
            <footer className="flex shrink-0 justify-end gap-2 border-t border-border px-5 py-3">
              <Pill onClick={onEdit}>Edit job</Pill>
              <Dialog.Close asChild>
                <Pill>Done</Pill>
              </Dialog.Close>
            </footer>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
