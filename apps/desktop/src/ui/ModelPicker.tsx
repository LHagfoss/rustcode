// The composer's model pill and its card. The card opens on the effort, because that is what is
// changed most: the current level and the model's name on one header row with Fast mode and the
// reset at its end, a slider over the levels that model itself lists, and the harness's logins
// below. The model name swaps the card for the searchable list of every harness's models (one
// harness on top, the current model checked), and picking one comes back to the slider with the
// effort clamped to the new model's levels. What the thread's harness cannot do is greyed out with
// the reason, from capability flags.

import { Check, ChevronLeft, ChevronRight, ChevronUp, Lock, RotateCcw, Zap } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  type ConfigOption,
  effortLabel,
  type Harness,
  type HarnessAccount,
  type HarnessModel,
  modelName,
} from "../acp/types";
import { clip } from "../lib/clip";
import { matchModels } from "../lib/modelSearch";
import {
  controlDisabledReason,
  defaultEffort,
  normalizeDefault,
  rememberedEffort,
  selectOption,
  unavailableReason,
} from "../state/harness";
import { accountSignedOut, newThreadAccountOn } from "../state/logins";
import {
  effortMemory,
  ensureHarnessModels,
  lastAccountFor,
  type NewThreadChoice,
  newThreadChoice,
  pickNewThreadAccount,
  pickNewThreadChoice,
  setConfigOption,
  switchThreadAccount,
  switchThreadHarness,
  useApp,
} from "../state/store";
import type { ThreadState } from "../state/thread";
import { CardPopover } from "./cardPopover";
import { HarnessIcon } from "./HarnessIcon";
import { cx, MenuSearchField, Select, Skeleton, Tooltip } from "./primitives";

interface Choice {
  value: string;
  name: string;
  description?: string;
}

/**
 * The effort steps from lowest to highest, so `off` sits at the left of the track whatever
 * order the harness listed them in. Unknown values follow the known ones, in the harness's order.
 */
const EFFORT_ORDER = ["off", "none", "default", "minimal", "low", "medium", "high", "xhigh", "max"];

/**
 * The slider thumb's diameter (`size-6`): the track is narrower than it, so each end keeps that room.
 * The `size-6` class and this number move together.
 */
const THUMB = 24;

/**
 * Where a point on the track sits from the pill's left edge, as a length: the thumb's radius at 0,
 * the track's width less that radius at 1, interpolated in between. The fill ends and the thumb
 * centres on it, so the two can never drift apart. Both use `calc`, and the sign is spelled out
 * because `calc(100% + -16px)` is not valid CSS.
 */
function alongTrack(ratio: number): string {
  const pad = THUMB / 2 - ratio * THUMB;
  return `calc(${ratio * 100}% ${pad < 0 ? "-" : "+"} ${Math.abs(pad)}px)`;
}

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
 * The effort a model keeps after its levels changed: the level it was last left on (`remembered`),
 * else the chosen one while the new list still has it, else the model's own default (the lowest
 * step when it names none).
 */
export function clampEffort(
  model: HarnessModel | undefined,
  efforts: string[],
  effort: string | null | undefined,
  remembered?: string | null,
): string | null {
  if (remembered && efforts.includes(remembered)) return remembered;
  if (effort && efforts.includes(effort)) return effort;
  return defaultEffort(model, efforts);
}

/** One write a model row makes on a thread: its config option and the value to set. */
export interface ModelWrite {
  configId: "model" | "effort";
  value: string;
}

/**
 * What a model row writes on an open thread, in order. The model comes first and the level it
 * brings second: the harness keeps a level per model from what the thread runs when the write
 * lands, so an effort sent beside its model would be filed under the model it replaced. A
 * harness's model brings the level it was last left on (`remembered`), else its own default. The
 * level is clamped from nothing after a switch to another harness, which never keeps this one's
 * effort, and from `effort` inside one harness, where the level picked here may stay.
 */
export function modelThreadWrites(
  model: HarnessModel | undefined,
  value: string,
  effort: string | null | undefined,
  remembered: string | null | undefined,
  sameHarness: boolean,
): ModelWrite[] {
  const next = clampEffort(
    model,
    orderEfforts(model?.efforts ?? []),
    sameHarness ? effort : null,
    remembered,
  );
  const writes: ModelWrite[] = [{ configId: "model", value }];
  // A model that lists no levels keeps the effort the thread had.
  if (next && (sameHarness ? next !== effort : true)) writes.push({ configId: "effort", value: next });
  return writes;
}

/**
 * Writes them one after another, so each lands on the thread the one before it left. `write` is
 * the store's `setConfigOption` unless a test hands in its own.
 */
export function writeModelPick(
  id: string,
  writes: ModelWrite[],
  write: (id: string, configId: string, value: string) => Promise<unknown> = setConfigOption,
): Promise<void> {
  return writes
    .reduce<Promise<unknown>>(
      (chain, next) => chain.then(() => write(id, next.configId, next.value)),
      Promise.resolve(),
    )
    .then(() => undefined);
}

/** One login row in the card: a named account, or a thread's own state. */
export interface AccountChip {
  /** The account to run on; `null` is the harness's own sign-in, `undefined` a login that is gone. */
  id: string | null | undefined;
  name: string;
  selected: boolean;
  /** A login removed from Settings: shown as the thread's state, never pressed. */
  gone?: boolean;
  /** The plan read found this login signed out: rows say so and are muted. */
  signedOut?: boolean;
}

/** A harness's logins, only when there is a choice to make between them. */
export function shownAccounts(chips: AccountChip[]): AccountChip[] {
  return chips.length > 1 ? chips : [];
}

/** One harness's models in the list, with what the list needs to offer them. */
interface ModelGroup {
  harness: Harness;
  /** Why none of them can be chosen now: not installed, or a running turn keeps its harness. */
  blocked?: string;
  models: Choice[];
}

/**
 * The effort as a track: one dot per level the model lists, the part up to the thumb filled in
 * the accent, and a round thumb. A press or a drag on the track lands on the nearest dot, ←→ step
 * one level (Home/End the ends), and hovering a dot names it. `role="slider"` with
 * `aria-valuetext` is the level's name, so a screen reader reads "High", not "3".
 */
function EffortSlider({
  levels,
  value,
  onChange,
}: {
  levels: string[];
  value: string;
  onChange: (effort: string) => void;
}) {
  const last = levels.length - 1;
  const at = Math.max(levels.indexOf(value), 0);
  const pct = last > 0 ? at / last : 0;
  const rail = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  // The slider, not the model's name, takes focus when the card opens: ←→ change the effort,
  // which is what the card is for; the model list is Enter or a click on its name.
  useEffect(() => {
    rail.current?.focus({ preventScroll: true });
  }, []);
  const choose = (index: number) => {
    const level = levels[Math.min(Math.max(index, 0), last)];
    if (level && level !== value) onChange(level);
  };
  // A press anywhere on the track goes to the dot nearest it, measured between the thumb's
  // centres at either end so a press on the ends reaches the first and last level.
  const levelAt = (clientX: number) => {
    const rect = rail.current?.getBoundingClientRect();
    const usable = (rect?.width ?? 0) - THUMB;
    if (!rect || usable <= 0) return at;
    const ratio = Math.min(Math.max((clientX - rect.left - THUMB / 2) / usable, 0), 1);
    return Math.round(ratio * last);
  };
  return (
    // The band above the track is the dots' own labels' room, not part of the target.
    <div className="mx-2 pt-6 pb-0.5">
      <div
        ref={rail}
        role="slider"
        tabIndex={0}
        aria-label="Reasoning effort"
        aria-valuemin={0}
        aria-valuemax={last}
        aria-valuenow={at}
        aria-valuetext={effortLabel(levels[at] ?? value)}
        onPointerDown={(e) => {
          e.currentTarget.setPointerCapture(e.pointerId);
          e.currentTarget.focus();
          dragging.current = true;
          choose(levelAt(e.clientX));
        }}
        onPointerMove={(e) => {
          if (dragging.current) choose(levelAt(e.clientX));
        }}
        onPointerUp={() => {
          dragging.current = false;
        }}
        onPointerCancel={() => {
          dragging.current = false;
        }}
        onKeyDown={(e) => {
          const step =
            e.key === "ArrowRight" || e.key === "ArrowUp"
              ? 1
              : e.key === "ArrowLeft" || e.key === "ArrowDown"
                ? -1
                : 0;
          if (step) {
            e.preventDefault();
            choose(at + step);
          } else if (e.key === "Home") {
            e.preventDefault();
            choose(0);
          } else if (e.key === "End") {
            e.preventDefault();
            choose(last);
          }
        }}
        className="relative h-5 cursor-pointer touch-none rounded-full bg-fill-subtle select-none"
      >
        {/* The fill runs from the pill's edge to the thumb's centre; the thumb is wider than
            the track, so its own radius is kept clear at both ends. */}
        <span
          aria-hidden
          className="pointer-events-none absolute inset-y-0 left-0 rounded-full bg-accent transition-[width] duration-200 ease-snappy"
          style={{ width: alongTrack(pct) }}
        />
        <div className="pointer-events-none absolute inset-y-0" style={{ left: THUMB / 2, right: THUMB / 2 }}>
          {levels.map((level, index) => (
            <span
              key={level}
              aria-hidden
              className="group/dot pointer-events-auto absolute top-1/2 flex size-5 -translate-x-1/2 -translate-y-1/2 items-center justify-center"
              style={{ left: last > 0 ? `${(index / last) * 100}%` : "0%" }}
            >
              <span
                className={cx(
                  "block size-[5px] rounded-full",
                  // The theme's own contrast on the accent where the dot sits on the fill (a light
                  // accent gets dark dots), a quiet grey on the track.
                  index < at ? "bg-accent-fg/70" : "bg-fg-muted/50",
                )}
              />
              <span className="pointer-events-none absolute bottom-full left-1/2 z-10 mb-1.5 -translate-x-1/2 rounded-full bg-surface-3 px-1.5 py-0.5 font-medium whitespace-nowrap text-caption text-fg opacity-0 shadow-1 transition-opacity group-hover/dot:opacity-100">
                {effortLabel(level)}
              </span>
            </span>
          ))}
        </div>
        <span
          aria-hidden
          // The active control's knob (docs/design.md › Active controls): white in every theme,
          // with the hairline that keeps it apart from a light track where white alone would not
          // read. A theme may set a dark `accent-text`; the knob still reads as the grip.
          className="pointer-events-none absolute top-1/2 size-6 -translate-x-1/2 -translate-y-1/2 rounded-full bg-knob shadow-knob transition-[left] duration-200 ease-snappy"
          style={{ left: alongTrack(pct) }}
        />
      </div>
    </div>
  );
}

/**
 * The card's first view: one header row with the effort and the model's name, Fast mode and the
 * reset at its end, then the slider, then the harness's logins. No effort control on the harness,
 * and the slider's place says why.
 */
function EffortCard({
  levels,
  effort,
  model,
  harness,
  defaultLevel,
  fast,
  fastOn,
  effortReason,
  modelReason,
  blocked,
  accounts,
  accountsDisabled,
  onEffort,
  onFast,
  onModels,
  onPickAccount,
}: {
  levels: string[];
  effort?: string | null;
  model?: string | null;
  harness?: Harness;
  defaultLevel: string | null;
  fast?: ConfigOption;
  fastOn: boolean;
  effortReason?: string;
  /** Why the harness picks its own model: the model's name cannot open the list. */
  modelReason?: string;
  /** Why nothing here runs now (an unavailable harness), under the model's name. */
  blocked?: string;
  accounts: AccountChip[];
  accountsDisabled: boolean;
  onEffort: (effort: string) => void;
  onFast: (on: boolean) => void;
  onModels: () => void;
  onPickAccount: (id: string | null) => void;
}) {
  const name = model ?? "Default model";
  const showEffort = effortsShown(levels, effort);
  return (
    <div className="flex flex-col pb-0.5">
      <div className="flex items-center gap-1 px-1.5 pt-1 pb-0.5">
        <div className="flex min-w-0 flex-1 items-center gap-1">
          <button
            type="button"
            aria-label={`Model: ${name}`}
            disabled={!!modelReason}
            onClick={onModels}
            className="flex min-w-0 items-center gap-0.5 rounded-full py-0.5 pr-1.5 pl-1 text-body font-semibold text-fg enabled:hover:bg-fill disabled:opacity-40"
          >
            <HarnessIcon id={harness?.id ?? "rustcode"} name={harness?.name} size={16} />
            <span className="truncate">{name}</span>
            <ChevronRight size={11} strokeWidth={2.5} className="shrink-0" />
          </button>
        </div>
        {fast && (
          <Tooltip label="Fast mode" side="bottom">
            <button
              type="button"
              role="switch"
              aria-checked={fastOn}
              aria-label="Fast mode"
              onClick={() => onFast(!fastOn)}
              className={cx(
                "flex size-6 shrink-0 items-center justify-center rounded-full",
                fastOn
                  ? "bg-fill-strong text-accent"
                  : "text-fg-muted enabled:hover:bg-fill enabled:hover:text-fg",
              )}
            >
              <Zap size={13} fill={fastOn ? "currentColor" : "none"} />
            </button>
          </Tooltip>
        )}
        <Tooltip label="Reset effort to default" side="bottom">
          <button
            type="button"
            aria-label="Reset effort to default"
            disabled={!!effortReason || !defaultLevel || defaultLevel === effort}
            onClick={() => defaultLevel && onEffort(defaultLevel)}
            className="flex size-6 shrink-0 items-center justify-center rounded-full text-fg-muted enabled:hover:bg-fill enabled:hover:text-fg disabled:opacity-40"
          >
            <RotateCcw size={12} />
          </button>
        </Tooltip>
      </div>
      {showEffort && <span className="px-3 pt-1 text-callout text-fg-muted">{effortLabel(effort!)}</span>}
      <div className="px-1.5">
        {effortReason ? (
          <p className="flex items-start gap-1.5 px-2 py-1.5 text-caption text-fg-faint">
            <Lock size={10} className="mt-[2px] shrink-0" />
            <span>{effortReason}</span>
          </p>
        ) : levels.length > 1 && effort ? (
          <EffortSlider levels={levels} value={effort} onChange={onEffort} />
        ) : null}
      </div>
      {blocked && <p className="px-3 pb-1 text-caption text-fg-faint">{blocked}</p>}
      {accounts.length > 0 && (
        <div className="mt-0.5 border-border border-t px-1.5 pt-1">
          <div className="flex items-baseline gap-1.5 px-1 pb-0.5 text-caption text-fg-faint">
            <span>Account</span>
            {accountsDisabled && <span className="ms-auto">Stop the turn to switch</span>}
          </div>
          {accounts.length > 3 ? (
            <Select
              label="Account"
              value={accounts.findIndex((chip) => chip.selected).toString()}
              disabled={accountsDisabled}
              options={accounts.map((chip, index) => ({
                value: String(index),
                label: chip.name,
                disabled: chip.gone || chip.signedOut,
              }))}
              onChange={(value) => {
                const chip = accounts[Number(value)];
                if (chip && chip.id !== undefined) onPickAccount(chip.id);
              }}
            />
          ) : (
            <div className="flex gap-1 rounded-md bg-fill-subtle p-1">
              {accounts.map((chip) => (
                <button
                  key={chip.id ?? chip.name}
                  type="button"
                  aria-pressed={chip.selected}
                  disabled={accountsDisabled || chip.gone || chip.signedOut}
                  title={chip.signedOut ? "This account is not signed in" : undefined}
                  aria-label={chip.signedOut ? `${chip.name} (not signed in)` : undefined}
                  onClick={() => {
                    if (chip.id === undefined) return;
                    onPickAccount(chip.id);
                  }}
                  className={cx(
                    "flex h-6 min-w-0 flex-1 items-center gap-1 rounded-md px-2 text-left text-callout",
                    chip.selected
                      ? "bg-fill font-medium text-fg"
                      : "text-fg-muted enabled:hover:bg-fill enabled:hover:text-fg",
                    chip.signedOut && !chip.selected && "text-fg-faint",
                    (accountsDisabled || chip.gone) && "opacity-40",
                    chip.gone && "italic",
                  )}
                >
                  <span className="min-w-0 flex-1 truncate">{chip.name}</span>
                  {chip.signedOut && <span className="shrink-0 text-caption text-fg-faint">Signed out</span>}
                  {chip.selected && <Check size={12} strokeWidth={2.5} className="shrink-0 text-accent" />}
                </button>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/** Whether the card names an effort: a level to show and the model's own word for it. */
function effortsShown(levels: string[], effort?: string | null): boolean {
  return levels.length > 0 && !!effort;
}

/**
 * The list itself: a search that opens focused, every harness's models under a sticky header
 * (the current harness first) and the current model checked, behind a back button to the effort.
 * ↑↓ move the highlight, Home/End the ends, PageUp/PageDown a screen, ⏎ picks, and Escape clears
 * the search before it closes the card.
 */
function ModelPanel({
  groups,
  current,
  onPick,
  onBack,
}: {
  groups: ModelGroup[];
  current?: string | null;
  onPick: (group: ModelGroup, value: string) => void;
  onBack: () => void;
}) {
  const [filter, setFilter] = useState("");
  const [provider, setProvider] = useState(groups[0]?.harness.id ?? "");
  const [column, setColumn] = useState<"providers" | "models">("models");
  const [active, setActive] = useState(0);
  const field = useRef<HTMLInputElement>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const searching = !!filter.trim();
  const rows = useMemo(() => filterModelRows(groups, provider, filter), [groups, provider, filter]);
  useEffect(() => {
    field.current?.focus();
  }, []);
  useEffect(() => {
    scroller.current?.querySelector(`[id="model-row-${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);
  return (
    <div
      className="flex min-h-0 flex-1 flex-col"
      onKeyDown={(e) => {
        if (e.key === "ArrowLeft") {
          e.preventDefault();
          setColumn("providers");
        } else if (e.key === "ArrowRight") {
          e.preventDefault();
          setColumn("models");
        } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          e.preventDefault();
          const delta = e.key === "ArrowDown" ? 1 : -1;
          if (column === "providers" && !searching) {
            const next = movePickerIndex(
              groups.findIndex((g) => g.harness.id === provider),
              delta,
              groups.length,
            );
            setProvider(groups[next]?.harness.id ?? provider);
            setActive(0);
          } else setActive((i) => movePickerIndex(i, delta, rows.length));
        } else if (e.key === "Enter" && column === "models") {
          const row = rows[active];
          if (row && !row.group.blocked) {
            e.preventDefault();
            onPick(row.group, row.model.value);
          }
        }
      }}
    >
      <div className="flex items-center gap-1 p-1">
        <button
          type="button"
          aria-label="Back to effort"
          onClick={onBack}
          className="flex size-6 shrink-0 items-center justify-center rounded-full text-fg-muted hover:bg-fill"
        >
          <ChevronLeft size={14} />
        </button>
        <MenuSearchField
          value={filter}
          onChange={(v) => {
            setFilter(v);
            setActive(0);
            setColumn("models");
          }}
          placeholder="Search models"
          label="Search models"
          inputRef={field}
          controls="model-list"
          activeDescendant={rows[active] ? `model-row-${active}` : undefined}
        />
      </div>
      <div className="px-2 min-[480px]:hidden">
        <Select
          label="Provider"
          value={provider}
          options={groups.map((g) => ({ value: g.harness.id, label: g.harness.name }))}
          onChange={(v) => {
            setProvider(v);
            setActive(0);
          }}
        />
      </div>
      <div className="flex min-h-0 flex-1">
        <div
          className="hidden w-36 shrink-0 overflow-auto border-r border-border p-1 min-[480px]:block"
          role="group"
          aria-label="Providers"
        >
          {groups.map((g) => (
            <button
              type="button"
              key={g.harness.id}
              onClick={() => {
                setProvider(g.harness.id);
                setFilter("");
                setActive(0);
                setColumn("providers");
              }}
              className={cx(
                "flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-callout",
                provider === g.harness.id && !searching && "font-semibold text-accent",
              )}
            >
              <HarnessIcon id={g.harness.id} name={g.harness.name} size={14} />
              <span className="truncate">{g.harness.name}</span>
            </button>
          ))}
        </div>
        <div
          ref={scroller}
          id="model-list"
          role="listbox"
          aria-label="Models"
          className="scroll-stable min-h-0 min-w-0 flex-1 p-1"
        >
          {rows.map((row, i) => (
            <div
              key={`${row.group.harness.id}:${row.model.value}`}
              id={`model-row-${i}`}
              role="option"
              tabIndex={-1}
              aria-selected={row.group.harness.id === groups[0]?.harness.id && row.model.value === current}
              aria-disabled={!!row.group.blocked}
              onMouseEnter={() => setActive(i)}
              onClick={() => !row.group.blocked && onPick(row.group, row.model.value)}
              className={cx(
                "flex cursor-pointer items-center gap-2 rounded-md px-2 py-1",
                i === active && "bg-fill",
                row.group.blocked && "opacity-45",
              )}
            >
              <span className="min-w-0 flex-1">
                <span className="block truncate text-body">{row.model.name || row.model.value}</span>
                {searching && (
                  <span className="block truncate text-caption text-fg-muted">{row.group.harness.name}</span>
                )}
              </span>
              {row.group.harness.id === groups[0]?.harness.id && row.model.value === current && (
                <Check size={13} className="text-accent" />
              )}
            </div>
          ))}
          {!rows.length && (
            <div className="px-2 py-3 text-center text-callout text-fg-muted">
              {searching ? "No matching models" : "No models listed"}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

export function movePickerIndex(index: number, delta: number, count: number): number {
  return Math.min(Math.max(index + delta, 0), Math.max(count - 1, 0));
}

export function filterModelRows(groups: ModelGroup[], provider: string, filter: string) {
  const searching = !!filter.trim();
  return groups.flatMap((group) =>
    searching || group.harness.id === provider
      ? (searching ? matchModels(filter, group.models).shown : group.models).map((model) => ({
          group,
          model,
        }))
      : [],
  );
}

export function pillLabel(model?: string | null, effort?: string | null, compact = false) {
  const name = model || "Default model";
  return {
    name: compact && name.length > 14 ? `${clip(name, 13)}…` : name,
    effort: compact ? undefined : effort,
  };
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
 * steps sit on the model's default effort.
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
    models = models.map((choice) => ({
      ...choice,
      name: harness?.models.find((entry) => entry.id === choice.value)?.name || choice.name || choice.value,
    }));
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
  // A new thread on an external harness that has not listed its models yet: its own choice, never
  // another thread's. `knownConfig` is whichever thread happens to be open, so it can only speak for
  // the built-in harness: that thread's effort is not this harness's, its model name is not one this
  // harness runs, and picking an effort here sends the shown model with the choice (`setChoice`).
  if (harness && !harness.builtin) {
    return { models: [], model: effective.model, efforts: [], effort: effective.effort };
  }
  // The built-in harness, or no snapshot yet: the model and effort list an open thread knows.
  const m = selectOption(knownConfig, "model");
  const e = selectOption(knownConfig, "effort");
  return {
    models: m?.choices ?? [],
    model: effective.model ?? m?.current,
    efforts: orderEfforts(e?.choices.map((c) => c.value) ?? []),
    effort: effective.effort ?? e?.current,
  };
}

type Page = "effort" | "models";

export function ModelPicker({ thread, compact }: { thread?: ThreadState; compact?: boolean }) {
  const harnesses = useApp((s) => s.harnesses);
  const choice = useApp((s) => s.newThread);
  // The first open thread's config options: stable while a reply streams, so the picker does not re-render per chunk.
  const knownConfig = useApp((s) => firstConfig(s.threads));
  const [open, setOpen] = useState(false);
  const [page, setPage] = useState<Page>("effort");
  // A swap plays the drill motion; only after the user moved between the two views.
  const [swapped, setSwapped] = useState(false);
  // `/model` asks the picker to open (store.runCommandAction).
  const ask = useApp((s) => s.modelPickerAsk);
  const seenAsk = useRef(ask);
  useEffect(() => {
    if (ask === seenAsk.current) return;
    seenAsk.current = ask;
    setSwapped(false);
    setPage("models");
    setOpen(true);
  }, [ask]);
  // Every open starts on the effort, the view changed most; a close leaves nothing behind.
  useEffect(() => {
    if (open) return;
    setSwapped(false);
    setPage("effort");
  }, [open]);
  const turnPage = (next: Page) => {
    setSwapped(true);
    setPage(next);
  };

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
  // A new thread starts on one of the harness's named accounts while it has
  // any (state/logins.ts); an open thread keeps its own, so the picker says
  // what it really runs on.
  const signedOut = useApp((s) => s.signedOutLogins);
  const newAccount = useApp((s) => newThreadAccountOn(harness, s.newThreadAccount, s.signedOutLogins));
  const account = isNew ? newAccount : (thread?.harness?.account ?? null);
  // A running thread keeps its login, as it keeps its harness.
  const accountBlocked = !isNew && !!thread?.running;
  const engineReady = useApp((s) => s.status === "ready");

  const { models, efforts, model, effort } = pickerModels(thread, harness, effective, knownConfig);
  // The model's own entry: what its effort levels are, and where the reset returns to.
  const modelEntry = harness?.models.find((m) => m.id === model);
  const defaultLevel = defaultEffort(modelEntry, efforts);
  const fast = thread?.configOptions.find((o) => o.id === "fast" && o.type === "boolean");
  const harnessName = harness?.name ?? "This harness";
  const effortReason = controlDisabledReason("effort", caps, harnessName);
  const modelReason = controlDisabledReason("model", caps, harnessName);
  const harnessBlocked = harness ? unavailableReason(harness) : undefined;

  const setChoice = (next: Partial<NewThreadChoice>) => {
    const merged = { ...effective, model: model ?? null, effort: effort ?? null, ...next };
    pickNewThreadChoice(harnesses ? normalizeDefault(harnesses, merged) : merged);
  };
  const pickEffort = (value: string) => {
    if (value === effort) return;
    if (thread) void setConfigOption(thread.id, "effort", value);
    else setChoice({ effort: value });
  };
  /**
   * A login row. A new thread keeps the choice for its next one; an idle
   * thread moves to it with the conversation so far as a summary, exactly as
   * switching harness does.
   */
  const pickAccount = (id: string | null) => {
    if (!harness) return;
    if (isNew) pickNewThreadAccount(id);
    // The row of the login the thread is already on asks nothing of the engine.
    else if (thread && !thread.running && (thread.harness?.account ?? null) !== id)
      void switchThreadAccount(thread.id, id);
    else return;
    setOpen(false);
  };

  /** The login rows the card shows: the thread's own harness, its named logins and its state. */
  const accountChips = (h: Harness): AccountChip[] => {
    const named = h.accounts ?? [];
    const chip = (a: HarnessAccount, selected: boolean): AccountChip => ({
      id: a.id,
      name: a.name,
      selected,
      // A definite `needs_login` is as signed out as a row the plan read saw.
      signedOut: a.auth === "needs_login" || accountSignedOut(h.id, a.id, signedOut),
    });
    // A new thread starts on one of the named logins while the harness has any.
    if (isNew) return named.map((a) => chip(a, a.id === account));
    const own = account;
    const chips: AccountChip[] = [];
    // A thread from before this harness had accounts keeps its own sign-in:
    // it stays readable and is only moved by hand.
    if (!own && named.length) chips.push({ id: null, name: "Original login", selected: true });
    // Its login was removed from Settings; without another, its way back is
    // the harness's own sign-in.
    if (own && !named.some((a) => a.id === own)) {
      chips.push({ id: undefined, name: "Removed", selected: true, gone: true });
      if (!named.length) chips.push({ id: null, name: "Harness login", selected: false });
    }
    chips.push(...named.map((a) => chip(a, own === a.id)));
    return chips;
  };

  // The harnesses worth listing: RustCode turned off in Settings › Agents leaves the list, and a
  // thread moves only to an external harness (RustCode cannot take one over yet).
  const providers = (harnesses?.harnesses ?? []).filter((h) =>
    isNew ? !h.builtin || h.enabled || h.id === effective.harness : !h.builtin || h.id === harness?.id,
  );
  const providerId = isNew ? effective.harness : (harness?.id ?? "rustcode");
  const canSwitch = isNew || !thread?.running;
  // The current harness first, then the rest in the order Settings lists them.
  const ordered = [
    ...providers.filter((h) => h.id === providerId),
    ...providers.filter((h) => h.id !== providerId),
  ];
  const groups: ModelGroup[] = ordered.map((h) => {
    const isCurrent = h.id === providerId;
    return {
      harness: h,
      blocked: unavailableReason(h) ?? (isCurrent || canSwitch ? undefined : "Stop the turn to switch"),
      models: isCurrent ? models : h.models.map((m) => ({ value: m.id, name: m.name })),
    };
  });
  // The current harness's logins, and only when there is more than one to choose between.
  const accountRows = harness ? shownAccounts(accountChips(harness)) : [];

  // An external harness lists its models only when asked: ask for every harness on show once the
  // card opens, so a group never sits empty behind a "no models" line it could have answered.
  const listing = groups
    .filter((g) => !g.blocked && !g.models.length)
    .map((g) => g.harness.id)
    .join();
  useEffect(() => {
    if (!open || !engineReady || !listing) return;
    for (const id of listing.split(",")) void ensureHarnessModels(id);
  }, [open, engineReady, listing]);

  /**
   * A model row. In the current harness it is set in place and the level the
   * new model does not offer is clamped to its default; another harness is
   * switched to with its remembered login, as its model row always did. Either
   * way the card goes back to the effort.
   */
  const pickModel = (group: ModelGroup, value: string) => {
    if (group.blocked) return;
    // The level the model was last left on, so picking it again returns to that level; a level
    // its list no longer offers is left to the model's default (`clampEffort`).
    const entry = group.harness.models.find((m) => m.id === value);
    const remembered = rememberedEffort(effortMemory(), group.harness.id, value);
    const same = group.harness.id === providerId;
    if (same) {
      if (thread) void writeModelPick(thread.id, modelThreadWrites(entry, value, effort, remembered, true));
      else {
        // A harness that lists no levels for the model keeps the effort it had.
        const next = clampEffort(entry, orderEfforts(entry?.efforts ?? []), effort, remembered);
        setChoice(entry?.efforts.length ? { model: value, effort: next } : { model: value });
      }
    } else if (isNew) {
      // Another harness's model brings the level it was last left on, never this harness's
      // effort: the choice normalizes it, and its remembered account goes with it.
      const next = clampEffort(entry, orderEfforts(entry?.efforts ?? []), null, remembered);
      setChoice({ harness: group.harness.id, model: value, effort: next });
      pickNewThreadAccount(newThreadAccountOn(group.harness, lastAccountFor(group.harness.id), signedOut));
    } else if (thread) {
      // The harness's remembered login goes with the model, as a login row would, and the model
      // brings the level it was last left on once the switch lands. That harness never keeps
      // this one's effort, so the level is clamped from nothing.
      const account = newThreadAccountOn(group.harness, lastAccountFor(group.harness.id), signedOut);
      const writes = modelThreadWrites(entry, value, null, remembered, false);
      void switchThreadHarness(thread.id, group.harness.id, account).then((switched) => {
        if (switched) void writeModelPick(thread.id, writes);
      });
    }
    turnPage("effort");
  };

  // A catalog name is shown as it is; a bare id reads without its `provider/` prefix.
  const catalogName = modelEntry?.name || models.find((item) => item.value === model)?.name;
  const displayName = catalogName && catalogName !== model ? catalogName : modelName(model ?? undefined);
  const label = pillLabel(displayName, effortReason ? null : effort, compact);
  // The pill names the login whenever the harness has named accounts, even
  // when there is one: which subscription a thread spends is worth a word.
  // A thread from before accounts says which login it is really on.

  // Nothing to show until the engine lists harnesses (or the thread its options): a placeholder pill.
  if (!harnesses && !thread?.configOptions.length) {
    return (
      <span role="status" aria-label="Loading models" className="flex h-[30px] items-center">
        <Skeleton className="h-[30px] w-[132px] rounded-full" />
      </span>
    );
  }

  return (
    <CardPopover
      open={open}
      onOpenChange={setOpen}
      role="dialog"
      label="Model and effort"
      // The card focuses its own control (the slider, or the search field from `/model`) so the
      // arrow keys act on what the card is for the moment it opens.
      onOpenAutoFocus={(event) => event.preventDefault()}
      // One width for both views: the effort card is small, and the model list lives in the same
      // box, so swapping between them never resizes the card.
      className="w-[min(480px,calc(100vw-32px))] p-0"
      style={{ maxHeight: "min(430px, var(--radix-popper-available-height, 430px))" }}
      trigger={
        <button
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
          <ChevronUp
            size={11}
            strokeWidth={2.5}
            className={cx("shrink-0 transition-transform duration-200 ease-snappy", open && "rotate-180")}
          />
        </button>
      }
    >
      <div
        key={page}
        className={cx(
          "flex min-h-0 flex-1 flex-col",
          swapped && (page === "models" ? "rc-drill" : "rc-drill-back"),
        )}
      >
        {page === "models" ? (
          <ModelPanel groups={groups} current={model} onPick={pickModel} onBack={() => turnPage("effort")} />
        ) : (
          <EffortCard
            levels={efforts}
            effort={effort}
            model={displayName}
            harness={harness}
            defaultLevel={defaultLevel}
            fast={fast}
            fastOn={!!fast?.currentBool}
            effortReason={effortReason}
            modelReason={modelReason}
            blocked={modelReason ?? harnessBlocked}
            accounts={accountRows}
            accountsDisabled={accountBlocked || !!harnessBlocked}
            onEffort={pickEffort}
            onFast={(on) => thread && void setConfigOption(thread.id, "fast", on)}
            onModels={() => turnPage("models")}
            onPickAccount={pickAccount}
          />
        )}
      </div>
    </CardPopover>
  );
}
