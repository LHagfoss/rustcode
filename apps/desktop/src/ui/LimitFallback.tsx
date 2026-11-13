// Settings › Harnesses › Switch a delegate at a limit (issue 3392): with the
// switch on, a delegate that stops on a usage limit goes on at once on the other
// provider's model of the same tier, from a table kept here, instead of waiting
// for the reset. Both are server settings (`limit-fallback`,
// `limit-fallback-map`). Rules live in state/limitFallback.ts; the move is
// `switchOnLimit` in state/store.ts.

import { Plus } from "lucide-react";
import { useState } from "react";
import {
  type FallbackRule,
  parseRules,
  removeRule,
  ruleError,
  ruleSummary,
  rulesJson,
  saveRule,
} from "../state/limitFallback";
import { useScopedSetting } from "../state/settingsScope";
import { useApp } from "../state/store";
import { cx, noTextHelpers, Switch } from "./primitives";
import { ScopedValue } from "./SettingsScope";
import { Action, fieldClass, Group, Row, Select } from "./settingsKit";

const rowsLabel = (value: unknown) => {
  const count = Array.isArray(value) ? value.length : 0;
  return count === 1 ? "1 model" : `${count} models`;
};
const boolLabel = (value: unknown) => (value ? "On" : "Off");

/** `undefined` while no row is being edited, `null` for a new one, else the row's `from`. */
type Editing = FallbackRule["from"] | null | undefined;

export function LimitFallbackGroup() {
  const enabled = useScopedSetting("limit-fallback", boolLabel);
  const table = useScopedSetting("limit-fallback-map", rowsLabel);
  const rules = parseRules(table.value);
  const on = Boolean(enabled.value ?? false);
  const [editing, setEditing] = useState<Editing>(undefined);
  const save = (next: FallbackRule[]) => table.set(rulesJson(next));
  return (
    <Group
      title="Switch a delegate at a limit"
      action={
        on && editing === undefined ? (
          <Action quiet size="sm" onClick={() => setEditing(null)}>
            <Plus size={12} />
            New model
          </Action>
        ) : undefined
      }
      footer="Only a delegate that stops on a Claude or Codex usage limit moves, and only to a harness in the table; switching between your own accounts of one harness stays manual. The thread says which model it moved to, and the move is in its activity."
    >
      <Row
        title="Continue a limited delegate on another provider"
        reset={enabled.mixed || on ? () => void enabled.set(false) : undefined}
        subtitle="Instead of waiting for the reset, if the table below has a model for it."
        hint="A delegate carries on within a minute. Off, it waits for the reset as it does today."
      >
        <ScopedValue mixed={enabled.mixed} breakdown={enabled.breakdown}>
          <Switch
            label="Continue a limited delegate on another provider"
            checked={on}
            onChange={(value) => void enabled.set(value)}
          />
        </ScopedValue>
      </Row>
      {on && editing === null && (
        <RuleForm
          rules={rules}
          onCancel={() => setEditing(undefined)}
          onSave={async (rule) => {
            await save(saveRule(rules, rule));
            setEditing(undefined);
          }}
        />
      )}
      {on && rules.length === 0 && editing !== null ? (
        <Row
          title="No models"
          subtitle="Add the model to use when a harness hits its limit, for example Claude Code's Opus to Codex."
        />
      ) : null}
      {on &&
        rules.map((rule) =>
          editing && editing.harness === rule.from.harness && editing.model === rule.from.model ? (
            <RuleForm
              key={ruleSummary(rule)}
              rules={rules}
              initial={rule}
              onCancel={() => setEditing(undefined)}
              onSave={async (next) => {
                await save(saveRule(rules, next, rule.from));
                setEditing(undefined);
              }}
            />
          ) : (
            <Row key={ruleSummary(rule)} title={ruleSummary(rule)}>
              <div className="flex gap-2">
                <Action size="sm" disabled={editing !== undefined} onClick={() => setEditing(rule.from)}>
                  Edit
                </Action>
                <Action
                  size="sm"
                  danger
                  disabled={editing !== undefined}
                  onClick={() => void save(removeRule(rules, rule.from))}
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

function RuleForm({
  rules,
  initial,
  onCancel,
  onSave,
}: {
  rules: FallbackRule[];
  initial?: FallbackRule;
  onCancel: () => void;
  onSave: (rule: FallbackRule) => Promise<void>;
}) {
  const harnesses = useApp((s) => s.harnesses?.harnesses ?? []);
  const [form, setForm] = useState<FallbackRule>(initial ?? { from: { harness: "" }, to: { harness: "" } });
  const [submitted, setSubmitted] = useState(false);
  const [busy, setBusy] = useState(false);
  const error = ruleError(rules, form, initial?.from);
  const options = (current: string) => [
    { value: "", label: "Choose…" },
    ...harnesses.map((h) => ({ value: h.id, label: h.name })),
    ...(current && !harnesses.some((h) => h.id === current) ? [{ value: current, label: current }] : []),
  ];
  const text = (
    side: "from" | "to",
    key: "model" | "effort" | "account",
    label: string,
    placeholder: string,
  ) => (
    <label className="flex min-w-0 flex-1 flex-col gap-1">
      <span className="text-caption text-fg-muted">{label}</span>
      <input
        value={(form[side] as unknown as Record<string, string | undefined>)[key] ?? ""}
        onChange={(e) => setForm((f) => ({ ...f, [side]: { ...f[side], [key]: e.target.value } }))}
        placeholder={placeholder}
        aria-label={`${side === "from" ? "From" : "To"} ${label.toLowerCase()}`}
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
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <span className="text-caption text-fg-muted">When this harness hits its limit</span>
          <Select
            label="From harness"
            value={form.from.harness}
            options={options(form.from.harness)}
            onChange={(harness) => setForm((f) => ({ ...f, from: { ...f.from, harness } }))}
          />
        </div>
        {text("from", "model", "Model contains", "Any model")}
      </div>
      <div className="flex flex-wrap gap-2">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <span className="text-caption text-fg-muted">Continue on</span>
          <Select
            label="To harness"
            value={form.to.harness}
            options={options(form.to.harness)}
            onChange={(harness) => setForm((f) => ({ ...f, to: { ...f.to, harness } }))}
          />
        </div>
        {text("to", "model", "Model", "Default")}
        {text("to", "effort", "Effort", "Default")}
        {text("to", "account", "Account", "Default")}
      </div>
      {submitted && error ? <span className="text-caption text-warning">{error}</span> : null}
      <div className="flex justify-end gap-2">
        <Action onClick={onCancel}>Cancel</Action>
        <Action prominent type="submit" disabled={busy}>
          {initial ? "Save" : "Add"}
        </Action>
      </div>
    </form>
  );
}
