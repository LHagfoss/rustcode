// The composer's model pill and its panel, after the SwiftUI app's
// ModelEffortPanel: Fast mode and the model (with its effort) on one line,
// the effort slider under them, and the harness only when more than one is
// usable. The model's name drills into the model list. What the thread's
// harness cannot do is greyed out with the reason, from capability flags.

import { ChevronLeft, ChevronRight, ChevronUp, Lock, Search, Zap } from "lucide-react";
import { Popover } from "radix-ui";
import { useEffect, useMemo, useRef, useState } from "react";
import { type ConfigOption, effortLabel, type Harness, modelName } from "../acp/types";
import { clip } from "../lib/clip";
import { matchModels } from "../lib/modelSearch";
import {
  controlDisabledReason,
  defaultEffort,
  normalizeDefault,
  selectOption,
  unavailableReason,
  usableHarnesses,
} from "../state/harness";
import { defaultLoginGone, newThreadAccountOn } from "../state/logins";
import {
  ensureHarnessModels,
  lacksModels,
  type NewThreadChoice,
  newThreadChoice,
  pickNewThreadAccount,
  pickNewThreadChoice,
  setConfigOption,
  switchNewThreadHarness,
  switchThreadAccount,
  switchThreadHarness,
  useApp,
} from "../state/store";
import type { ThreadState } from "../state/thread";
import { composerPopoverCollisionPadding, useComposerPopoverOffset } from "./composerPopover";
import { HarnessIcon } from "./HarnessIcon";
import { cx, Skeleton, Tooltip } from "./primitives";

interface Choice {
  value: string;
  name: string;
  description?: string;
}

/**
 * The effort steps from lowest to highest, so `off` sits at the left of the slider whatever
 * order the harness listed them in. Unknown values follow the known ones, in the harness's order.
 */
const EFFORT_ORDER = ["off", "none", "default", "minimal", "low", "medium", "high", "xhigh", "max"];

export function orderEfforts(efforts: string[]): string[] {
  const rank = (value: string) => {
    const at = EFFORT_ORDER.indexOf(value.toLowerCase());
    return at < 0 ? EFFORT_ORDER.length : at;
  };
  return efforts
    .map((value, index) => ({ value, index }))
    .sort((a, b) => rank(a.value) - rank(b.value) || a.index - b.index)
    .map((entry) => entry.value);
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
      className="relative h-7 touch-none rounded-full bg-fill-subtle"
    >
      <div
        className={cx(
          "absolute top-0 bottom-0 left-0 rounded-full bg-fill-strong",
          drag === null && "transition-[width] duration-300 ease-snappy",
        )}
        style={{ width: `calc(${INSET * 2 + KNOB}px + (100% - ${KNOB + INSET * 2}px) * ${fraction})` }}
      />
      {efforts.map((effort, i) => (
        <span
          key={effort}
          className={cx(
            "absolute top-1/2 size-1 -translate-x-1/2 -translate-y-1/2 rounded-full",
            i <= shown ? "bg-fg/45" : "bg-fg-faint",
          )}
          style={{ left: `calc(${INSET + KNOB / 2}px + (100% - ${KNOB + INSET * 2}px) * ${fractionOf(i)})` }}
        />
      ))}
      <span
        className={cx(
          "absolute top-[2px] size-6 rounded-full bg-fg shadow-1",
          drag === null ? "transition-[left,transform] duration-300 ease-snappy" : "scale-105",
        )}
        style={{ left: `calc(${INSET}px + (100% - ${KNOB + INSET * 2}px) * ${fraction})` }}
      />
    </div>
  );
}

/**
 * The model browser: providers (harnesses) in a column on the left, their
 * models at the right under a search that opens focused. A model's name and
 * its id both match; hundreds of models show a window, and the rest wait
 * behind a `keep typing` line.
 */
function ModelBrowser({
  providers,
  provider,
  choices,
  current,
  canSwitch,
  onProvider,
  onPick,
}: {
  providers: Harness[];
  /** The harness the models below belong to. */
  provider: string;
  choices: Choice[];
  current?: string | null;
  /** A running turn keeps its harness: then the column only shows where they came from. */
  canSwitch: boolean;
  onProvider: (id: string) => void;
  onPick: (v: string) => void;
}) {
  const [filter, setFilter] = useState("");
  const [active, setActive] = useState(0);
  const field = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const { shown, total } = useMemo(() => matchModels(filter, choices), [filter, choices]);
  const columns = providers.length > 1;
  // biome-ignore lint/correctness/useExhaustiveDependencies: a new query or provider starts at the top
  useEffect(() => setActive(0), [filter, provider]);
  useEffect(() => {
    field.current?.focus();
  }, []);
  useEffect(() => {
    list.current?.querySelector('[data-active="true"]')?.scrollIntoView({ block: "nearest" });
  }, []);
  const move = (by: number) => setActive((i) => Math.min(Math.max(i + by, 0), Math.max(shown.length - 1, 0)));
  return (
    <div
      className="flex flex-col gap-1.5"
      onKeyDown={(e) => {
        if (e.key === "ArrowDown") {
          e.preventDefault();
          move(1);
        } else if (e.key === "ArrowUp") {
          e.preventDefault();
          move(-1);
        } else if (e.key === "Enter") {
          const pick = shown[active];
          if (pick) {
            e.preventDefault();
            onPick(pick.value);
          }
        }
      }}
    >
      <div className="flex h-8 items-center gap-1.5 rounded-full bg-fill-subtle px-3">
        <Search size={13} className="shrink-0 text-fg-faint" />
        <input
          ref={field}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Search models"
          aria-label="Search models"
          aria-controls="model-list"
          aria-activedescendant={shown[active] ? `model-${shown[active]!.value}` : undefined}
          className="h-full min-w-0 flex-1 bg-transparent text-body outline-none placeholder:text-fg-faint"
        />
      </div>
      <div className="flex min-h-0 gap-1.5">
        {columns && (
          <div
            className="flex w-[124px] shrink-0 flex-col gap-0.5 overflow-y-auto border-border/60 border-r pr-1.5"
            role="listbox"
            aria-label="Providers"
          >
            {providers.map((h) => {
              const reason = unavailableReason(h);
              const blocked =
                reason ?? (!canSwitch && h.id !== provider ? "Stop the turn to switch harness" : undefined);
              return (
                <Tooltip key={h.id} label={reason ?? blocked ?? `${h.name} models`} side="left">
                  <button
                    type="button"
                    role="option"
                    aria-selected={h.id === provider}
                    aria-disabled={!!blocked}
                    onClick={() => !blocked && onProvider(h.id)}
                    className={cx(
                      "flex h-7 shrink-0 items-center gap-1.5 rounded-sm px-2 text-left text-callout",
                      h.id === provider
                        ? "bg-fill font-medium text-fg"
                        : blocked
                          ? "text-fg-muted opacity-45"
                          : "text-fg-muted hover:bg-fill-subtle hover:text-fg",
                    )}
                  >
                    <HarnessIcon id={h.id} name={h.name} size={13} className="shrink-0" />
                    <span className="min-w-0 truncate">{h.name}</span>
                  </button>
                </Tooltip>
              );
            })}
          </div>
        )}
        <div
          ref={list}
          id="model-list"
          className="max-h-[268px] min-w-0 flex-1 overflow-y-auto"
          role="listbox"
          aria-label="Models"
        >
          {shown.map((c, i) => (
            <button
              key={c.value}
              id={`model-${c.value}`}
              type="button"
              role="option"
              aria-selected={c.value === current}
              data-active={i === active}
              onMouseEnter={() => setActive(i)}
              onClick={() => onPick(c.value)}
              className={cx(
                "flex w-full items-center gap-2 rounded-lg px-2.5 py-[7px] text-left",
                i === active ? "bg-fill" : "hover:bg-fill-subtle",
              )}
            >
              <span className="min-w-0 flex-1">
                <span className="block truncate text-body">{modelName(c.name)}</span>
                {c.description && c.description !== modelName(c.name) && (
                  <span className="block truncate text-caption text-fg-muted">{c.description}</span>
                )}
              </span>
              {c.value === current && <span className="shrink-0 text-accent">✓</span>}
            </button>
          ))}
          {!shown.length && (
            <div className="px-2.5 py-3 text-center text-callout text-fg-muted">No matching models</div>
          )}
          {total > shown.length && (
            <p className="px-2.5 pt-1 pb-0.5 text-caption text-fg-faint">
              Keep typing · {total - shown.length} more
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

/** The pill label: model (after the last `/`) and effort. */
function pillLabel(model?: string | null, effort?: string | null, compact = false) {
  const name = modelName(model ?? undefined) ?? "Default model";
  return {
    name: compact && name.length > 14 ? `${clip(name, 13)}…` : name,
    effort: compact ? undefined : effort,
  };
}

/** Fast mode as a labelled toggle, offered only when the model has it. */
function FastToggle({ on, onChange }: { on: boolean; onChange: (on: boolean) => void }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-description="Faster replies; uses more of your plan"
      onClick={() => onChange(!on)}
      className={cx(
        "flex h-7 items-center gap-1.5 rounded-full px-2.5 font-medium text-callout",
        on ? "bg-fill-strong text-fg" : "text-fg-muted hover:bg-fill hover:text-fg",
      )}
    >
      <Zap size={13} fill={on ? "currentColor" : "none"} />
      Fast mode
    </button>
  );
}

const NO_CONFIG: ConfigOption[] = [];

/** A built-in model list known from any open thread, for a new thread before it exists. */
function firstConfig(threads: Record<string, ThreadState>): ConfigOption[] {
  for (const id in threads) {
    const options = threads[id]!.configOptions;
    if (options.length) return options;
  }
  return NO_CONFIG;
}

/**
 * What the picker offers: a thread's own config options (a restored external thread whose harness
 * has not started again lists only its own model: the harness's listed models fill in), or for a
 * new thread the harness's listed models (an external harness lists them once asked, see
 * `ensureHarnessModels`), else the built-in list an open thread knows. With no effort chosen the
 * slider sits on the model's default effort.
 */
export function pickerModels(
  thread: ThreadState | undefined,
  harness: Harness | undefined,
  effective: NewThreadChoice,
  knownConfig: ConfigOption[],
): {
  models: Choice[];
  efforts: string[];
  model: string | null | undefined;
  effort: string | null | undefined;
} {
  const listedModels = (h: Harness) => h.models.map((m) => ({ value: m.id, name: m.name }));
  if (thread) {
    const m = selectOption(thread.configOptions, "model");
    const e = selectOption(thread.configOptions, "effort");
    let models = m?.choices ?? [];
    let efforts = e?.choices.map((c) => c.value) ?? [];
    let effort: string | null | undefined = e?.current;
    const external = !!harness && !harness.builtin;
    if (external && models.length <= 1 && harness.models.length > models.length) {
      const own = models.filter((c) => !harness.models.some((x) => x.id === c.value));
      models = [...own, ...listedModels(harness)];
    }
    if (external && !efforts.length && harness.capabilities.effort) {
      const listed = harness.models.find((x) => x.id === m?.current);
      efforts = listed?.efforts ?? [];
      effort = defaultEffort(listed, efforts);
    }
    return { models, model: m?.current, efforts: orderEfforts(efforts), effort };
  }
  if (harness?.models.length) {
    // A stale default (a model the harness no longer lists) falls back to the model an open
    // built-in thread is on, else the harness's first.
    const listed = harness.models.some((m) => m.id === effective.model);
    const current = harness.builtin ? selectOption(knownConfig, "model")?.current : undefined;
    const model = listed
      ? effective.model
      : (harness.models.find((m) => m.id === current)?.id ?? harness.models[0]?.id);
    const entry = harness.models.find((m) => m.id === model);
    const efforts = entry?.efforts ?? [];
    const effort =
      effective.effort && efforts.includes(effective.effort)
        ? effective.effort
        : defaultEffort(entry, efforts);
    return { models: listedModels(harness), model, efforts: orderEfforts(efforts), effort };
  }
  const m = selectOption(knownConfig, "model");
  const e = selectOption(knownConfig, "effort");
  const builtin = harness?.builtin !== false;
  return {
    models: builtin ? (m?.choices ?? []) : [],
    model: effective.model ?? m?.current,
    efforts: builtin ? orderEfforts(e?.choices.map((c) => c.value) ?? []) : [],
    effort: effective.effort ?? e?.current,
  };
}

export function ModelPicker({ thread, compact }: { thread?: ThreadState; compact?: boolean }) {
  const harnesses = useApp((s) => s.harnesses);
  const choice = useApp((s) => s.newThread);
  // The first open thread's config options: stable while a reply streams, so the picker does not re-render per chunk.
  const knownConfig = useApp((s) => firstConfig(s.threads));
  const [open, setOpen] = useState(false);
  // `/model` asks the picker to open (store.runCommandAction).
  const ask = useApp((s) => s.modelPickerAsk);
  const seenAsk = useRef(ask);
  useEffect(() => {
    if (ask === seenAsk.current) return;
    seenAsk.current = ask;
    setOpen(true);
  }, [ask]);
  const [pane, setPane] = useState<"main" | "models" | "accounts">("main");
  const [preview, setPreview] = useState<string | null>(null);
  const { triggerRef, sideOffset } = useComposerPopoverOffset();

  const isNew = !thread;
  const effective: NewThreadChoice = newThreadChoice({ newThread: choice, harnesses }) ?? {
    harness: "rustcode",
    model: null,
    effort: null,
  };
  const harness = harnesses?.harnesses.find(
    (h) => h.id === (isNew ? effective.harness : (thread?.harness?.id ?? "rustcode")),
  );
  const caps = isNew ? harness?.capabilities : thread?.harness?.capabilities;
  const accounts = harness?.accounts ?? [];
  // A new thread on a signed-out default login starts on a named account instead (state/logins.ts);
  // an open thread keeps its own, so it says what it really runs on.
  const defaultGone = useApp((s) => defaultLoginGone(harness, s.signedOutLogins));
  const newAccount = useApp((s) => newThreadAccountOn(harness, s.newThreadAccount, s.signedOutLogins));
  const account = isNew ? newAccount : (thread?.harness?.account ?? null);
  const accountName = accounts.find((a) => a.id === account)?.name;
  // A thread keeps its account id when that login is removed from Settings:
  // it stays readable, and the picker says which one is gone.
  const accountRemoved = !isNew && !!account && !accountName;
  const accountLabel = accountRemoved ? "Removed account" : (accountName ?? "Default login");
  // "Default login" is not offered once it is known to be signed out, unless an
  // open thread is on it already (hiding it would misstate where that thread runs)
  // or on an account that was removed, where it is the way back.
  const defaultOffered = !defaultGone || accountRemoved || (!isNew && !account);
  // A running thread keeps its login, as it keeps its harness.
  const accountBlocked = (accounts.length > 0 || accountRemoved) && !isNew && !!thread?.running;
  // An external harness lists its models only when asked: ask for a new thread on it, or a restored
  // thread that knows only its own model. A failed ask is tried again when the picker opens.
  const listsLater =
    !!harness && !harness.builtin && !harness.models.length && (isNew || (!!thread && lacksModels(thread)));
  const engineReady = useApp((s) => s.status === "ready");
  // biome-ignore lint/correctness/useExhaustiveDependencies: opening the picker asks again
  useEffect(() => {
    if (listsLater && engineReady && harness) void ensureHarnessModels(harness.id);
  }, [listsLater, engineReady, harness, open]);
  const harnessName = harness?.name ?? "This harness";

  const { models, efforts, model, effort } = pickerModels(thread, harness, effective, knownConfig);
  const fast = thread?.configOptions.find((o) => o.id === "fast" && o.type === "boolean");

  const setChoice = (next: Partial<NewThreadChoice>) => {
    const merged = { ...effective, model: model ?? null, effort: effort ?? null, ...next };
    pickNewThreadChoice(harnesses ? normalizeDefault(harnesses, merged) : merged);
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
  /**
   * The login a thread runs on. A new thread keeps the choice for its next
   * one; an idle thread moves to the other account with the conversation so
   * far as a summary, exactly as switching harness does.
   */
  const pickAccount = (id: string | null) => {
    if (isNew) {
      pickNewThreadAccount(id);
      setPane("main");
      return;
    }
    if (!thread || thread.running) return;
    void switchThreadAccount(thread.id, id).then((switched) => {
      if (switched) setOpen(false);
    });
  };

  // The providers worth listing: RustCode turned off in Settings › Agents leaves the list, and a
  // thread moves only to an external harness (RustCode cannot take one over yet).
  const providers = (harnesses?.harnesses ?? []).filter((h) =>
    isNew ? !h.builtin || h.enabled || h.id === effective.harness : !h.builtin || h.id === harness?.id,
  );
  const providerId = isNew ? effective.harness : (harness?.id ?? "rustcode");
  const canSwitch = isNew || !thread?.running;
  const pickProvider = (id: string) => {
    if (id === providerId) return;
    if (isNew) switchNewThreadHarness(id);
    else if (thread) void switchThreadHarness(thread.id, id);
    // A harness lists its models only when asked; the browser says so until it does.
    const next = providers.find((h) => h.id === id);
    if (next && !next.models.length) void ensureHarnessModels(id);
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

  // Nothing to show until the engine lists harnesses (or the thread its options): a placeholder pill.
  if (!harnesses && !thread?.configOptions.length) {
    return (
      <span role="status" aria-label="Loading models" className="flex h-[30px] items-center">
        <Skeleton className="h-[30px] w-[132px] rounded-full" />
      </span>
    );
  }

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
          ref={triggerRef}
          type="button"
          aria-label="Model and effort"
          className={cx(
            "flex h-[30px] min-w-0 max-w-[260px] items-center gap-1.5 rounded-full px-2 text-callout text-fg hover:bg-fill @max-lg/composer:max-w-[180px]",
            open && "bg-fill-strong text-fg",
          )}
        >
          <HarnessIcon id={harness?.id ?? "rustcode"} name={harness?.name} size={14} />
          {fast?.currentBool && <Zap size={11} className="shrink-0 text-fg" fill="currentColor" />}
          <span className="min-w-0 truncate">{label.name}</span>
          {label.effort && <span className="shrink-0 text-fg-faint">{effortLabel(label.effort)}</span>}
          {accountName && (
            // The account is a hint, not a second model label: capped and ellipsized so a long
            // login never takes the width the model's own name needs.
            <span className="min-w-0 max-w-[72px] truncate text-caption text-fg-faint @max-lg/composer:hidden">
              {accountName}
            </span>
          )}
          <ChevronUp
            size={11}
            strokeWidth={2.5}
            className={cx("shrink-0 transition-transform duration-200 ease-snappy", open && "rotate-180")}
          />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          side="top"
          align="end"
          sideOffset={sideOffset}
          collisionPadding={composerPopoverCollisionPadding}
          className="elevated z-40 w-[min(420px,calc(100vw-40px))] overflow-hidden p-1.5"
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
                    className="flex h-7 min-w-0 items-center gap-1.5 rounded-full px-2.5 font-medium text-callout hover:bg-fill disabled:hover:bg-transparent"
                  >
                    <span className="truncate">
                      {modelName(model ?? undefined) ?? (models.length ? "Default" : "Harness default")}
                    </span>
                    {shownEffort && (
                      <span className="shrink-0 text-fg-faint">{effortLabel(shownEffort)}</span>
                    )}
                    {!modelReason && models.length > 0 && (
                      <ChevronRight size={12} strokeWidth={2.5} className="shrink-0 text-fg-faint" />
                    )}
                  </button>
                </Tooltip>
              </div>
              {fast && (
                <p className="-mt-1 px-2.5 text-caption text-fg-faint">
                  {fast.currentBool
                    ? "Fast mode: faster replies, more of your plan"
                    : "Fast mode answers faster and uses more of your plan"}
                </p>
              )}
              {hasEffort && (
                <EffortSlider
                  efforts={efforts}
                  current={effort}
                  onChange={pickEffort}
                  onPreview={setPreview}
                />
              )}
              {effortReason && efforts.length > 0 && (
                <div className="flex items-center gap-1.5 px-2.5 text-caption text-fg-faint">
                  <Lock size={10} />
                  {effortReason}
                </div>
              )}
              {(accounts.length > 0 || accountRemoved) && (
                <Tooltip label={accountBlocked ? "Stop the turn to switch account" : undefined}>
                  <button
                    type="button"
                    onClick={() => !accountBlocked && setPane("accounts")}
                    aria-disabled={accountBlocked}
                    className={cx(
                      "flex h-8 items-center gap-2 rounded-lg px-2.5 text-left",
                      accountBlocked ? "text-fg-muted opacity-45" : "hover:bg-fill",
                    )}
                  >
                    <span className="text-callout text-fg-muted">Account</span>
                    <span className="flex min-w-0 flex-1 items-center justify-end gap-1.5 font-medium text-callout">
                      <span className={cx("min-w-0 truncate", accountRemoved && "text-warning")}>
                        {accountLabel}
                      </span>
                    </span>
                    <ChevronRight size={12} strokeWidth={2.5} className="shrink-0 text-fg-faint" />
                  </button>
                </Tooltip>
              )}
              {showHarness && (
                <button
                  type="button"
                  onClick={() => setPane("models")}
                  className="flex h-8 items-center gap-2 rounded-lg px-2.5 text-left hover:bg-fill"
                >
                  <span className="text-callout text-fg-muted">Harness</span>
                  <span className="flex flex-1 items-center justify-end gap-1.5 truncate font-medium text-callout">
                    <HarnessIcon id={harness?.id ?? "rustcode"} name={harnessName} size={14} />
                    {harnessName}
                  </span>
                  <ChevronRight size={12} strokeWidth={2.5} className="text-fg-faint" />
                </button>
              )}
            </div>
          )}
          {shownPane === "accounts" && (
            <div key="accounts" className="flex flex-col gap-1.5 rc-drill">
              <button
                type="button"
                onClick={() => setPane("main")}
                className="flex h-7 items-center gap-1 self-start rounded-full px-2 font-semibold text-callout text-fg-muted hover:bg-fill"
              >
                <ChevronLeft size={13} />
                Account
              </button>
              {accountRemoved && (
                <div className="flex flex-col gap-0.5 rounded-lg bg-fill-subtle px-2.5 py-1.5">
                  <span className="text-callout text-warning">Removed account · {account}</span>
                  <span className="text-caption text-fg-faint">
                    This login is no longer in Settings › Agents. The thread stays readable; choose Default
                    login or another account to continue it.
                  </span>
                </div>
              )}
              {[
                ...(defaultOffered ? [{ id: null, name: "Default login" }] : []),
                ...accounts.map((a) => ({ id: a.id, name: a.name })),
              ].map((option) => {
                const current = (option.id ?? null) === (account ?? null);
                return (
                  <Tooltip
                    key={option.id ?? "default"}
                    label={accountBlocked ? "Stop the turn to switch account" : undefined}
                  >
                    <button
                      type="button"
                      role="option"
                      aria-selected={current}
                      aria-disabled={accountBlocked}
                      onClick={() => !accountBlocked && pickAccount(option.id)}
                      className={cx(
                        "flex w-full items-center gap-2 rounded-lg px-2.5 py-[7px] text-left text-body",
                        current ? "bg-fill" : "hover:bg-fill-subtle",
                        accountBlocked && "opacity-45",
                      )}
                    >
                      <span className="min-w-0 flex-1 truncate">{option.name}</span>
                      {current && <span className="shrink-0 text-accent">✓</span>}
                    </button>
                  </Tooltip>
                );
              })}
              <p className="px-2.5 pt-0.5 text-caption text-fg-faint">
                Each login has its own plan and limits. Changing account moves the conversation so far as a
                summary, on your next message.
              </p>
            </div>
          )}
          {shownPane === "models" && (
            <div key="models" className="flex flex-col gap-1.5 rc-drill">
              {!listOnly && (
                <button
                  type="button"
                  onClick={() => setPane("main")}
                  className="flex h-7 items-center gap-1 self-start rounded-full px-2 font-semibold text-callout text-fg-muted hover:bg-fill"
                >
                  <ChevronLeft size={13} />
                  Models
                </button>
              )}
              <ModelBrowser
                providers={providers}
                provider={providerId}
                choices={models}
                current={model}
                canSwitch={canSwitch}
                onProvider={pickProvider}
                onPick={pickModel}
              />
            </div>
          )}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
