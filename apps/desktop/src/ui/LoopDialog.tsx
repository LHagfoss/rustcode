// New loop / Edit loop (issues 3892, 3937): a name, the project it runs in, the prompt, a schedule
// from the presets, a cron of the user's own or one time ("in 2h", "at 09:00"), where each run goes
// (a fresh thread, or one thread every time) and the agent a fresh run starts. Saved through
// `_rustcode/loops/save` into the scheduler daemon's store.

import { Dialog } from "radix-ui";
import { useEffect, useMemo, useState } from "react";
import {
  cronInWords,
  cronProblem,
  DEFAULT_PRESET,
  type Loop,
  type LoopDraft,
  parseOnce,
  presetOf,
  SCHEDULE_PRESETS,
  type SchedulePreset,
  saveLoop,
} from "../state/loops";
import { projectsOn } from "../state/projects";
import { errorText, useApp } from "../state/store";
import { cx, inputClass, noTextHelpers, Pill, Segments, Select, Switch } from "./primitives";

const EFFORTS = [
  { value: "", label: "Default" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
];

function draftOf(loop: Loop | null, project: string): LoopDraft {
  if (!loop) {
    return {
      name: "",
      workspace: project,
      prompt: "",
      cron: SCHEDULE_PRESETS.find((preset) => preset.id === DEFAULT_PRESET)?.cron ?? "0 */4 * * *",
      readOnly: true,
    };
  }
  return {
    id: loop.id,
    name: loop.name,
    workspace: loop.workspace,
    prompt: loop.prompt,
    cron: loop.schedule.kind === "cron" ? loop.schedule.expression : "0 9 * * *",
    harness: loop.harness,
    model: loop.model,
    effort: loop.effort,
    readOnly: loop.readOnly,
    threadId: loop.threadId,
  };
}

const pad = (n: number) => String(n).padStart(2, "0");

/** What the Once field shows for a loop saved with one time: the clock time it runs at. */
function onceTextOf(loop: Loop | null): string {
  if (loop?.schedule.kind !== "once") return "in 2h";
  const at = new Date(Date.parse(loop.schedule.at));
  return Number.isFinite(at.getTime()) ? `at ${pad(at.getHours())}:${pad(at.getMinutes())}` : "in 2h";
}

/** "Today at 14:30" or "2026-11-18 at 09:00": when a one-time run will happen. */
function onceInWords(at: Date, now = new Date()): string {
  const clock = `${pad(at.getHours())}:${pad(at.getMinutes())}`;
  const sameDay = at.toDateString() === now.toDateString();
  const tomorrow = new Date(now);
  tomorrow.setDate(now.getDate() + 1);
  if (sameDay) return `Once, today at ${clock}`;
  if (at.toDateString() === tomorrow.toDateString()) return `Once, tomorrow at ${clock}`;
  return `Once, ${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())} at ${clock}`;
}

const TARGETS = [
  { value: "fresh", label: "A new thread each run" },
  { value: "one", label: "One thread" },
] as const;

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <span className="font-medium text-caption text-fg-muted">{label}</span>
      {children}
    </div>
  );
}

export function LoopDialog({
  open,
  loop,
  onOpenChange,
  onSaved,
}: {
  open: boolean;
  /** The loop to edit; null makes a new one. */
  loop: Loop | null;
  onOpenChange: (open: boolean) => void;
  onSaved: () => void;
}) {
  const sessions = useApp((s) => s.sessions);
  const projectCwd = useApp((s) => s.projectCwd);
  const harnesses = useApp((s) => s.harnesses);
  const projects = useMemo(() => projectsOn(sessions), [sessions]);
  const [draft, setDraft] = useState<LoopDraft>(() => draftOf(loop, projectCwd));
  const [preset, setPreset] = useState<SchedulePreset>(DEFAULT_PRESET);
  const [onceText, setOnceText] = useState("in 2h");
  const [target, setTarget] = useState<"fresh" | "one">("fresh");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Each opening starts from the loop being edited, or a fresh draft in the current project.
  useEffect(() => {
    if (!open) return;
    const next = draftOf(loop, projectCwd || projects[0]?.path || "");
    setDraft(next);
    setPreset(loop?.schedule.kind === "once" ? "once" : loop ? presetOf(next.cron) : DEFAULT_PRESET);
    setOnceText(onceTextOf(loop));
    setTarget(loop?.threadId ? "one" : "fresh");
    setError(null);
    setSaving(false);
  }, [open, loop, projectCwd, projects]);

  const patch = (change: Partial<LoopDraft>) => setDraft((d) => ({ ...d, ...change }));
  const once = preset === "once" ? parseOnce(onceText) : null;
  const cronError =
    preset === "once"
      ? once
        ? null
        : "Write a time like “in 2h”, “in 30 min” or “at 09:00”"
      : cronProblem(draft.cron);
  const words = cronError ? null : once ? onceInWords(once) : cronInWords(draft.cron);
  // The threads a loop can send every run to: this computer's threads in the loop's project.
  const threadOptions = sessions
    .filter((session) => !session.host && session.cwd === draft.workspace)
    .map((session) => ({ value: session.id, label: session.title || "New thread" }));
  if (draft.threadId && !threadOptions.some((option) => option.value === draft.threadId)) {
    threadOptions.unshift({ value: draft.threadId, label: "The loop's thread" });
  }
  const threadId = target === "one" ? draft.threadId || threadOptions[0]?.value : undefined;
  const projectOptions = projects.map((project) => ({ value: project.path, label: project.name }));
  if (draft.workspace && !projectOptions.some((option) => option.value === draft.workspace)) {
    projectOptions.unshift({ value: draft.workspace, label: draft.workspace });
  }
  const harnessOptions = [
    { value: "", label: "Default" },
    ...(harnesses?.harnesses ?? [])
      .filter((harness) => harness.enabled || harness.id === draft.harness)
      .map((harness) => ({ value: harness.id, label: harness.name })),
  ];
  const ready =
    draft.name.trim() &&
    draft.prompt.trim() &&
    draft.workspace &&
    !cronError &&
    (target === "fresh" || threadId);

  const save = async () => {
    if (!ready || saving) return;
    setSaving(true);
    setError(null);
    try {
      await saveLoop({ ...draft, once: once ?? undefined, threadId });
      onSaved();
      onOpenChange(false);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="rc-overlay fixed inset-0 z-50 bg-bg/60" />
        <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center">
          <Dialog.Content
            aria-describedby={undefined}
            className="elevated pointer-events-auto flex max-h-[calc(100%-48px)] w-[min(520px,calc(100vw-48px))] flex-col gap-4 overflow-y-auto p-5"
          >
            <Dialog.Title className="font-semibold text-body">{loop ? "Edit loop" : "New loop"}</Dialog.Title>
            <Field label="Name">
              <input
                value={draft.name}
                onChange={(e) => patch({ name: e.target.value })}
                placeholder="CI watch"
                aria-label="Name"
                {...noTextHelpers}
                className={inputClass}
              />
            </Field>
            <Field label="Project">
              <Select
                label="Project"
                value={draft.workspace}
                options={projectOptions.length ? projectOptions : [{ value: "", label: "No projects yet" }]}
                onChange={(workspace) => patch({ workspace })}
                className="max-w-full self-start"
              />
            </Field>
            <Field label="Prompt">
              <textarea
                value={draft.prompt}
                onChange={(e) => patch({ prompt: e.target.value })}
                placeholder="Check CI on main and tell me only if something broke."
                aria-label="Prompt"
                rows={4}
                {...noTextHelpers}
                className="resize-none rounded-lg bg-fill-subtle px-3 py-2 text-callout text-fg placeholder:text-fg-faint hover:bg-fill focus:bg-fill"
              />
            </Field>
            <Field label="Schedule">
              <Segments
                label="Schedule"
                value={preset}
                options={SCHEDULE_PRESETS.map((p) => ({ value: p.id, label: p.label }))}
                onChange={(next) => {
                  setPreset(next);
                  const cron = SCHEDULE_PRESETS.find((p) => p.id === next)?.cron;
                  if (cron) patch({ cron });
                }}
              />
              {preset === "once" && (
                <input
                  value={onceText}
                  onChange={(e) => setOnceText(e.target.value)}
                  placeholder="in 2h, at 09:00"
                  aria-label="When"
                  {...noTextHelpers}
                  className={inputClass}
                />
              )}
              {preset === "custom" && (
                <input
                  value={draft.cron}
                  onChange={(e) => patch({ cron: e.target.value })}
                  placeholder="0 9 * * 1-5"
                  aria-label="Cron expression"
                  {...noTextHelpers}
                  className={cx(inputClass, "font-mono")}
                />
              )}
              <p
                aria-live="polite"
                className={cx("text-caption", cronError ? "text-danger" : "text-fg-muted")}
              >
                {cronError ?? words ?? `Cron ${draft.cron.trim()}`}
                {!cronError && !once && " · a run missed while the computer slept runs once when it wakes"}
              </p>
            </Field>
            <Field label="Each run">
              <Segments
                label="Each run"
                value={target}
                options={TARGETS.map((t) => ({ value: t.value, label: t.label }))}
                onChange={setTarget}
              />
              {target === "one" &&
                (threadOptions.length ? (
                  <Select
                    label="Thread"
                    value={threadId ?? ""}
                    options={threadOptions}
                    onChange={(id) => patch({ threadId: id })}
                    className="max-w-full self-start"
                  />
                ) : (
                  <p className="text-caption text-danger">This project has no threads yet.</p>
                ))}
              <p className="text-caption text-fg-muted">
                {target === "one"
                  ? "Every run is a new message in this thread; a run while it is busy is skipped."
                  : "A run that finds nothing to report archives its thread."}
              </p>
            </Field>
            {target === "fresh" && (
              <Field label="Agent">
                <div className="flex flex-wrap items-center gap-2">
                  <Select
                    label="Harness"
                    value={draft.harness ?? ""}
                    options={harnessOptions}
                    onChange={(harness) => patch({ harness })}
                  />
                  <input
                    value={draft.model ?? ""}
                    onChange={(e) => patch({ model: e.target.value })}
                    placeholder="Model (default)"
                    aria-label="Model"
                    {...noTextHelpers}
                    className={cx(inputClass, "min-w-0 flex-1")}
                  />
                </div>
                <Segments
                  label="Effort"
                  value={draft.effort ?? ""}
                  options={EFFORTS}
                  onChange={(effort) => patch({ effort })}
                />
              </Field>
            )}
            <div className="flex items-start justify-between gap-4">
              <div className="flex flex-col">
                <span className="text-callout text-fg">Read-only</span>
                <span className="text-caption text-fg-muted">
                  Edits, commits, pushes and anything else that asks for approval are declined.
                </span>
              </div>
              <Switch
                label="Read-only"
                checked={draft.readOnly}
                onChange={(readOnly) => patch({ readOnly })}
              />
            </div>
            {error && (
              <p role="alert" className="text-callout text-danger">
                {error}
              </p>
            )}
            <div className="flex justify-end gap-2">
              <Pill onClick={() => onOpenChange(false)}>Cancel</Pill>
              <Pill prominent disabled={!ready || saving} onClick={() => void save()}>
                {loop ? "Save" : "Create loop"}
              </Pill>
            </div>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
