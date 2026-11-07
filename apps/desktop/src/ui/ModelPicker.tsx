// The composer's model pill and its card. The card opens on the effort, because that is what is
// changed most: the current level as a large word, the model under it, a slider over the levels
// that model itself lists, the harness's logins below and Fast mode beside the reset. The model
// name swaps the card for the searchable list of every harness's models (one harness on top, the
// current model checked), and picking one comes back to the slider with the effort clamped to the
// new model's levels. What the thread's harness cannot do is greyed out with the reason, from
// capability flags.

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
  selectOption,
  unavailableReason,
} from "../state/harness";
import { accountSignedOut, newThreadAccountOn } from "../state/logins";
import {
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
import { cx, MenuSearchField, Skeleton, Tooltip } from "./primitives";

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
 * The effort a model keeps after its levels changed: the chosen one while the new list still has
 * it, else the model's own default (the lowest step when it names none).
 */
export function clampEffort(
  model: HarnessModel | undefined,
  efforts: string[],
  effort: string | null | undefined,
): string | null {
  if (effort && efforts.includes(effort)) return effort;
  return defaultEffort(model, efforts);
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
  const choose = (index: number) => {
    const level = levels[Math.min(Math.max(index, 0), last)];
    if (level && level !== value) onChange(level);
  };
  // A press anywhere on the track goes to the dot nearest it.
  const levelAt = (clientX: number) => {
    const rect = rail.current?.getBoundingClientRect();
    if (!rect || rect.width <= 0) return at;
    const ratio = Math.min(Math.max((clientX - rect.left) / rect.width, 0), 1);
    return Math.round(ratio * last);
  };
  return (
    // The band above the track is the dots' own labels' room, not part of the target.
    <div className="mx-2 pt-5">
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
        className="relative h-4 cursor-pointer touch-none select-none"
      >
        <div className="relative h-4">
          <span
            aria-hidden
            className="pointer-events-none absolute inset-x-0 top-1/2 h-[3px] -translate-y-1/2 rounded-full bg-fill-strong"
          />
          <span
            aria-hidden
            className="pointer-events-none absolute top-1/2 left-0 h-[3px] -translate-y-1/2 rounded-full bg-accent transition-[width] duration-200 ease-snappy"
            style={{ width: `${pct * 100}%` }}
          />
          {levels.map((level, index) => (
            <span
              key={level}
              aria-hidden
              className="group/dot absolute top-1/2 flex size-4 -translate-x-1/2 -translate-y-1/2 items-center justify-center"
              style={{ left: last > 0 ? `${(index / last) * 100}%` : "0%" }}
            >
              <span
                className={cx("block size-[6px] rounded-full", index < at ? "bg-accent-fg" : "bg-fg-faint")}
              />
              <span className="pointer-events-none absolute bottom-full left-1/2 z-10 mb-1 -translate-x-1/2 rounded-full bg-surface-3 px-1.5 py-0.5 font-medium whitespace-nowrap text-caption text-fg opacity-0 shadow-1 transition-opacity group-hover/dot:opacity-100">
                {effortLabel(level)}
              </span>
            </span>
          ))}
          <span
            aria-hidden
            className="pointer-events-none absolute top-1/2 size-[13px] -translate-x-1/2 -translate-y-1/2 rounded-full bg-accent shadow-1 ring-2 ring-surface-2 transition-[left] duration-200 ease-snappy"
            style={{ left: `${pct * 100}%` }}
          />
        </div>
      </div>
    </div>
  );
}

/**
 * The card's first view: Fast mode and the reset on top, the effort big under them, the model
 * under that, the slider, then the harness's logins. No effort control on the harness, and the
 * slider's place says why.
 */
function EffortCard({
  levels,
  effort,
  model,
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
  const name = modelName(model ?? undefined) ?? "Default model";
  return (
    <div className="flex flex-col pb-0.5">
      <div className="flex items-center justify-between px-1.5 pt-0.5">
        {fast ? (
          <Tooltip label="Fast mode" side="bottom">
            <button
              type="button"
              role="switch"
              aria-checked={fastOn}
              aria-label="Fast mode"
              onClick={() => onFast(!fastOn)}
              className={cx(
                "flex size-7 items-center justify-center rounded-full",
                fastOn
                  ? "bg-fill-strong text-accent"
                  : "text-fg-muted enabled:hover:bg-fill enabled:hover:text-fg",
              )}
            >
              <Zap size={14} fill={fastOn ? "currentColor" : "none"} />
            </button>
          </Tooltip>
        ) : (
          <span aria-hidden className="size-7" />
        )}
        <Tooltip label="Reset effort to default" side="bottom">
          <button
            type="button"
            aria-label="Reset effort to default"
            disabled={!!effortReason || !defaultLevel || defaultLevel === effort}
            onClick={() => defaultLevel && onEffort(defaultLevel)}
            className="flex size-7 items-center justify-center rounded-full text-fg-muted enabled:hover:bg-fill enabled:hover:text-fg disabled:opacity-40"
          >
            <RotateCcw size={13} />
          </button>
        </Tooltip>
      </div>
      <div className="flex flex-col items-center gap-0.5 px-3 pt-0.5 pb-1">
        {effortsShown(levels, effort) && (
          <span className="text-title2 font-semibold text-accent">{effortLabel(effort!)}</span>
        )}
        <button
          type="button"
          aria-label={`Model: ${name}`}
          disabled={!!modelReason}
          onClick={onModels}
          // Enter is the button's own; → opens the list, the way the arrow beside it points.
          onKeyDown={(e) => {
            if (e.key !== "ArrowRight") return;
            e.preventDefault();
            onModels();
          }}
          className="flex max-w-full items-center gap-0.5 rounded-full px-2 py-0.5 text-callout text-fg-muted enabled:hover:bg-fill enabled:hover:text-fg disabled:opacity-40"
        >
          <span className="truncate">{name}</span>
          <ChevronRight size={11} strokeWidth={2.5} className="shrink-0" />
        </button>
      </div>
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
        <div className="mt-1 border-border border-t px-1.5 pt-1.5">
          <div className="flex items-baseline gap-1.5 px-1 pb-0.5 text-caption text-fg-faint">
            <span>Account</span>
            {accountsDisabled && <span className="ms-auto">Stop the turn to switch</span>}
          </div>
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
                "flex h-7 w-full items-center gap-2 rounded-md px-2 text-left text-callout",
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
  const [active, setActive] = useState(0);
  const field = useRef<HTMLInputElement>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const searching = !!filter.trim();
  const { sections, rows, index } = useMemo(() => {
    const shown = groups.map((group) => ({
      group,
      // One matchModels per group per keystroke, not one per section and one for the flat rows.
      models: searching ? matchModels(filter, group.models).shown : group.models,
    }));
    const sections = shown.filter((s) => !searching || s.models.length);
    const rows = sections.flatMap((s) => s.models.map((model) => ({ group: s.group, model })));
    // Where each row sits in the flat list, read once instead of a findIndex per rendered row.
    const index = new Map(rows.map((row, at) => [`${row.group.harness.id}:${row.model.value}`, at]));
    return { sections, rows, index };
  }, [groups, filter, searching]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: a new query starts at the top
  useEffect(() => setActive(0), [filter]);
  useEffect(() => {
    field.current?.focus();
  }, []);
  // biome-ignore lint/correctness/useExhaustiveDependencies: the highlight moved, so follow it
  useEffect(() => {
    scroller.current?.querySelector('[data-active="true"]')?.scrollIntoView({ block: "nearest" });
  }, [active]);
  const move = (by: number) => setActive((i) => Math.min(Math.max(i + by, 0), Math.max(rows.length - 1, 0)));
  return (
    <div
      className="flex min-h-0 flex-1 flex-col"
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          // A search clears first; the second Escape closes the card (Radix's own dismiss).
          if (!filter) return;
          e.preventDefault();
          e.stopPropagation();
          setFilter("");
          return;
        }
        if (e.key === "ArrowDown") {
          e.preventDefault();
          move(1);
        } else if (e.key === "ArrowUp") {
          e.preventDefault();
          move(-1);
        } else if (e.key === "PageDown") {
          e.preventDefault();
          move(6);
        } else if (e.key === "PageUp") {
          e.preventDefault();
          move(-6);
        } else if (e.key === "Home") {
          e.preventDefault();
          setActive(0);
        } else if (e.key === "End") {
          e.preventDefault();
          setActive(Math.max(rows.length - 1, 0));
        } else if (e.key === "Enter") {
          const row = rows[active];
          if (row && !row.group.blocked) {
            e.preventDefault();
            onPick(row.group, row.model.value);
          }
        }
      }}
    >
      <div className="flex items-center gap-1 px-1 pt-1 pb-0.5">
        <button
          type="button"
          aria-label="Back to effort"
          onClick={onBack}
          className="flex size-6 shrink-0 items-center justify-center rounded-full text-fg-muted enabled:hover:bg-fill enabled:hover:text-fg"
        >
          <ChevronLeft size={14} strokeWidth={2.5} />
        </button>
        <span className="text-sub font-medium">Model</span>
      </div>
      <div className="p-1 pb-1.5">
        <MenuSearchField
          value={filter}
          onChange={setFilter}
          placeholder="Search models"
          label="Search models"
          inputRef={field}
          controls="model-list"
          activeDescendant={
            rows[active] ? `model-${rows[active]!.group.harness.id}-${rows[active]!.model.value}` : undefined
          }
        />
      </div>
      <div
        ref={scroller}
        id="model-list"
        role="listbox"
        aria-label="Models"
        className="scroll-stable min-h-0 flex-1 px-1 pb-1"
      >
        {sections.map((section) => (
          <div key={section.group.harness.id}>
            {!searching && (
              <div className="sticky top-0 z-10 -mx-1 flex items-start gap-1.5 bg-surface-2 px-3 pt-1.5 pb-1 text-caption text-fg-faint">
                <HarnessIcon
                  id={section.group.harness.id}
                  name={section.group.harness.name}
                  size={12}
                  className="mt-0.5 shrink-0"
                />
                <span className="min-w-0 truncate py-0.5">{section.group.harness.name}</span>
                {section.group.blocked && (
                  <span className="ms-auto shrink-0 truncate">{section.group.blocked}</span>
                )}
              </div>
            )}
            {section.models.map((choice) => {
              const key = `${section.group.harness.id}:${choice.value}`;
              const at = index.get(key) ?? 0;
              const isCurrent =
                choice.value === current && section.group.harness.id === groups[0]?.harness.id;
              return (
                <div
                  key={key}
                  id={`model-${section.group.harness.id}-${choice.value}`}
                  role="option"
                  tabIndex={-1}
                  aria-selected={isCurrent}
                  aria-disabled={!!section.group.blocked}
                  data-active={at === active}
                  data-model-row=""
                  onMouseEnter={() => setActive(at)}
                  onClick={() => !section.group.blocked && onPick(section.group, choice.value)}
                  className={cx(
                    "flex min-h-7 w-full cursor-pointer items-center gap-2 rounded-md px-2 py-1 text-left",
                    at === active && "bg-fill",
                    section.group.blocked && "cursor-default opacity-45",
                  )}
                >
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-body">{modelName(choice.name)}</span>
                    {choice.description && choice.description !== modelName(choice.name) && (
                      <span className="block truncate text-caption text-fg-muted">{choice.description}</span>
                    )}
                  </span>
                  {isCurrent && <Check size={13} strokeWidth={2.5} className="shrink-0 text-accent" />}
                </div>
              );
            })}
          </div>
        ))}
        {!rows.length && (
          <div className="px-2.5 py-3 text-center text-callout text-fg-muted">
            {searching ? "No matching models" : "No models listed"}
          </div>
        )}
        {!searching && rows.length > 0 && <span className="sr-only">{rows.length} models</span>}
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
  const accounts = harness?.accounts ?? [];
  // A new thread starts on one of the harness's named accounts while it has
  // any (state/logins.ts); an open thread keeps its own, so the picker says
  // what it really runs on.
  const signedOut = useApp((s) => s.signedOutLogins);
  const newAccount = useApp((s) => newThreadAccountOn(harness, s.newThreadAccount, s.signedOutLogins));
  const account = isNew ? newAccount : (thread?.harness?.account ?? null);
  const accountName = accounts.find((a) => a.id === account)?.name;
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
    if (group.harness.id === providerId) {
      const entry = harness?.models.find((m) => m.id === value);
      const levels = orderEfforts(entry?.efforts ?? []);
      const next = clampEffort(entry, levels, effort);
      if (thread) {
        void setConfigOption(thread.id, "model", value);
        if (next && next !== effort) void setConfigOption(thread.id, "effort", next);
      } else {
        // A harness that lists no levels for the model keeps the effort it had.
        setChoice(levels.length ? { model: value, effort: next } : { model: value });
      }
    } else if (isNew) {
      // A harness's remembered effort belongs to its own models: the choice normalizes it,
      // and its remembered account goes with it.
      setChoice({ harness: group.harness.id, model: value, effort: null });
      pickNewThreadAccount(newThreadAccountOn(group.harness, lastAccountFor(group.harness.id), signedOut));
    } else if (thread) {
      // The harness's remembered login goes with the model, as a login row would.
      const account = newThreadAccountOn(group.harness, lastAccountFor(group.harness.id), signedOut);
      void switchThreadHarness(thread.id, group.harness.id, account).then((switched) => {
        if (switched) void setConfigOption(thread.id, "model", value);
      });
    }
    turnPage("effort");
  };

  const label = pillLabel(model, effortReason ? null : effort, compact);
  // The pill names the login whenever the harness has named accounts, even
  // when there is one: which subscription a thread spends is worth a word.
  // A thread from before accounts says which login it is really on.
  const accountLabel = accountName ?? (!isNew && !account && accounts.length ? "Original login" : undefined);

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
      // One width for both views: the effort card is small, and the model list lives in the same
      // box, so swapping between them never resizes the card.
      className="w-[min(300px,calc(100vw-32px))] p-0"
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
          {accountLabel && (
            // The account is a hint, not a second model label: capped and ellipsized so a long
            // login never takes the width the model's own name needs.
            <span className="min-w-0 max-w-[72px] truncate text-caption text-fg-faint @max-lg/composer:hidden">
              {accountLabel}
            </span>
          )}
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
            model={model}
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
