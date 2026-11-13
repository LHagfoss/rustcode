// Settings › Harnesses › Delegate presets (issue 3391): named sets of harness,
// model, effort, account, access, worktree and instructions an agent passes to
// `start_session` or a `fan_out` entry as `preset`. They are the
// `delegate-presets` server setting; the engine expands them
// (rustcode/engine/src/orchestration/presets.rs). Rules live in
// state/delegatePresets.ts.

import { Plus } from "lucide-react";
import { useState } from "react";
import {
  type DelegatePreset,
  parsePresets,
  presetError,
  presetSummary,
  presetsJson,
  removePreset,
  savePreset,
} from "../state/delegatePresets";
import { PERMISSION_MODES } from "../state/permissions";
import { useScopedSetting } from "../state/settingsScope";
import { useApp } from "../state/store";
import { cx, noTextHelpers } from "./primitives";
import { Action, fieldClass, Group, Row, Select } from "./settingsKit";

const presetsLabel = (value: unknown) => {
  const count = Array.isArray(value) ? value.length : 0;
  return count === 1 ? "1 preset" : `${count} presets`;
};

/** `undefined` while a preset is not being edited, `null` for a new one. */
type Editing = string | null | undefined;

export function DelegatePresetsGroup() {
  const setting = useScopedSetting("delegate-presets", presetsLabel);
  const presets = parsePresets(setting.value);
  const [editing, setEditing] = useState<Editing>(undefined);
  const save = (next: DelegatePreset[]) => setting.set(presetsJson(next));
  return (
    <Group
      title="Delegate presets"
      action={
        editing === undefined ? (
          <Action quiet size="sm" onClick={() => setEditing(null)}>
            <Plus size={12} />
            New preset
          </Action>
        ) : undefined
      }
      footer="An agent passes a preset's name as preset to start_session or a fan_out entry; list_harnesses lists them. Arguments the agent passes itself override the preset, its instructions go before the prompt, and its access can only lower what the delegate would get."
    >
      {editing === null && (
        <PresetForm
          presets={presets}
          onCancel={() => setEditing(undefined)}
          onSave={async (preset) => {
            await save(savePreset(presets, preset));
            setEditing(undefined);
          }}
        />
      )}
      {presets.length === 0 && editing !== null ? (
        <Row
          title="No presets"
          subtitle="Save a harness, model and the instructions you repeat in every delegation."
        />
      ) : null}
      {presets.map((preset) =>
        editing === preset.name ? (
          <PresetForm
            key={preset.name}
            presets={presets}
            initial={preset}
            onCancel={() => setEditing(undefined)}
            onSave={async (next) => {
              await save(savePreset(presets, next, preset.name));
              setEditing(undefined);
            }}
          />
        ) : (
          <Row key={preset.name} title={preset.name} subtitle={presetSummary(preset)}>
            <div className="flex gap-2">
              <Action size="sm" disabled={editing !== undefined} onClick={() => setEditing(preset.name)}>
                Edit
              </Action>
              <Action
                size="sm"
                danger
                disabled={editing !== undefined}
                onClick={() => void save(removePreset(presets, preset.name))}
              >
                Delete
              </Action>
            </div>
          </Row>
        ),
      )}
    </Group>
  );
}

function PresetForm({
  presets,
  initial,
  onCancel,
  onSave,
}: {
  presets: DelegatePreset[];
  initial?: DelegatePreset;
  onCancel: () => void;
  onSave: (preset: DelegatePreset) => Promise<void>;
}) {
  const harnesses = useApp((s) => s.harnesses?.harnesses ?? []);
  const [form, setForm] = useState<DelegatePreset>(initial ?? { name: "" });
  const [submitted, setSubmitted] = useState(false);
  const [busy, setBusy] = useState(false);
  const error = presetError(presets, form, initial?.name);
  const patch = (next: Partial<DelegatePreset>) => setForm((f) => ({ ...f, ...next }));
  const text = (key: "name" | "model" | "effort" | "account", label: string, placeholder: string) => (
    <label className="flex min-w-0 flex-1 flex-col gap-1">
      <span className="text-caption text-fg-muted">{label}</span>
      <input
        value={form[key] ?? ""}
        onChange={(e) => patch({ [key]: e.target.value })}
        placeholder={placeholder}
        aria-label={label}
        {...noTextHelpers}
        className={cx(fieldClass, "min-w-0")}
      />
    </label>
  );
  const submit = async () => {
    setSubmitted(true);
    if (error || busy) return;
    setBusy(true);
    try {
      await onSave(form);
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      className="flex flex-col gap-3 px-4 py-3"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <div className="flex flex-wrap gap-2">
        {text("name", "Name", "Pi flash")}
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <span className="text-caption text-fg-muted">Harness</span>
          <Select
            label="Harness"
            value={form.harness ?? ""}
            options={[
              { value: "", label: "The caller's" },
              ...harnesses.map((h) => ({ value: h.id, label: h.name })),
              ...(form.harness && !harnesses.some((h) => h.id === form.harness)
                ? [{ value: form.harness, label: form.harness }]
                : []),
            ]}
            onChange={(harness) => patch({ harness: harness || undefined })}
          />
        </div>
      </div>
      <div className="flex flex-wrap gap-2">
        {text("model", "Model", "Default")}
        {text("effort", "Effort", "Default")}
        {text("account", "Account", "Default")}
      </div>
      <div className="flex flex-wrap gap-2">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <span className="text-caption text-fg-muted">Access</span>
          <Select
            label="Access"
            value={form.access ?? ""}
            options={[
              { value: "", label: "As high as the caller's" },
              ...PERMISSION_MODES.map((mode) => ({ value: mode.id, label: `At most ${mode.label}` })),
            ]}
            onChange={(access) =>
              patch({
                access: access === "ask" || access === "edits" || access === "full" ? access : undefined,
              })
            }
          />
        </div>
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <span className="text-caption text-fg-muted">Worktree</span>
          <Select
            label="Worktree"
            value={form.worktree === undefined ? "" : String(form.worktree)}
            options={[
              { value: "", label: "Default (own worktree)" },
              { value: "true", label: "Own worktree" },
              { value: "false", label: "Caller's checkout" },
            ]}
            onChange={(value) => patch({ worktree: value === "" ? undefined : value === "true" })}
          />
        </div>
      </div>
      <label className="flex flex-col gap-1">
        <span className="text-caption text-fg-muted">Instructions, put before the prompt</span>
        <textarea
          value={form.instructions ?? ""}
          onChange={(e) => patch({ instructions: e.target.value })}
          placeholder="Read AGENTS.md first. Wrap anything that can block in timeout 120."
          aria-label="Instructions"
          rows={4}
          {...noTextHelpers}
          className="rounded-lg bg-fill px-3 py-2 text-callout text-fg placeholder:text-fg-faint hover:bg-fill-strong focus:bg-fill-strong"
        />
      </label>
      {submitted && error ? <span className="text-caption text-warning">{error}</span> : null}
      <div className="flex justify-end gap-2">
        <Action onClick={onCancel}>Cancel</Action>
        <Action prominent type="submit" disabled={busy}>
          {initial ? "Save" : "Add preset"}
        </Action>
      </div>
    </form>
  );
}
