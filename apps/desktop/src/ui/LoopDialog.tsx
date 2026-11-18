// New loop / Edit loop (issues 3892, 3937): a name, the project it runs in, the prompt, a schedule
// (every few hours, on chosen weekdays at a time, monthly, once at a date and time, or a cron of
// the user's own), where each run goes (a fresh thread, or one thread every time) and the agent a
// fresh run starts: a harness, one of its models and an effort. Saved through
// `_rustcode/loops/save` into the scheduler daemon's store.

import { Dialog } from "radix-ui";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  cronInWords,
  cronOfForm,
  defaultForm,
  EVERY_DAY,
  formOfCron,
  formProblem,
  HOUR_STEPS,
  type Loop,
  type LoopDraft,
  localDateTime,
  onceOfForm,
  type Repeat,
  type ScheduleForm,
  saveLoop,
  userTimeZone,
  WEEKDAYS,
  WEEKENDS,
} from "../state/loops";
import { projectsOn } from "../state/projects";
import { errorText, useApp } from "../state/store";
import { cx, inputClass, noTextHelpers, Pill, Segments, Select, Switch } from "./primitives";

const EFFORTS = ["low", "medium", "high"];

const REPEATS: { value: Repeat; label: string }[] = [
  { value: "hours", label: "Hourly" },
  { value: "days", label: "Weekly" },
  { value: "monthly", label: "Monthly" },
  { value: "once", label: "Once" },
  { value: "cron", label: "Cron" },
];

/** The day toggles, Monday first. */
const DAY_TOGGLES = [
  { day: 1, label: "Mon" },
  { day: 2, label: "Tue" },
  { day: 3, label: "Wed" },
  { day: 4, label: "Thu" },
  { day: 5, label: "Fri" },
  { day: 6, label: "Sat" },
  { day: 0, label: "Sun" },
];

const DAY_SHORTCUTS = [
  { label: "Every day", days: EVERY_DAY },
  { label: "Weekdays", days: WEEKDAYS },
  { label: "Weekends", days: WEEKENDS },
];

function ordinal(n: number): string {
  const tens = n % 100;
  if (tens >= 11 && tens <= 13) return `${n}th`;
  return `${n}${{ 1: "st", 2: "nd", 3: "rd" }[n % 10] ?? "th"}`;
}

const MONTH_DAYS = Array.from({ length: 28 }, (_, i) => ({ value: String(i + 1), label: ordinal(i + 1) }));

function draftOf(loop: Loop | null, project: string): LoopDraft {
  if (!loop) {
    return { name: "", workspace: project, prompt: "", cron: "0 */4 * * *", readOnly: true };
  }
  return {
    id: loop.id,
    name: loop.name,
    workspace: loop.workspace,
    prompt: loop.prompt,
    cron: loop.schedule.kind === "cron" ? loop.schedule.expression : "0 9 * * *",
    timezone: "timezone" in loop.schedule ? loop.schedule.timezone : undefined,
    harness: loop.harness,
    account: loop.account,
    model: loop.model,
    effort: loop.effort?.toLowerCase() === "default" ? undefined : loop.effort,
    readOnly: loop.readOnly,
    threadId: loop.threadId,
  };
}

const pad = (n: number) => String(n).padStart(2, "0");

/** The schedule form a loop opens on: its own schedule, or every 4 hours for a new one. */
function formOf(loop: Loop | null): ScheduleForm {
  const base = defaultForm();
  if (!loop) return base;
  const schedule = loop.schedule;
  switch (schedule.kind) {
    case "once": {
      const at = new Date(Date.parse(schedule.at));
      return {
        ...base,
        repeat: "once",
        onceAt: Number.isFinite(at.getTime()) ? localDateTime(at) : base.onceAt,
      };
    }
    case "daily":
      return { ...base, repeat: "days", time: `${pad(schedule.hour)}:${pad(schedule.minute)}` };
    case "monthly":
      return formOfCron(`${schedule.minute} ${schedule.hour} ${schedule.day} * *`, base);
    case "cron":
      return formOfCron(schedule.expression, base);
  }
}

/** A shortcut for the Once field: in an hour, or tomorrow morning. */
function onceShortcuts(now = new Date()): { label: string; at: string }[] {
  const inHour = new Date(now.getTime() + 3_600_000);
  const tomorrow = new Date(now);
  tomorrow.setDate(now.getDate() + 1);
  tomorrow.setHours(9, 0, 0, 0);
  return [
    { label: "In 1 hour", at: localDateTime(inHour) },
    { label: "Tomorrow 09:00", at: localDateTime(tomorrow) },
  ];
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
  { value: "fresh", label: "New thread" },
  { value: "one", label: "Existing thread" },
] as const;

function TimeInput({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  return (
    <input
      type="time"
      value={value}
      onChange={(e) => onChange(e.target.value)}
      aria-label="Time"
      className={cx(inputClass, "w-auto")}
    />
  );
}

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
  const [form, setForm] = useState<ScheduleForm>(() => formOf(loop));
  const [target, setTarget] = useState<"fresh" | "one">("fresh");
  const [customDays, setCustomDays] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const fallbackProject = projectCwd || projects[0]?.path || "";

  // Each opening starts from the loop being edited, or a fresh draft in the current project, once:
  // the thread list and the loops page refresh while the dialog is open, and must not wipe what
  // the user is typing.
  const openedFor = useRef<string | null>(null);
  useEffect(() => {
    if (!open) {
      openedFor.current = null;
      return;
    }
    const key = loop?.id ?? "";
    if (openedFor.current === key) return;
    openedFor.current = key;
    setDraft(draftOf(loop, fallbackProject));
    setForm(formOf(loop));
    setTarget(loop?.threadId ? "one" : "fresh");
    setCustomDays(false);
    setError(null);
    setSaving(false);
  }, [open, loop, fallbackProject]);

  // A new loop opened before the projects were read takes the first one when they arrive.
  useEffect(() => {
    if (open && fallbackProject) setDraft((d) => (d.workspace ? d : { ...d, workspace: fallbackProject }));
  }, [open, fallbackProject]);

  const patch = (change: Partial<LoopDraft>) => setDraft((d) => ({ ...d, ...change }));
  const patchForm = (change: Partial<ScheduleForm>) => setForm((f) => ({ ...f, ...change }));
  const cron = cronOfForm(form);
  const once = form.repeat === "once" ? onceOfForm(form) : null;
  const cronError = formProblem(form);
  const words = cronError ? null : once ? onceInWords(once) : cronInWords(cron);
  // The threads a loop can send every run to: this computer's threads in the loop's project.
  const threadOptions = sessions
    .filter((session) => !session.host && session.cwd === draft.workspace)
    .map((session) => ({ value: session.id, label: session.title || "New thread" }));
  if (draft.threadId && !threadOptions.some((option) => option.value === draft.threadId)) {
    threadOptions.unshift({ value: draft.threadId, label: "The loop's thread" });
  }
  const threadId = target === "one" ? draft.threadId || threadOptions[0]?.value : undefined;
  const timezone = draft.timezone ?? userTimeZone();
  const daysChoice =
    DAY_SHORTCUTS.find(
      ({ days }) => days.length === form.days.length && days.every((day) => form.days.includes(day)),
    )?.label ?? "Custom days";
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
  // The models of the harness a run starts on: the chosen one, else the default harness.
  const runHarness = harnesses?.harnesses.find(
    (harness) => harness.id === (draft.harness || harnesses.default.harness),
  );
  const accounts = runHarness?.accounts ?? [];
  const accountOptions = [
    { value: "", label: "Default account" },
    ...accounts.map((account) => ({ value: account.id, label: account.name })),
  ];
  if (draft.account && !accounts.some((account) => account.id === draft.account)) {
    accountOptions.push({ value: draft.account, label: `${draft.account} (unavailable)` });
  }
  const models = runHarness?.models ?? [];
  const modelOptions = [
    { value: "", label: "Default model" },
    ...models.map((model) => ({ value: model.id, label: model.name || model.id })),
  ];
  if (draft.model && !models.some((model) => model.id === draft.model)) {
    modelOptions.push({ value: draft.model, label: draft.model });
  }
  const defaultModel =
    harnesses?.default.harness === runHarness?.id ? harnesses?.default.model : runHarness?.last?.model;
  const chosenModel = models.find((model) => model.id === (draft.model || defaultModel));
  const effortValues = [...new Set(chosenModel?.efforts.length ? chosenModel.efforts : EFFORTS)].filter(
    (effort) => effort.toLowerCase() !== "default",
  );
  const effortOptions = [
    { value: "", label: "Default" },
    ...effortValues.map((effort) => ({ value: effort, label: effort[0]?.toUpperCase() + effort.slice(1) })),
  ];
  if (draft.effort && !effortValues.includes(draft.effort)) {
    effortOptions.push({ value: draft.effort, label: draft.effort });
  }
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
      await saveLoop({ ...draft, cron, once: once ?? undefined, threadId });
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
            className="elevated pointer-events-auto flex max-h-[calc(100%-48px)] w-[min(580px,calc(100vw-48px))] flex-col overflow-hidden"
          >
            <div className="flex shrink-0 flex-col gap-1 px-5 pt-5 pb-3">
              <Dialog.Title className="font-semibold text-title3">
                {loop ? "Edit loop" : "New loop"}
              </Dialog.Title>
              <p className="text-caption text-fg-muted">Runs while RustCode is open.</p>
            </div>
            <div className="flex min-h-0 flex-col gap-4 overflow-y-auto px-5 pb-3">
              <div className="grid grid-cols-[minmax(0,2fr)_minmax(0,1fr)] items-end gap-3">
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
                    options={
                      projectOptions.length ? projectOptions : [{ value: "", label: "No projects yet" }]
                    }
                    onChange={(workspace) => patch({ workspace, threadId: undefined })}
                    className="h-8 max-w-full justify-between rounded-md bg-fill-subtle"
                  />
                </Field>
              </div>
              <Field label="Prompt">
                <textarea
                  value={draft.prompt}
                  onChange={(e) => patch({ prompt: e.target.value })}
                  placeholder="Check CI on main and tell me only if something broke."
                  aria-label="Prompt"
                  rows={3}
                  {...noTextHelpers}
                  className="max-h-56 min-h-18 resize-y rounded-lg bg-fill-subtle px-3 py-2 text-callout text-fg placeholder:text-fg-faint hover:bg-fill focus:bg-fill"
                />
              </Field>
              <Field label="Schedule">
                <Segments
                  label="Repeat"
                  value={form.repeat}
                  options={REPEATS}
                  onChange={(repeat) =>
                    // Switching to Cron starts from what the other choices made, to edit by hand.
                    patchForm(repeat === "cron" && form.repeat !== "once" ? { repeat, cron } : { repeat })
                  }
                />
                {form.repeat === "hours" && (
                  <div className="flex items-center gap-2 text-callout text-fg-muted">
                    <span>Every</span>
                    <Select
                      label="Hours between runs"
                      value={String(form.everyHours)}
                      options={HOUR_STEPS.map((n) => ({
                        value: String(n),
                        label: n === 1 ? "hour" : `${n} hours`,
                      }))}
                      onChange={(n) => patchForm({ everyHours: Number(n) })}
                    />
                  </div>
                )}
                {form.repeat === "days" && (
                  <>
                    <div className="flex flex-wrap items-center gap-2">
                      <Select
                        label="Days to run"
                        value={customDays ? "Custom days" : daysChoice}
                        options={[
                          ...DAY_SHORTCUTS.map(({ label }) => ({ value: label, label })),
                          {
                            value: "Custom days",
                            label: "Custom days",
                          },
                        ]}
                        onChange={(value) => {
                          const preset = DAY_SHORTCUTS.find(({ label }) => label === value);
                          setCustomDays(!preset);
                          if (preset) patchForm({ days: [...preset.days] });
                        }}
                      />
                      <span className="text-callout text-fg-muted">at</span>
                      <TimeInput value={form.time} onChange={(time) => patchForm({ time })} />
                    </div>
                    {(customDays || daysChoice === "Custom days") && (
                      <div role="group" aria-label="Days" className="flex flex-wrap gap-1">
                        {DAY_TOGGLES.map(({ day, label }) => {
                          const on = form.days.includes(day);
                          return (
                            <Pill
                              key={day}
                              size="sm"
                              prominent={on}
                              aria-pressed={on}
                              className="w-11"
                              onClick={() =>
                                setForm((f) => ({
                                  ...f,
                                  days: f.days.includes(day)
                                    ? f.days.filter((d) => d !== day)
                                    : [...f.days, day],
                                }))
                              }
                            >
                              {label}
                            </Pill>
                          );
                        })}
                      </div>
                    )}
                  </>
                )}
                {form.repeat === "monthly" && (
                  <div className="flex items-center gap-2 text-callout text-fg-muted">
                    <span>On the</span>
                    <Select
                      label="Day of the month"
                      value={String(form.monthDay)}
                      options={MONTH_DAYS}
                      onChange={(day) => patchForm({ monthDay: Number(day) })}
                    />
                    <span>at</span>
                    <TimeInput value={form.time} onChange={(time) => patchForm({ time })} />
                  </div>
                )}
                {form.repeat === "once" && (
                  <div className="flex flex-wrap items-center gap-2">
                    <input
                      type="datetime-local"
                      value={form.onceAt}
                      min={localDateTime(new Date())}
                      onChange={(e) => patchForm({ onceAt: e.target.value })}
                      aria-label="Date and time"
                      className={cx(inputClass, "w-auto")}
                    />
                    {onceShortcuts().map((shortcut) => (
                      <Pill
                        key={shortcut.label}
                        quiet
                        size="sm"
                        onClick={() => patchForm({ onceAt: shortcut.at })}
                      >
                        {shortcut.label}
                      </Pill>
                    ))}
                  </div>
                )}
                {form.repeat === "cron" && (
                  <input
                    value={form.cron}
                    onChange={(e) => patchForm({ cron: e.target.value })}
                    placeholder="0 9 * * MON-FRI"
                    aria-label="Cron expression"
                    {...noTextHelpers}
                    className={cx(inputClass, "font-mono")}
                  />
                )}
                <p
                  aria-live="polite"
                  className={cx("text-caption", cronError ? "text-danger" : "text-fg-muted")}
                >
                  {cronError ?? words ?? `Cron ${cron}`}
                  {!cronError && !once && timezone && ` · ${timezone}`}
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
                    ? "Sends the prompt to this thread when it’s idle."
                    : "Archives the thread when there’s nothing to report."}
                </p>
              </Field>
              {target === "fresh" && (
                <div className="grid grid-cols-2 gap-x-3 gap-y-2 sm:grid-cols-4">
                  <Field label="Harness">
                    <Select
                      label="Harness"
                      value={draft.harness ?? ""}
                      options={harnessOptions}
                      onChange={(harness) => patch({ harness, account: "", model: "", effort: "" })}
                    />
                  </Field>
                  <Field label="Account">
                    <Select
                      label="Account"
                      value={draft.account ?? ""}
                      options={accountOptions}
                      disabled={accountOptions.length === 1}
                      onChange={(account) =>
                        patch({
                          account,
                          harness: account ? runHarness?.id : draft.harness,
                        })
                      }
                    />
                  </Field>
                  <Field label="Model">
                    {models.length ? (
                      <Select
                        label="Model"
                        value={draft.model ?? ""}
                        options={modelOptions}
                        onChange={(model) => patch({ model, effort: "" })}
                      />
                    ) : (
                      <input
                        value={draft.model ?? ""}
                        onChange={(e) => patch({ model: e.target.value, effort: "" })}
                        placeholder="Default model"
                        aria-label="Model"
                        {...noTextHelpers}
                        className={cx(inputClass, "h-[26px] min-w-0")}
                      />
                    )}
                  </Field>
                  <Field label="Effort">
                    <Select
                      label="Effort"
                      value={draft.effort ?? ""}
                      options={effortOptions}
                      onChange={(effort) => patch({ effort })}
                    />
                  </Field>
                </div>
              )}
              <div className="flex items-start justify-between gap-4">
                <div className="flex flex-col">
                  <span className="text-callout text-fg">Read-only</span>
                  <span className="text-caption text-fg-muted">
                    {draft.readOnly
                      ? "Decline approvals for edits, commits, pushes, and other actions."
                      : "Runs with full access, including edits, commits, and pushes."}
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
            </div>
            <div className="flex shrink-0 justify-end gap-2 border-t border-border px-5 py-3">
              <Pill onClick={() => onOpenChange(false)}>Cancel</Pill>
              <Pill prominent disabled={!ready || saving} onClick={() => void save()}>
                {saving ? "Saving…" : loop ? "Save changes" : "Create loop"}
              </Pill>
            </div>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
