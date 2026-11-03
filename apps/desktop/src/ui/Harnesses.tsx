// Harness cards and the default-harness form, shared by first-run
// onboarding and Settings › Harnesses (docs/harnesses.md). RustCode never
// signs in for a harness: "Sign in" shows the harness's own login command.

import { Check, ChevronRight, Minus, SquareTerminal } from "lucide-react";
import { Popover } from "radix-ui";
import { useEffect, useMemo, useState } from "react";
import { effortLabel, type Harness, type HarnessSnapshot } from "../acp/types";
import { canOpenTerminal, openInTerminal } from "../lib/native";
import {
  capabilityRows,
  commandLine,
  harnessStatus,
  normalizeDefault,
  orderHarnesses,
  selectOption,
  unavailableReason,
  validateDefault,
} from "../state/harness";
import {
  ensureHarnessModels,
  fetchHarnessModels,
  type NewThreadChoice,
  removeHarness,
  setHarnessEnabled,
  useApp,
} from "../state/store";
import { HarnessIcon } from "./HarnessIcon";
import { CopyButton } from "./Markdown";
import { cx, Pill, Spinner, Switch, Tooltip } from "./primitives";

function StatusDot({ tone }: { tone: "ok" | "warn" | "off" }) {
  return (
    <span
      className={cx(
        "inline-block size-[7px] shrink-0 rounded-full",
        tone === "ok" ? "bg-success" : tone === "warn" ? "bg-warning" : "bg-fg-faint",
      )}
    />
  );
}

function SignIn({ harness }: { harness: Harness }) {
  const command = harness.login_command;
  if (!command?.length) return null;
  const line = commandLine(command);
  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <Pill prominent={harness.auth === "needs_login"} disabled={!harness.installed}>
          Sign in
        </Pill>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="end"
          sideOffset={6}
          className="elevated z-[60] flex w-[340px] flex-col gap-2.5 p-3 rc-in"
        >
          <p className="text-callout text-fg-muted">
            {harness.name} signs in with its own login. Run this in a terminal; RustCode never sees the
            credentials.
          </p>
          <div className="flex items-center gap-2 rounded-md bg-fill-subtle py-1 pr-1 pl-2.5">
            <code className="selectable flex-1 truncate font-mono text-callout">{line}</code>
            <CopyButton text={line} />
          </div>
          {canOpenTerminal() && (
            <Pill onClick={() => void openInTerminal(command)} className="self-start">
              <SquareTerminal size={13} />
              Open in Terminal
            </Pill>
          )}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

/** The card's check: lists the models, which is also the cheapest proof the harness runs. */
function useCheck(harness: Harness) {
  const [checking, setChecking] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const check = async () => {
    setChecking(true);
    setResult(null);
    try {
      await fetchHarnessModels(harness.id);
      const models = useApp.getState().harnesses?.harnesses.find((h) => h.id === harness.id)?.models ?? [];
      setResult({
        ok: true,
        text: models.length ? `Works · ${models.length} model${models.length === 1 ? "" : "s"}` : "Works",
      });
    } catch (error) {
      setResult({ ok: false, text: error instanceof Error ? error.message : String(error) });
    } finally {
      setChecking(false);
    }
  };
  return { checking, result, check };
}

export function HarnessCard({ harness }: { harness: Harness }) {
  const { checking, result, check } = useCheck(harness);
  const [open, setOpen] = useState(false);
  const status = harnessStatus(harness);
  const local = harness.builtin;
  // RustCode turns off only once another harness starts new threads: the engine refuses otherwise.
  const isDefault = useApp((s) => s.harnesses?.default.harness === harness.id);
  const pinned = local && isDefault && harness.enabled;
  const custom = !local && !["codex", "claude-code", "gemini", "opencode", "pi"].includes(harness.id);
  // Sign in stays out of the way once the harness is known to be signed in.
  const signIn = !local && harness.installed && harness.auth !== "ok" && !result?.ok;
  return (
    <div className="flex flex-col gap-2 px-3.5 py-3">
      <div className="flex items-center gap-3">
        <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-fill">
          <HarnessIcon id={harness.id} name={harness.name} size={17} />
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="font-semibold text-body">{harness.name}</span>
            {local && (
              <span className="rounded-full bg-fill px-1.5 text-caption text-fg-muted leading-[16px]">
                Local
              </span>
            )}
          </div>
          <div className="flex items-center gap-1.5 text-caption text-fg-muted">
            <StatusDot tone={status.tone} />
            <span className="truncate">
              {local ? `${status.label} · for the TUI, testing and local models` : status.label}
            </span>
          </div>
        </div>
        {signIn && <SignIn harness={harness} />}
        {harness.installed && harness.enabled && (
          <Pill onClick={() => void check()} disabled={checking || harness.auth === "needs_login"}>
            {checking && <Spinner size={10} />}
            Check
          </Pill>
        )}
        <Tooltip
          label={
            !harness.installed
              ? "Install it first"
              : pinned
                ? "Choose another default harness first"
                : undefined
          }
        >
          <span>
            <Switch
              label={`Use ${harness.name}`}
              checked={harness.enabled}
              disabled={!harness.installed || pinned}
              onChange={(v) =>
                // Turning a harness on checks it at once, so its models are ready to pick.
                void setHarnessEnabled(harness.id, v).then((snap) => {
                  if (v && snap) void check();
                })
              }
            />
          </span>
        </Tooltip>
      </div>
      {!harness.installed && harness.install_hint && (
        <div className="ml-11 flex items-center gap-2 rounded-md bg-fill-subtle py-1 pr-1 pl-2.5">
          <span className="text-caption text-fg-muted">Install</span>
          <code className="selectable flex-1 truncate font-mono text-callout">{harness.install_hint}</code>
          <CopyButton text={harness.install_hint} />
        </div>
      )}
      {(result || harness.error) && (
        <div
          className={cx(
            "selectable ml-11 flex items-start gap-1.5 text-callout",
            result?.ok ? "text-success" : "text-warning",
          )}
        >
          {result?.ok && <Check size={13} className="mt-[3px] shrink-0" />}
          <span>{result ? result.text : harness.error}</span>
        </div>
      )}
      <div className="ml-11">
        <button
          type="button"
          onClick={() => setOpen(!open)}
          aria-expanded={open}
          className="flex items-center gap-1 text-caption text-fg-muted hover:text-fg"
        >
          <ChevronRight size={11} className={cx("transition-transform", open && "rotate-90")} />
          Details
        </button>
        {open && (
          <div className="mt-2 flex flex-col gap-2.5">
            {!local && harness.command.length > 0 && (
              <div className="flex items-center gap-2 text-caption">
                <span className="text-fg-muted">Runs</span>
                <code className="selectable truncate font-mono text-fg-muted">
                  {commandLine(harness.command)}
                </code>
              </div>
            )}
            <ul className="grid grid-cols-2 gap-x-4 gap-y-1">
              {capabilityRows(harness.capabilities).map((row) => (
                <li
                  key={row.key}
                  className={cx("flex items-start gap-1.5 text-caption", !row.available && "text-fg-faint")}
                >
                  {row.available ? (
                    <Check size={11} className="mt-px text-success" />
                  ) : (
                    <Minus size={11} className="mt-px" />
                  )}
                  <span>
                    {row.label}
                    {row.detail && <span className="block text-fg-faint">{row.detail}</span>}
                  </span>
                </li>
              ))}
              {!local && (
                <li className="col-span-2 text-caption text-fg-faint">
                  Runs with its own tools and permission rules; RustCode's sandbox does not apply.
                </li>
              )}
            </ul>
            {custom && (
              <button
                type="button"
                onClick={() => void removeHarness(harness.id)}
                className="self-start text-caption text-danger hover:underline"
              >
                Remove
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

export function HarnessList({ snapshot }: { snapshot: HarnessSnapshot }) {
  return (
    <div className="divide-y divide-border overflow-hidden rounded-lg bg-fill-subtle">
      {orderHarnesses(snapshot.harnesses).map((h) => (
        <HarnessCard key={h.id} harness={h} />
      ))}
    </div>
  );
}

function Select({
  label,
  value,
  options,
  onChange,
  disabled,
}: {
  label: string;
  value: string;
  options: { value: string; label: string; disabled?: boolean }[];
  onChange: (v: string) => void;
  disabled?: boolean;
}) {
  return (
    <label className="flex items-center justify-between gap-4 px-3.5 py-[11px]">
      <span className="text-body">{label}</span>
      <select
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
        className="h-[26px] max-w-[260px] rounded-full bg-fill px-3 text-callout outline-none disabled:opacity-45"
      >
        {options.map((o) => (
          <option key={o.value} value={o.value} disabled={o.disabled}>
            {o.label}
          </option>
        ))}
      </select>
    </label>
  );
}

/** Default harness, model and effort for new threads. */
export function DefaultsForm({
  snapshot,
  value,
  onChange,
}: {
  snapshot: HarnessSnapshot;
  value: NewThreadChoice;
  onChange: (v: NewThreadChoice) => void;
}) {
  const threads = useApp((s) => s.threads);
  const harness = snapshot.harnesses.find((h) => h.id === value.harness);
  const [listing, setListing] = useState(false);
  // An external harness lists its models once asked; ask as soon as it is picked.
  useEffect(() => {
    if (!harness || harness.builtin || harness.models.length || !harness.installed) return;
    let live = true;
    setListing(true);
    void ensureHarnessModels(harness.id).finally(() => live && setListing(false));
    return () => {
      live = false;
    };
  }, [harness]);
  // The built-in harness lists its models through a thread's config options.
  const known = useMemo(
    () => Object.values(threads).find((t) => t.configOptions.length)?.configOptions ?? [],
    [threads],
  );
  const models = harness?.models.length
    ? harness.models.map((m) => ({ value: m.id, label: m.name, efforts: m.efforts }))
    : harness?.builtin
      ? (selectOption(known, "model")?.choices ?? []).map((c) => ({
          value: c.value,
          label: c.name,
          efforts: selectOption(known, "effort")?.choices.map((e) => e.value) ?? [],
        }))
      : [];
  const efforts = models.find((m) => m.value === value.model)?.efforts ?? models[0]?.efforts ?? [];
  const problem = validateDefault(snapshot, value);
  const set = (next: Partial<NewThreadChoice>) => {
    const merged = { ...value, ...next };
    onChange(harness?.models.length || next.harness ? normalizeDefault(snapshot, merged) : merged);
  };
  return (
    <div className="flex flex-col">
      <div className="divide-y divide-border overflow-hidden rounded-lg bg-fill-subtle">
        <Select
          label="Harness"
          value={value.harness}
          onChange={(harnessId) => set({ harness: harnessId, model: null, effort: null })}
          options={orderHarnesses(snapshot.harnesses).map((h) => {
            const reason = unavailableReason(h);
            const name = h.builtin ? `${h.name} (local)` : h.name;
            return { value: h.id, label: reason ? `${name} — ${reason}` : name, disabled: !!reason };
          })}
        />
        <Select
          label="Model"
          value={value.model ?? ""}
          disabled={!models.length}
          onChange={(model) => set({ model: model || null })}
          options={[
            {
              value: "",
              label: models.length
                ? "Harness default"
                : listing
                  ? "Loading models…"
                  : harness?.builtin
                    ? "Open a thread to list models"
                    : "Harness default",
            },
            ...models.map((m) => ({ value: m.value, label: m.label })),
          ]}
        />
        <div className="flex items-center justify-between gap-4 px-3.5 py-[11px]">
          <span className="text-body">Effort</span>
          {harness && !harness.capabilities.effort ? (
            <span className="text-callout text-fg-faint">{harness.name} has no effort setting</span>
          ) : efforts.length ? (
            <div className="flex rounded-full bg-fill-subtle p-0.5">
              {efforts.map((e) => (
                <button
                  key={e}
                  type="button"
                  aria-pressed={e === value.effort}
                  onClick={() => set({ effort: e })}
                  className={cx(
                    "h-[22px] rounded-full px-3 font-medium text-callout",
                    e === value.effort ? "bg-surface-3 text-fg" : "text-fg-muted hover:text-fg",
                  )}
                >
                  {effortLabel(e)}
                </button>
              ))}
            </div>
          ) : (
            <span className="text-callout text-fg-faint">Model default</span>
          )}
        </div>
      </div>
      {problem && <p className="mt-2 px-1 text-warning text-callout">{problem}</p>}
    </div>
  );
}
