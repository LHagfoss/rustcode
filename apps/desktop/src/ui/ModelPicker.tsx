// The composer's model pill and its panel: harness (for a new thread),
// model and reasoning effort. What the thread's harness cannot do is greyed
// out with the reason, read from capability flags only.

import { ChevronLeft, ChevronRight, ChevronUp, Lock } from "lucide-react";
import { Popover } from "radix-ui";
import { useMemo, useState } from "react";
import { type ConfigOption, effortLabel, type Harness, modelName } from "../acp/types";
import { controlDisabledReason, normalizeDefault, selectOption, unavailableReason } from "../state/harness";
import { type NewThreadChoice, setConfigOption, setNewThreadChoice, useApp } from "../state/store";
import type { ThreadState } from "../state/thread";
import { cx, Tooltip } from "./primitives";

interface Choice {
  value: string;
  name: string;
  description?: string;
}

function EffortSegments({
  efforts,
  current,
  onChange,
  disabledReason,
}: {
  efforts: string[];
  current?: string | null;
  onChange: (effort: string) => void;
  disabledReason?: string;
}) {
  if (disabledReason) {
    return (
      <div className="flex h-7 items-center gap-1.5 rounded-full bg-fill-3 px-3 text-callout text-fg-3">
        <Lock size={11} />
        {disabledReason}
      </div>
    );
  }
  if (!efforts.length) return null;
  const index = Math.max(0, efforts.indexOf(current ?? ""));
  return (
    <div
      role="radiogroup"
      aria-label="Reasoning effort"
      className="relative flex h-7 rounded-full bg-fill-3 p-0.5"
    >
      <div
        className="absolute top-0.5 bottom-0.5 rounded-full bg-brand-fill transition-[left,width] duration-200 ease-snappy"
        style={{ left: 2, width: `calc(${((index + 1) / efforts.length) * 100}% - 4px)` }}
      />
      {efforts.map((effort, i) => (
        <button
          key={effort}
          type="button"
          role="radio"
          aria-checked={effort === current}
          onClick={() => onChange(effort)}
          className={cx(
            "relative z-10 flex-1 rounded-full font-medium text-callout transition-colors",
            i <= index ? "text-white" : "text-fg-2 hover:text-fg",
          )}
        >
          {effortLabel(effort)}
        </button>
      ))}
    </div>
  );
}

function ModelList({
  choices,
  current,
  onPick,
}: {
  choices: Choice[];
  current?: string | null;
  onPick: (v: string) => void;
}) {
  const [filter, setFilter] = useState("");
  const shown = useMemo(() => {
    const f = filter.toLowerCase();
    return f ? choices.filter((c) => `${c.name} ${c.description ?? ""}`.toLowerCase().includes(f)) : choices;
  }, [choices, filter]);
  return (
    <div className="flex flex-col gap-1">
      {choices.length > 8 && (
        <input
          autoFocus
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter models"
          aria-label="Filter models"
          className="mb-1 h-[34px] rounded-full bg-fill-3 px-3.5 text-body outline-none placeholder:text-fg-3"
        />
      )}
      <div className="max-h-[260px] overflow-y-auto" role="listbox" aria-label="Models">
        {shown.map((c) => (
          <button
            key={c.value}
            type="button"
            role="option"
            aria-selected={c.value === current}
            onClick={() => onPick(c.value)}
            className={cx(
              "flex w-full items-center gap-2 rounded-[14px] px-2.5 py-2 text-left hover:bg-fill-2",
              c.value === current && "bg-brand/15",
            )}
          >
            <span className="min-w-0 flex-1">
              <span className="block truncate text-body">{modelName(c.name)}</span>
              {c.description && c.description !== modelName(c.name) && (
                <span className="block truncate text-caption text-fg-2">{c.description}</span>
              )}
            </span>
            {c.value === current && <span className="text-brand">✓</span>}
          </button>
        ))}
        {!shown.length && (
          <div className="px-2.5 py-3 text-center text-callout text-fg-2">No matching models</div>
        )}
      </div>
    </div>
  );
}

function HarnessList({
  harnesses,
  current,
  onPick,
}: {
  harnesses: Harness[];
  current: string;
  onPick: (id: string) => void;
}) {
  return (
    <div className="flex flex-col gap-0.5" role="listbox" aria-label="Harness">
      {harnesses.map((h) => {
        const reason = unavailableReason(h);
        return (
          <Tooltip key={h.id} label={reason} side="left">
            <button
              type="button"
              role="option"
              aria-selected={h.id === current}
              aria-disabled={!!reason}
              onClick={() => !reason && onPick(h.id)}
              className={cx(
                "flex w-full items-center gap-2 rounded-[14px] px-2.5 py-2 text-left",
                reason ? "opacity-45" : "hover:bg-fill-2",
                h.id === current && "bg-brand/15",
              )}
            >
              <span className="flex-1 text-body">{h.name}</span>
              <span className="text-caption text-fg-2">
                {reason ? "Unavailable" : h.builtin ? "Built in" : h.kind.toUpperCase()}
              </span>
            </button>
          </Tooltip>
        );
      })}
    </div>
  );
}

/** The pill label: model (after the last `/`) and effort. */
function pillLabel(model?: string | null, effort?: string | null, compact = false) {
  const name = modelName(model ?? undefined) ?? "Default model";
  return {
    name: compact && name.length > 14 ? `${name.slice(0, 14)}…` : name,
    effort: compact ? undefined : effort,
  };
}

export function ModelPicker({ thread, compact }: { thread?: ThreadState; compact?: boolean }) {
  const harnesses = useApp((s) => s.harnesses);
  const choice = useApp((s) => s.newThread);
  const threads = useApp((s) => s.threads);
  const [open, setOpen] = useState(false);
  const [pane, setPane] = useState<"main" | "models" | "harness">("main");

  // A built-in model list known from any open thread, for a new thread before it exists.
  const knownConfig: ConfigOption[] = useMemo(
    () => Object.values(threads).find((t) => t.configOptions.length > 0)?.configOptions ?? [],
    [threads],
  );

  const isNew = !thread;
  const effective: NewThreadChoice = choice ??
    harnesses?.default ?? { harness: "rustcode", model: null, effort: null };
  const harness = harnesses?.harnesses.find(
    (h) => h.id === (isNew ? effective.harness : (thread?.harness?.id ?? "rustcode")),
  );
  const caps = isNew ? harness?.capabilities : thread?.harness?.capabilities;
  const harnessName = harness?.name ?? "This harness";

  // Model and effort come from the thread's config options, or for a new thread from the harness list.
  let models: Choice[] = [];
  let efforts: string[] = [];
  let model: string | null | undefined;
  let effort: string | null | undefined;
  if (thread) {
    const m = selectOption(thread.configOptions, "model");
    const e = selectOption(thread.configOptions, "effort");
    models = m?.choices ?? [];
    model = m?.current;
    efforts = e?.choices.map((c) => c.value) ?? [];
    effort = e?.current;
  } else if (harness?.models.length) {
    models = harness.models.map((m) => ({ value: m.id, name: m.name }));
    model = effective.model ?? harness.models[0]?.id;
    efforts = harness.models.find((m) => m.id === model)?.efforts ?? [];
    effort = effective.effort;
  } else {
    const m = selectOption(knownConfig, "model");
    const e = selectOption(knownConfig, "effort");
    models = harness?.builtin !== false ? (m?.choices ?? []) : [];
    model = effective.model ?? m?.current;
    efforts = harness?.builtin !== false ? (e?.choices.map((c) => c.value) ?? []) : [];
    effort = effective.effort ?? e?.current;
  }

  const setChoice = (next: Partial<NewThreadChoice>) => {
    const merged = { ...effective, model: model ?? null, effort: effort ?? null, ...next };
    setNewThreadChoice(harnesses ? normalizeDefault(harnesses, merged) : merged);
  };

  const pickModel = (value: string) => {
    if (thread) void setConfigOption(thread.id, "model", value);
    else setChoice({ model: value });
    setPane("main");
  };
  const pickEffort = (value: string) => {
    if (thread) void setConfigOption(thread.id, "effort", value);
    else setChoice({ effort: value });
  };

  const effortReason = controlDisabledReason("effort", caps, harnessName);
  const modelReason = controlDisabledReason("model", caps, harnessName);
  const label = pillLabel(model, effortReason ? null : effort, compact);
  const showHarness = (harnesses?.harnesses.length ?? 0) > 1 && !harnesses?.unsupported;

  return (
    <Popover.Root
      open={open}
      onOpenChange={(o) => {
        setOpen(o);
        if (!o) setPane("main");
      }}
    >
      <Popover.Trigger asChild>
        <button
          type="button"
          aria-label="Model and effort"
          className={cx(
            "flex h-[30px] max-w-[260px] items-center gap-1.5 rounded-full px-3 text-callout text-fg-2 transition-colors hover:bg-fill-1",
            open ? "bg-fill-1" : "bg-fill-2",
          )}
        >
          {showHarness && harness && !harness.builtin && (
            <span className="font-medium text-fg">{harness.name}</span>
          )}
          <span className="truncate">{label.name}</span>
          {label.effort && <span className="text-fg-3">{effortLabel(label.effort)}</span>}
          <ChevronUp
            size={11}
            strokeWidth={2.5}
            className={cx("transition-transform", open && "rotate-180")}
          />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          side="top"
          align="end"
          sideOffset={8}
          collisionPadding={10}
          className="surface z-40 flex w-[min(380px,calc(100vw-40px))] flex-col gap-2 rounded-[20px] border border-hairline p-1.5 rc-in"
        >
          {pane === "main" && (
            <>
              {showHarness && (
                <button
                  type="button"
                  disabled={!isNew}
                  onClick={() => setPane("harness")}
                  className="flex h-9 items-center gap-2 rounded-[14px] px-2.5 text-left hover:bg-fill-2 disabled:hover:bg-transparent"
                >
                  <span className="text-callout text-fg-2">Harness</span>
                  <span className="flex-1 truncate text-right font-medium text-callout">{harnessName}</span>
                  {isNew ? (
                    <ChevronRight size={13} className="text-fg-3" />
                  ) : (
                    <Tooltip label="A thread keeps the harness it started on">
                      <Lock size={12} className="text-fg-3" />
                    </Tooltip>
                  )}
                </button>
              )}
              <Tooltip label={modelReason}>
                <button
                  type="button"
                  disabled={!!modelReason || !models.length}
                  onClick={() => setPane("models")}
                  className="flex h-9 items-center gap-2 rounded-[14px] px-2.5 text-left hover:bg-fill-2 disabled:opacity-50 disabled:hover:bg-transparent"
                >
                  <span className="text-callout text-fg-2">Model</span>
                  <span className="flex-1 truncate text-right font-medium text-callout">
                    {modelName(model ?? undefined) ?? (models.length ? "Default" : "Harness default")}
                  </span>
                  <ChevronRight size={13} className="text-fg-3" />
                </button>
              </Tooltip>
              <div className="px-1 pb-1">
                <EffortSegments
                  efforts={efforts}
                  current={effort}
                  onChange={pickEffort}
                  disabledReason={effortReason}
                />
              </div>
            </>
          )}
          {pane !== "main" && (
            <button
              type="button"
              onClick={() => setPane("main")}
              className="flex h-7 items-center gap-1 self-start rounded-full px-2 font-medium text-callout text-fg-2 hover:bg-fill-2"
            >
              <ChevronLeft size={13} />
              {pane === "models" ? "Models" : "Harness"}
            </button>
          )}
          {pane === "models" && <ModelList choices={models} current={model} onPick={pickModel} />}
          {pane === "harness" && harnesses && (
            <HarnessList
              harnesses={harnesses.harnesses}
              current={effective.harness}
              onPick={(id) => {
                setNewThreadChoice(normalizeDefault(harnesses, { harness: id, model: null, effort: null }));
                setPane("main");
              }}
            />
          )}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
