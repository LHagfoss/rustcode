// The composer's model pill and its panel, after the SwiftUI app's
// ModelEffortPanel: Fast mode and the model (with its effort) on one line,
// the effort slider under them, and the harness only when more than one is
// usable. The model's name drills into the model list. What the thread's
// harness cannot do is greyed out with the reason, from capability flags.

import { ChevronLeft, ChevronRight, ChevronUp, Lock, Zap } from "lucide-react";
import { Popover } from "radix-ui";
import { useEffect, useMemo, useRef, useState } from "react";
import { type ConfigOption, effortLabel, type Harness, modelName } from "../acp/types";
import {
  controlDisabledReason,
  normalizeDefault,
  selectOption,
  unavailableReason,
  usableHarnesses,
} from "../state/harness";
import { type NewThreadChoice, setConfigOption, setNewThreadChoice, useApp } from "../state/store";
import type { ThreadState } from "../state/thread";
import { cx, Tooltip } from "./primitives";

interface Choice {
  value: string;
  name: string;
  description?: string;
}

/**
 * Reasoning effort as one track of evenly spaced stops, after the SwiftUI
 * app's EffortSlider: the knob follows the pointer while dragged and snaps to
 * the nearest stop when let go; a click on the track glides there; ←→ step.
 * Only the stop the knob settles on is sent.
 */
function EffortSlider({
  efforts,
  current,
  onChange,
  onPreview,
}: {
  efforts: string[];
  current?: string | null;
  onChange: (effort: string) => void;
  onPreview: (effort: string | null) => void;
}) {
  const track = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<number | null>(null);
  const [pending, setPending] = useState<number | null>(null);
  const count = efforts.length;
  const known = Math.max(0, efforts.indexOf(current ?? ""));
  const index = pending ?? known;
  // The engine confirmed (or changed) the effort: drop the optimistic stop.
  // biome-ignore lint/correctness/useExhaustiveDependencies: reset when the confirmed effort changes
  useEffect(() => setPending(null), [current]);
  const fractionOf = (i: number) => (count > 1 ? i / (count - 1) : 0);
  const nearest = (f: number) => Math.round(f * (count - 1));
  const fraction = drag ?? fractionOf(index);
  const shown = drag === null ? index : nearest(drag);
  const KNOB = 24;
  const INSET = 2;

  const at = (clientX: number) => {
    const el = track.current;
    if (!el) return 0;
    const rect = el.getBoundingClientRect();
    const travel = rect.width - KNOB - 2 * INSET;
    return Math.min(1, Math.max(0, (clientX - rect.left - INSET - KNOB / 2) / Math.max(travel, 1)));
  };
  const commit = (i: number) => {
    const clamped = Math.min(count - 1, Math.max(0, i));
    setPending(clamped);
    const effort = efforts[clamped];
    if (effort && effort !== current) onChange(effort);
  };

  return (
    <div
      ref={track}
      role="slider"
      tabIndex={0}
      aria-label="Reasoning effort"
      aria-valuemin={0}
      aria-valuemax={count - 1}
      aria-valuenow={index}
      aria-valuetext={effortLabel(efforts[index] ?? "")}
      onPointerDown={(e) => {
        e.currentTarget.setPointerCapture(e.pointerId);
        const f = at(e.clientX);
        setDrag(f);
        onPreview(efforts[nearest(f)] ?? null);
      }}
      onPointerMove={(e) => {
        if (drag === null) return;
        const f = at(e.clientX);
        setDrag(f);
        onPreview(efforts[nearest(f)] ?? null);
      }}
      onPointerUp={() => {
        if (drag === null) return;
        commit(nearest(drag));
        setDrag(null);
        onPreview(null);
      }}
      onPointerCancel={() => {
        setDrag(null);
        onPreview(null);
      }}
      onKeyDown={(e) => {
        if (e.key === "ArrowLeft" || e.key === "ArrowDown") {
          e.preventDefault();
          commit(index - 1);
        } else if (e.key === "ArrowRight" || e.key === "ArrowUp") {
          e.preventDefault();
          commit(index + 1);
        } else if (e.key === "Home") {
          e.preventDefault();
          commit(0);
        } else if (e.key === "End") {
          e.preventDefault();
          commit(count - 1);
        }
      }}
      className="relative h-7 touch-none rounded-full bg-fill-3 outline-none focus-visible:ring-2 focus-visible:ring-info/50"
    >
      <div
        className={cx(
          "absolute top-0 bottom-0 left-0 rounded-full bg-brand-fill",
          drag === null && "transition-[width] duration-300 ease-snappy",
        )}
        style={{ width: `calc(${INSET * 2 + KNOB}px + (100% - ${KNOB + INSET * 2}px) * ${fraction})` }}
      />
      {efforts.map((effort, i) => (
        <span
          key={effort}
          className={cx(
            "absolute top-1/2 size-1 -translate-x-1/2 -translate-y-1/2 rounded-full transition-colors duration-150",
            i <= shown ? "bg-white/55" : "bg-fg-3",
          )}
          style={{ left: `calc(${INSET + KNOB / 2}px + (100% - ${KNOB + INSET * 2}px) * ${fractionOf(i)})` }}
        />
      ))}
      <span
        className={cx(
          "absolute top-[2px] size-6 rounded-full bg-white shadow-[0_1px_3px_rgb(0_0_0/0.3)]",
          drag === null ? "transition-[left,transform] duration-300 ease-snappy" : "scale-105",
        )}
        style={{ left: `calc(${INSET}px + (100% - ${KNOB + INSET * 2}px) * ${fraction})` }}
      />
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

/** Fast mode as a labelled toggle, offered only when the model has it. */
function FastToggle({ on, onChange }: { on: boolean; onChange: (on: boolean) => void }) {
  return (
    <Tooltip label="Faster replies; uses more of your plan">
      <button
        type="button"
        role="switch"
        aria-checked={on}
        onClick={() => onChange(!on)}
        className={cx(
          "flex h-7 items-center gap-1.5 rounded-full px-2.5 font-medium text-callout",
          on ? "bg-brand/15 text-brand" : "text-fg-2 hover:bg-fill-2 hover:text-fg",
        )}
      >
        <Zap size={13} fill={on ? "currentColor" : "none"} />
        Fast mode
      </button>
    </Tooltip>
  );
}

export function ModelPicker({ thread, compact }: { thread?: ThreadState; compact?: boolean }) {
  const harnesses = useApp((s) => s.harnesses);
  const choice = useApp((s) => s.newThread);
  const threads = useApp((s) => s.threads);
  const [open, setOpen] = useState(false);
  const [pane, setPane] = useState<"main" | "models" | "harness">("main");
  const [preview, setPreview] = useState<string | null>(null);

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
  const fast = thread?.configOptions.find((o) => o.id === "fast" && o.type === "boolean");

  const setChoice = (next: Partial<NewThreadChoice>) => {
    const merged = { ...effective, model: model ?? null, effort: effort ?? null, ...next };
    setNewThreadChoice(harnesses ? normalizeDefault(harnesses, merged) : merged);
  };

  const pickModel = (value: string) => {
    if (thread) void setConfigOption(thread.id, "model", value);
    else setChoice({ model: value });
    if (listOnly) setOpen(false);
    else setPane("main");
  };
  const pickEffort = (value: string) => {
    if (thread) void setConfigOption(thread.id, "effort", value);
    else setChoice({ effort: value });
  };

  const effortReason = controlDisabledReason("effort", caps, harnessName);
  const modelReason = controlDisabledReason("model", caps, harnessName);
  const label = pillLabel(model, effortReason ? null : effort, compact);
  const usable = harnesses ? usableHarnesses(harnesses).length : 0;
  const showHarness = usable > 1 && !harnesses?.unsupported;
  const hasEffort = !effortReason && efforts.length > 1;
  // With nothing to set but the model, the list is the whole panel.
  const listOnly = !hasEffort && !fast && !showHarness && !modelReason && models.length > 0;
  const shownPane = listOnly ? "models" : pane;
  const shownEffort = preview ?? (hasEffort ? effort : null);

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
            "flex h-[30px] max-w-[260px] items-center gap-1.5 rounded-full px-3 text-callout text-fg-2 transition-colors hover:bg-fill-1 hover:text-fg",
            open ? "bg-fill-1 text-fg" : "bg-fill-2",
          )}
        >
          {fast?.currentBool && <Zap size={11} className="shrink-0 text-brand" fill="currentColor" />}
          <span className="truncate">{label.name}</span>
          {label.effort && <span className="text-fg-3">{effortLabel(label.effort)}</span>}
          <ChevronUp
            size={11}
            strokeWidth={2.5}
            className={cx("transition-transform duration-200 ease-snappy", open && "rotate-180")}
          />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          side="top"
          align="end"
          sideOffset={8}
          collisionPadding={10}
          className="surface z-40 w-[min(340px,calc(100vw-40px))] overflow-hidden rounded-[20px] border border-hairline p-2"
        >
          {shownPane === "main" && (
            <div key="main" className="flex flex-col gap-2 rc-drill-back">
              <div className="flex items-center gap-2">
                {fast && (
                  <FastToggle
                    on={!!fast.currentBool}
                    onChange={(on) => thread && void setConfigOption(thread.id, "fast", on)}
                  />
                )}
                <span className="flex-1" />
                <Tooltip label={modelReason}>
                  <button
                    type="button"
                    disabled={!!modelReason || !models.length}
                    onClick={() => setPane("models")}
                    aria-label={`Model ${modelName(model ?? undefined) ?? "default"}`}
                    className="flex h-7 min-w-0 items-center gap-1.5 rounded-full px-2.5 font-medium text-callout hover:bg-fill-2 disabled:hover:bg-transparent"
                  >
                    <span className="truncate">
                      {modelName(model ?? undefined) ?? (models.length ? "Default" : "Harness default")}
                    </span>
                    {shownEffort && <span className="shrink-0 text-fg-3">{effortLabel(shownEffort)}</span>}
                    {!modelReason && models.length > 0 && (
                      <ChevronRight size={12} strokeWidth={2.5} className="shrink-0 text-fg-3" />
                    )}
                  </button>
                </Tooltip>
              </div>
              {hasEffort && (
                <EffortSlider
                  efforts={efforts}
                  current={effort}
                  onChange={pickEffort}
                  onPreview={setPreview}
                />
              )}
              {effortReason && efforts.length > 0 && (
                <div className="flex items-center gap-1.5 px-2.5 text-caption text-fg-3">
                  <Lock size={10} />
                  {effortReason}
                </div>
              )}
              {showHarness && (
                <button
                  type="button"
                  disabled={!isNew}
                  onClick={() => setPane("harness")}
                  className="flex h-8 items-center gap-2 rounded-[12px] px-2.5 text-left hover:bg-fill-2 disabled:hover:bg-transparent"
                >
                  <span className="text-callout text-fg-2">Harness</span>
                  <span className="flex-1 truncate text-right font-medium text-callout">{harnessName}</span>
                  {isNew ? (
                    <ChevronRight size={12} strokeWidth={2.5} className="text-fg-3" />
                  ) : (
                    <Tooltip label="A thread keeps its harness">
                      <Lock size={11} className="text-fg-3" />
                    </Tooltip>
                  )}
                </button>
              )}
            </div>
          )}
          {shownPane !== "main" && (
            <div key={shownPane} className="flex flex-col gap-1.5 rc-drill">
              {!listOnly && (
                <button
                  type="button"
                  onClick={() => setPane("main")}
                  className="flex h-7 items-center gap-1 self-start rounded-full px-2 font-semibold text-callout text-fg-2 hover:bg-fill-2"
                >
                  <ChevronLeft size={13} />
                  {shownPane === "models" ? "Models" : "Harness"}
                </button>
              )}
              {shownPane === "models" && <ModelList choices={models} current={model} onPick={pickModel} />}
              {shownPane === "harness" && harnesses && (
                <HarnessList
                  harnesses={harnesses.harnesses}
                  current={effective.harness}
                  onPick={(id) => {
                    setNewThreadChoice(
                      normalizeDefault(harnesses, { harness: id, model: null, effort: null }),
                    );
                    setPane("main");
                  }}
                />
              )}
            </div>
          )}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
