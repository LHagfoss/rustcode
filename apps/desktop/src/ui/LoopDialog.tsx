// New / Edit scheduled job (issues 3892, 3937, 3990): a name, the project it runs in, the prompt, a schedule
// (every few hours, on chosen weekdays at a time, monthly, once at a date and time, or a cron of
// the user's own), where each run goes (a fresh thread, or one thread every time) and the agent a
// fresh run starts: a harness, one of its models and an effort. A webhook can start runs too, with
// or without a schedule; its URL is shown once, after the save that made it. Saved through
// `_rustcode/loops/save` into the scheduler daemon's store.

import { Dialog } from "radix-ui";
import { useEffect, useMemo, useRef, useState } from "react";
import { effortLabel, projectName } from "../acp/types";
import { concreteModels, defaultEffort, firstModel, listedModel, restoreChoice } from "../state/harness";
import { ensureHarnessModels, lastAccountFor } from "../state/harnessActions";
import { refreshHub, useHub } from "../state/hub";
import { newThreadAccountOn } from "../state/logins";
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
  type LoopWebhook,
  localDateTime,
  onceOfForm,
  type Repeat,
  type ScheduleForm,
  saveLoop,
  userTimeZone,
  WEEKDAYS,
  WEEKENDS,
  withRunAccount,
} from "../state/loops";
import { projectsOn } from "../state/projects";
import { errorText, useApp } from "../state/store";
import { ChevronRight, Webhook } from "./icons";
import { CopyButton } from "./Markdown";
import { Callout, cx, inputClass, noTextHelpers, Pill, Segments, Select, Switch } from "./primitives";

const EFFORTS = ["low", "medium", "high"];

const REPEATS: { value: Repeat; label: string }[] = [
  { value: "hours", label: "Hourly" },
  { value: "days", label: "Weekly" },
  { value: "monthly", label: "Monthly" },
  { value: "once", label: "Once" },
  { value: "cron", label: "Cron" },
  { value: "manual", label: "Manual" },
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
    effort: loop.effort,
    readOnly: loop.readOnly,
    threadId: loop.threadId,
    script: loop.script,
    webhook: loop.webhook,
    machine: loop.machine,
  };
}

/** A folder path as another machine would read it: absolute on macOS, Linux or Windows. */
const absoluteOnAnyMachine = (path: string) => /^(\/|[A-Za-z]:[\\/])/.test(path.trim());

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
    case "manual":
      return { ...base, repeat: "manual" };
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
    <div className="flex min-w-0 flex-col gap-1">
      <span className="font-medium text-caption text-fg-muted">{label}</span>
      {children}
    </div>
  );
}

/** One part of the form (what, when, how), under a heading and a rule after the first. */
function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section
      aria-label={title}
      className="flex flex-col gap-3 border-border border-t pt-4 first:border-t-0 first:pt-0"
    >
      <h3 className="font-semibold text-callout text-fg">{title}</h3>
      {children}
    </section>
  );
}

/** A switch with its name and what it does now. */
function ToggleRow({
  label,
  hint,
  checked,
  onChange,
}: {
  label: string;
  hint: string;
  checked: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <div className="flex items-start justify-between gap-4">
      <div className="flex min-w-0 flex-col">
        <span className="text-callout text-fg">{label}</span>
        <span className="text-caption text-fg-muted">{hint}</span>
      </div>
      <Switch label={label} checked={checked} onChange={onChange} />
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
  // The optional script stays folded until asked for, or when the job already has one.
  const [scriptOpen, setScriptOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The webhook URL a save just made: shown once, in place of the form, until the dialog closes.
  const [madeWebhook, setMadeWebhook] = useState<LoopWebhook | null>(null);
  const fallbackProject = projectCwd || projects[0]?.path || "";
  // On the fleet's hub, a Loop may run on a member: the hub keeps time and holds runs while it sleeps.
  const hub = useHub((s) => s.status);
  useEffect(() => {
    if (open) void refreshHub();
  }, [open]);
  const members = hub?.hub ? (hub.members ?? []) : [];
  const remote = Boolean(draft.machine);
  const machineOptions = [
    { value: "", label: "This computer" },
    ...members.map((member) => ({
      value: member.name,
      label: member.online ? member.name : `${member.name} (offline)`,
    })),
  ];
  if (draft.machine && !machineOptions.some((option) => option.value === draft.machine)) {
    machineOptions.push({ value: draft.machine, label: `${draft.machine} (not a member)` });
  }
  const chooseMachine = (machine: string) => {
    patch({
      machine: machine || undefined,
      workspace: machine ? "" : fallbackProject,
      threadId: undefined,
      script: undefined,
      account: "",
    });
    if (machine) {
      setTarget("fresh");
      setScriptOpen(false);
    }
  };

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
    setScriptOpen(Boolean(loop?.script));
    setError(null);
    setSaving(false);
    setMadeWebhook(null);
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
  const manual = form.repeat === "manual";
  const words = cronError
    ? null
    : manual
      ? draft.webhook
        ? "Runs when its webhook is called, or from Run now"
        : "Runs only from Run now"
      : once
        ? onceInWords(once)
        : cronInWords(cron);
  // The threads a loop can send every run to: this computer's threads in the loop's project.
  const threadOptions = sessions
    .filter((session) => !session.host && session.cwd === draft.workspace)
    .map((session) => ({ value: session.id, label: session.title || "New thread" }));
  if (draft.threadId && !threadOptions.some((option) => option.value === draft.threadId)) {
    threadOptions.unshift({ value: draft.threadId, label: "The job's thread" });
  }
  const threadId = target === "one" ? draft.threadId || threadOptions[0]?.value : undefined;
  const timezone = draft.timezone ?? userTimeZone();
  const daysChoice =
    DAY_SHORTCUTS.find(
      ({ days }) => days.length === form.days.length && days.every((day) => form.days.includes(day)),
    )?.label ?? "Custom days";
  const projectOptions = projects.map((project) => ({ value: project.path, label: project.name }));
  if (draft.workspace && !projectOptions.some((option) => option.value === draft.workspace)) {
    projectOptions.unshift({ value: draft.workspace, label: projectName(draft.workspace) });
  }
  const runHarnessId = draft.harness || harnesses?.default.harness;
  const runHarness = harnesses?.harnesses.find((harness) => harness.id === runHarnessId);
  const harnessOptions = (harnesses?.harnesses ?? [])
    .filter((harness) => harness.enabled || harness.id === runHarnessId)
    .map((harness) => ({ value: harness.id, label: harness.name }));
  if (runHarnessId && !runHarness) {
    harnessOptions.push({ value: runHarnessId, label: `${runHarnessId} (unavailable)` });
  }
  useEffect(() => {
    if (open && target === "fresh" && runHarnessId) void ensureHarnessModels(runHarnessId);
  }, [open, target, runHarnessId]);
  const accounts = runHarness?.accounts ?? [];
  // A job with no account runs on the one a new thread would get, never on the bare default login
  // while named accounts exist; the picker shows it, and saving keeps it.
  const signedOut = useApp((s) => s.signedOutLogins);
  const runAccount = runHarness
    ? newThreadAccountOn(runHarness, lastAccountFor(runHarness.id), signedOut)
    : null;
  const accountOptions = accounts.map((account) => ({ value: account.id, label: account.name }));
  if (draft.account && !accounts.some((account) => account.id === draft.account)) {
    accountOptions.push({ value: draft.account, label: `${draft.account} (unavailable)` });
  }
  const models = runHarness ? concreteModels(runHarness.id, runHarness.models) : [];
  const remembered = harnesses && runHarness ? restoreChoice(harnesses, runHarness.id) : null;
  const modelId = draft.model || remembered?.model;
  const chosenModel =
    listedModel(runHarnessId ?? "", models, modelId) ??
    (!draft.model || draft.model === "default" ? firstModel(runHarnessId ?? "", models) : undefined);
  const runModel = chosenModel?.id ?? (draft.model === "default" ? undefined : draft.model);
  const modelOptions = models.map((model) => ({ value: model.id, label: model.name || model.id }));
  if (runModel && !chosenModel) {
    modelOptions.push({ value: runModel, label: runModel });
  }
  const effortValues = chosenModel?.efforts ?? (runHarness?.capabilities.effort ? EFFORTS : []);
  const runEffort =
    draft.effort ||
    (chosenModel && remembered?.model === chosenModel.id ? remembered.effort : undefined) ||
    (chosenModel ? defaultEffort(chosenModel, effortValues) : undefined);
  const effortOptions = effortValues.map((effort) => ({ value: effort, label: effortLabel(effort) }));
  if (runEffort && !effortValues.includes(runEffort)) {
    effortOptions.push({ value: runEffort, label: effortLabel(runEffort) });
  }
  const ready =
    draft.name.trim() &&
    draft.prompt.trim() &&
    draft.workspace &&
    (!remote || absoluteOnAnyMachine(draft.workspace)) &&
    !cronError &&
    (target === "fresh" || threadId);

  const save = async () => {
    if (!ready || saving) return;
    setSaving(true);
    setError(null);
    try {
      const agent = threadId
        ? {}
        : { harness: runHarnessId, model: runModel ?? draft.model, effort: runEffort ?? undefined };
      const made = await saveLoop({
        ...withRunAccount({ ...draft, ...agent, threadId }, runHarness?.id, runAccount),
        cron,
        once: once ?? undefined,
        manual,
      });
      onSaved();
      if (made) setMadeWebhook(made);
      else onOpenChange(false);
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
            // Radix focuses the first field with its text selected, so one keystroke would
            // replace an existing job's name. An edit opens with nothing focused; a new job
            // starts in its empty Name field.
            onOpenAutoFocus={(e) => {
              if (!loop) return;
              e.preventDefault();
              (e.currentTarget as HTMLElement).focus({ preventScroll: true });
            }}
            className="elevated pointer-events-auto flex max-h-[calc(100%-48px)] w-[min(580px,calc(100vw-48px))] flex-col overflow-hidden"
          >
            {madeWebhook ? (
              <WebhookMade webhook={madeWebhook} onDone={() => onOpenChange(false)} />
            ) : (
              <>
                <div className="flex shrink-0 flex-col gap-1 px-5 pt-5 pb-3">
                  <Dialog.Title className="font-semibold text-title3">
                    {loop ? "Edit scheduled job" : "New scheduled job"}
                  </Dialog.Title>
                  <p className="text-caption text-fg-muted">
                    {remote
                      ? `This hub keeps the schedule and sends each run to ${draft.machine}. While it’s offline, runs wait here.`
                      : "Runs while RustCode is open."}
                  </p>
                </div>
                <div className="flex min-h-0 flex-col gap-4 overflow-y-auto px-5 pt-1 pb-4">
                  <Section title="Task">
                    {machineOptions.length > 1 && (
                      <Field label="Machine">
                        <Select
                          label="Machine"
                          value={draft.machine ?? ""}
                          options={machineOptions}
                          onChange={chooseMachine}
                          className="h-8 max-w-full justify-between self-start rounded-md bg-fill-subtle"
                        />
                      </Field>
                    )}
                    <div className="grid grid-cols-[minmax(0,2fr)_minmax(0,1fr)] items-start gap-3">
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
                        {remote ? (
                          <input
                            value={draft.workspace}
                            onChange={(e) => patch({ workspace: e.target.value })}
                            placeholder={`Absolute folder on ${draft.machine}`}
                            aria-label="Project"
                            {...noTextHelpers}
                            className={cx(inputClass, "font-mono")}
                          />
                        ) : (
                          <Select
                            label="Project"
                            value={draft.workspace}
                            options={
                              projectOptions.length
                                ? projectOptions
                                : [{ value: "", label: "No projects yet" }]
                            }
                            onChange={(workspace) => patch({ workspace, threadId: undefined })}
                            className="h-8 max-w-full justify-between rounded-md bg-fill-subtle"
                          />
                        )}
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
                    {remote ? null : scriptOpen ? (
                      <Field label="Script (optional)">
                        <textarea
                          value={draft.script ?? ""}
                          onChange={(e) => patch({ script: e.target.value })}
                          placeholder="gh run list --branch main --status failure --limit 5"
                          aria-label="Script"
                          rows={2}
                          {...noTextHelpers}
                          className="max-h-40 min-h-12 resize-y rounded-lg bg-fill-subtle px-3 py-2 font-mono text-caption text-fg placeholder:text-fg-faint hover:bg-fill focus:bg-fill"
                        />
                        <p className="text-caption text-fg-muted">
                          Runs first in the project folder. The agent starts only when it prints something,
                          with that output added to the prompt; no output costs no tokens.
                        </p>
                      </Field>
                    ) : (
                      <button
                        type="button"
                        onClick={() => setScriptOpen(true)}
                        className="-mx-1 flex items-center gap-1 self-start rounded-md px-1 py-0.5 text-caption text-fg-muted hover:bg-fill-subtle hover:text-fg"
                      >
                        <ChevronRight size={12} />
                        Run a script first, and start the agent only when it prints something
                      </button>
                    )}
                  </Section>
                  <Section title="When it runs">
                    <Field label="Repeat">
                      <Segments
                        label="Repeat"
                        value={form.repeat}
                        options={REPEATS}
                        onChange={(repeat) =>
                          // Switching to Cron starts from what the other choices made, to edit by hand.
                          patchForm(
                            repeat === "cron" && form.repeat !== "once" ? { repeat, cron } : { repeat },
                          )
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
                        {!cronError && !once && !manual && timezone && ` · ${timezone}`}
                      </p>
                    </Field>
                    <ToggleRow
                      label="Webhook"
                      hint={
                        draft.webhook
                          ? "A POST to the job’s URL starts a run, with the request body added to the prompt as data."
                          : "Also start runs from GitHub or another service by calling a URL."
                      }
                      checked={draft.webhook ?? false}
                      onChange={(webhook) => patch({ webhook, rotateWebhook: false })}
                    />
                    {draft.webhook && loop?.webhook && (
                      <div className="flex items-center gap-3 rounded-lg bg-fill-subtle px-3 py-2">
                        <Webhook size={14} className="shrink-0 text-fg-muted" />
                        <div className="flex min-w-0 flex-1 flex-col">
                          <span className="truncate font-mono text-caption text-fg">
                            POST /hooks/{loop.id}/••••••••
                          </span>
                          <span className="text-caption text-fg-muted">
                            {draft.rotateWebhook
                              ? "A new URL is shown after saving, and this one stops working."
                              : "Its URL was shown once, when it was made."}
                          </span>
                        </div>
                        <Pill
                          size="sm"
                          prominent={draft.rotateWebhook}
                          aria-pressed={draft.rotateWebhook ?? false}
                          onClick={() => patch({ rotateWebhook: !draft.rotateWebhook })}
                        >
                          {draft.rotateWebhook ? "Keep URL" : "Replace URL"}
                        </Pill>
                      </div>
                    )}
                  </Section>
                  <Section title="How it runs">
                    {!remote && (
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
                    )}
                    {target === "fresh" && (
                      <div className="grid grid-cols-2 gap-x-3 gap-y-2 sm:grid-cols-4">
                        <Field label="Harness">
                          <Select
                            label="Harness"
                            value={runHarnessId ?? ""}
                            options={harnessOptions}
                            onChange={(harness) => patch({ harness, account: "", model: "", effort: "" })}
                          />
                        </Field>
                        {!remote && accountOptions.length > 0 && (
                          <Field label="Account">
                            <Select
                              label="Account"
                              value={draft.account || runAccount || ""}
                              options={accountOptions}
                              disabled={accountOptions.length === 1}
                              onChange={(account) => patch({ account })}
                            />
                          </Field>
                        )}
                        <Field label="Model">
                          {models.length ? (
                            <Select
                              label="Model"
                              value={runModel ?? ""}
                              options={modelOptions}
                              onChange={(model) => patch({ model, effort: "" })}
                            />
                          ) : (
                            <input
                              value={draft.model ?? ""}
                              onChange={(e) => patch({ model: e.target.value, effort: "" })}
                              placeholder="Model ID"
                              aria-label="Model"
                              {...noTextHelpers}
                              className={cx(inputClass, "h-[26px] min-w-0")}
                            />
                          )}
                        </Field>
                        {effortOptions.length > 0 && (
                          <Field label="Effort">
                            <Select
                              label="Effort"
                              value={runEffort ?? ""}
                              options={
                                runEffort
                                  ? effortOptions
                                  : [{ value: "", label: "Choose effort", disabled: true }, ...effortOptions]
                              }
                              onChange={(effort) => patch({ effort })}
                            />
                          </Field>
                        )}
                      </div>
                    )}
                    <ToggleRow
                      label="Read-only"
                      hint={
                        draft.readOnly
                          ? "Decline approvals for edits, commits, pushes, and other actions."
                          : "Runs with full access, including edits, commits, and pushes."
                      }
                      checked={draft.readOnly}
                      onChange={(readOnly) => patch({ readOnly })}
                    />
                  </Section>
                </div>
                <div className="flex shrink-0 items-center justify-end gap-2 border-t border-border px-5 py-3">
                  {/* In the footer, not under the form: a refused save is seen without scrolling. */}
                  {error && (
                    <p role="alert" className="mr-auto min-w-0 text-caption text-danger">
                      {error}
                    </p>
                  )}
                  <Pill onClick={() => onOpenChange(false)}>Cancel</Pill>
                  <Pill prominent disabled={!ready || saving} onClick={() => void save()}>
                    {saving ? "Saving…" : loop ? "Save changes" : "Schedule job"}
                  </Pill>
                </div>
              </>
            )}
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** The webhook URL a save just made. Only its hash is kept, so this is the one time it is shown. */
function WebhookMade({ webhook, onDone }: { webhook: LoopWebhook; onDone: () => void }) {
  return (
    <>
      <div className="flex shrink-0 items-start gap-3 px-5 pt-5 pb-3">
        <span className="inline-flex size-9 shrink-0 items-center justify-center rounded-full bg-fill text-fg">
          <Webhook size={18} />
        </span>
        <div className="flex min-w-0 flex-col gap-1">
          <Dialog.Title className="font-semibold text-title3">Webhook ready</Dialog.Title>
          <p className="text-caption text-fg-muted">
            Copy the URL now. It is shown only this once; if it is lost, replace it from the job’s editor.
          </p>
        </div>
      </div>
      <div className="flex min-h-0 flex-col gap-4 overflow-y-auto px-5 pb-4">
        <div className="flex items-center gap-2 rounded-lg bg-fill-subtle py-1 pr-1 pl-3">
          <input
            readOnly
            value={webhook.url}
            aria-label="Webhook URL"
            onFocus={(e) => e.currentTarget.select()}
            {...noTextHelpers}
            className="min-w-0 flex-1 bg-transparent font-mono text-caption text-fg outline-none"
          />
          <CopyButton text={webhook.url} label="Copy URL" />
        </div>
        {!webhook.public && (
          <Callout tone="warning">
            Only this computer can reach this URL. Set a public address for this machine to receive calls from
            GitHub or other services.
          </Callout>
        )}
        <div className="flex flex-col gap-2">
          <h3 className="font-semibold text-callout text-fg">Connect GitHub</h3>
          <ol className="flex list-decimal flex-col gap-1 pl-5 text-callout text-fg-muted marker:text-fg-faint">
            <li>In the repository, open Settings › Webhooks › Add webhook.</li>
            <li>Paste the URL as the Payload URL, and pick application/json as the content type.</li>
            <li>Choose the events that should start a run, such as Issues.</li>
          </ol>
        </div>
        <p className="text-caption text-fg-muted">
          Any service can send a POST instead. The body, up to 64 KB, is added to the prompt as data. A call
          while a run is going is refused, and calls arrive only while RustCode is open.
        </p>
      </div>
      <div className="flex shrink-0 justify-end gap-2 border-t border-border px-5 py-3">
        <Pill prominent onClick={onDone}>
          Done
        </Pill>
      </div>
    </>
  );
}
