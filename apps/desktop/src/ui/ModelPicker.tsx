// The composer's model pill and the effort pill beside it. The model pill opens a narrow card: a
// rail of harness icons on the left (the current harness first) and, beside it, a search over the
// browsed harness's models with the current one checked; typing searches every harness. The card
// is as tall as the longest list it can show, up to the room the window has, and only the list
// scrolls. The effort pill opens a small menu of the running model's levels, Fast mode and the
// harness's logins. What the thread's harness cannot do is greyed out with the reason, from
// capability flags.

import { Check, ChevronDown, ChevronUp, Lock, RotateCcw, UserRound, Zap } from "lucide-react";
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
import { CardMenuItem, CardPopover, menuKeys } from "./cardPopover";
import { HarnessIcon } from "./HarnessIcon";
import { cx, MenuSearchField, menuLabel, menuSeparator, Skeleton, Switch, Tooltip } from "./primitives";

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
 * The card's ceiling: as tall as the longest list it shows, and never past the room the window has
 * under the pill. 360 keeps it as tall as it is wide; the 16px is the panel's own padding, which
 * the window's room already counts.
 */
const CARD_MAX = "min(360px, calc(var(--radix-popper-available-height, 360px) - 16px))";

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
  const levels = orderEfforts(model?.efforts ?? []);
  const next = clampEffort(model, levels, sameHarness ? effort : null, remembered);
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
 * What the effort pill reads: the level the model runs at, else the login the thread runs on, else
 * Fast mode, else nothing, and then there is no pill. A harness that sets its own effort names no
 * level.
 */
export function traitsLabel({
  levels,
  effort,
  effortReason,
  accounts,
  fast,
}: {
  levels: string[];
  effort?: string | null;
  effortReason?: string;
  accounts: AccountChip[];
  fast: boolean;
}): string | null {
  if (!effortReason && levels.length > 0 && effort) return effortLabel(effort);
  if (accounts.length) return accounts.find((chip) => chip.selected)?.name ?? "Account";
  return fast ? "Fast mode" : null;
}

/** A model row's height, the search row's and the list's padding, as the card draws them. */
const ROW_PX = 28;
const LIST_CHROME_PX = 48;
/** A rail button (`size-9`) and the gap under it, and the rail's own padding. */
const RAIL_ITEM_PX = 40;
const RAIL_CHROME_PX = 8;

/**
 * The card's floor: as tall as the longest harness's list (or the rail, when it is longer), so
 * moving along the rail never makes the card jump. `CARD_MAX` still caps it to the window's room.
 */
export function panelHeight(groups: { models: unknown[] }[]): number {
  const rows = Math.max(1, ...groups.map((group) => group.models.length));
  return Math.max(LIST_CHROME_PX + rows * ROW_PX, RAIL_CHROME_PX + groups.length * RAIL_ITEM_PX);
}

/**
 * The effort pill's menu, for the model the thread runs: its levels from lowest to highest with the
 * default marked, a reset, Fast mode when the model has it, and the harness's logins while it has
 * more than one. A harness without an effort control says why in the levels' place.
 */
function EffortMenu({
  label,
  levels,
  effort,
  defaultLevel,
  effortReason,
  fast,
  fastOn,
  accounts,
  accountsDisabled,
  onEffort,
  onFast,
  onPickAccount,
}: {
  label: string;
  levels: string[];
  effort?: string | null;
  defaultLevel: string | null;
  effortReason?: string;
  fast: boolean;
  fastOn: boolean;
  accounts: AccountChip[];
  accountsDisabled: boolean;
  onEffort: (effort: string) => void;
  onFast: (on: boolean) => void;
  onPickAccount: (id: string | null) => void;
}) {
  const showLevels = !effortReason && levels.length > 0;
  return (
    <CardPopover
      label="Effort and account"
      onKeyDown={menuKeys}
      className="flex w-[220px] flex-col"
      trigger={
        <button
          type="button"
          aria-label="Effort and account"
          className="flex h-[30px] min-w-0 max-w-[140px] items-center gap-1 rounded-full px-2 text-callout text-fg-muted hover:bg-fill hover:text-fg data-[state=open]:bg-fill-strong data-[state=open]:text-fg"
        >
          {fastOn && <Zap size={11} className="shrink-0 text-fg" fill="currentColor" />}
          <span className="min-w-0 truncate">{label}</span>
          <ChevronDown size={11} strokeWidth={2.5} className="shrink-0 text-fg-faint" />
        </button>
      }
    >
      {(showLevels || effortReason) && <div className={menuLabel}>Effort</div>}
      {effortReason && (
        <p data-menu-static className="flex items-start gap-1.5 px-2 pb-1 text-caption text-fg-faint">
          <Lock size={10} className="mt-[2px] shrink-0" />
          <span>{effortReason}</span>
        </p>
      )}
      {showLevels && (
        <>
          {levels.map((level) => (
            <CardMenuItem
              key={level}
              role="menuitemradio"
              aria-checked={level === effort}
              onSelect={() => onEffort(level)}
            >
              <span className="min-w-0 flex-1 truncate">{effortLabel(level)}</span>
              {level === defaultLevel && <span className="shrink-0 text-caption text-fg-faint">Default</span>}
              <Check
                size={13}
                strokeWidth={2.5}
                className={cx("shrink-0 text-accent", level !== effort && "invisible")}
              />
            </CardMenuItem>
          ))}
          <CardMenuItem
            disabled={!defaultLevel || defaultLevel === effort}
            onSelect={() => defaultLevel && onEffort(defaultLevel)}
            className="text-fg-muted disabled:cursor-default disabled:opacity-40"
          >
            <RotateCcw size={12} className="shrink-0" />
            Reset to default
          </CardMenuItem>
        </>
      )}
      {fast && (
        <div data-menu-static className="flex min-h-7 items-center gap-2 px-2 py-1 text-body">
          <Zap size={13} className="shrink-0 text-fg-muted" />
          <span className="min-w-0 flex-1 truncate">Fast mode</span>
          <Switch checked={fastOn} onChange={onFast} label="Fast mode" />
        </div>
      )}
      {accounts.length > 0 && (
        <>
          {(showLevels || effortReason || fast) && <div className={menuSeparator} />}
          <div className={cx(menuLabel, "flex items-baseline gap-1.5")}>
            <span>Account</span>
            {accountsDisabled && <span className="ms-auto">Stop the turn to switch</span>}
          </div>
          {accounts.map((chip) => (
            <CardMenuItem
              key={chip.id ?? chip.name}
              role="menuitemradio"
              aria-checked={chip.selected}
              disabled={accountsDisabled || chip.gone || chip.signedOut}
              title={chip.signedOut ? "This account is not signed in" : undefined}
              aria-label={chip.signedOut ? `${chip.name} (not signed in)` : undefined}
              onSelect={() => {
                if (chip.id !== undefined) onPickAccount(chip.id);
              }}
              className={cx(
                "disabled:cursor-default disabled:opacity-40",
                chip.signedOut && !chip.selected && "text-fg-faint",
                chip.gone && "italic",
              )}
            >
              <UserRound size={13} strokeWidth={1.75} className="shrink-0 text-fg-muted" />
              <span className="min-w-0 flex-1 truncate">{chip.name}</span>
              {chip.signedOut && <span className="shrink-0 text-caption text-fg-faint">Signed out</span>}
              <Check
                size={13}
                strokeWidth={2.5}
                className={cx("shrink-0 text-accent", !chip.selected && "invisible")}
              />
            </CardMenuItem>
          ))}
        </>
      )}
    </CardPopover>
  );
}

/**
 * The card: a rail of harness icons (each names its harness on hover, the browsed one filled and
 * marked) and beside it a search that opens focused over that harness's models, the current model
 * checked; typing searches every harness. ↑↓ move the highlight (the current model until the user
 * moves), ⏎ picks it, ← at the start of the search moves to the rail, where ↑↓ move between
 * harnesses and → returns to the search.
 */
function ModelPanel({
  groups,
  current,
  onPick,
}: {
  groups: ModelGroup[];
  current?: string | null;
  onPick: (group: ModelGroup, value: string) => void;
}) {
  const [filter, setFilter] = useState("");
  // The harness the user is browsing, until a switch makes another one current.
  const [browsed, setBrowsed] = useState<string | null>(null);
  // The row the user moved to, named rather than numbered: the rows change under it when the
  // provider, the filter or the current harness changes.
  const [picked, setPicked] = useState<{ harness: string; model: string } | null>(null);
  const field = useRef<HTMLInputElement>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const rail = useRef<HTMLDivElement>(null);
  const searching = !!filter.trim();
  const provider = groups.some((g) => g.harness.id === browsed)
    ? (browsed as string)
    : (groups[0]?.harness.id ?? "");
  const rows = useMemo(() => filterModelRows(groups, provider, filter), [groups, provider, filter]);
  // Until the user moves, the list highlights the model the picker's owner runs; a highlight the
  // current rows no longer hold (another harness browsed) falls back to the first row.
  const marker = picked ?? (current ? { harness: groups[0]?.harness.id ?? "", model: current } : null);
  const found = marker
    ? rows.findIndex((r) => r.group.harness.id === marker.harness && r.model.value === marker.model)
    : -1;
  const active = Math.max(found, 0);
  const row = rows[active];
  useEffect(() => {
    field.current?.focus();
  }, []);
  useEffect(() => {
    scroller.current?.querySelector(`[id="model-row-${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);
  const highlight = (at: number) => {
    const next = rows[at];
    if (next) setPicked({ harness: next.group.harness.id, model: next.model.value });
  };
  const browse = (id: string) => {
    setBrowsed(id);
    setFilter("");
    setPicked(null);
  };
  // ← leaves the search for the rail only from the caret's first place, so it still edits text.
  const caretAtStart = () => {
    const input = field.current;
    if (!input || document.activeElement !== input) return true;
    return input.selectionStart === 0 && input.selectionEnd === 0;
  };
  const railButtons = () => [...(rail.current?.querySelectorAll<HTMLButtonElement>("button") ?? [])];
  return (
    <div className="flex min-h-0 flex-1" style={{ minHeight: `min(${panelHeight(groups)}px, ${CARD_MAX})` }}>
      <div
        ref={rail}
        role="toolbar"
        aria-orientation="vertical"
        aria-label="Providers"
        className="flex w-11 shrink-0 flex-col gap-1 overflow-y-auto border-border border-r p-1 [scrollbar-width:none]"
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            const buttons = railButtons();
            const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
            buttons[movePickerIndex(at, e.key === "ArrowDown" ? 1 : -1, buttons.length)]?.focus();
          } else if (e.key === "ArrowRight") {
            e.preventDefault();
            field.current?.focus();
          }
        }}
      >
        {groups.map((g) => {
          const selected = provider === g.harness.id && !searching;
          return (
            <Tooltip
              key={g.harness.id}
              label={g.blocked ? `${g.harness.name}: ${g.blocked}` : g.harness.name}
              side="left"
            >
              <button
                type="button"
                aria-label={g.harness.name}
                aria-pressed={selected}
                onClick={() => browse(g.harness.id)}
                className={cx(
                  "relative flex size-9 shrink-0 items-center justify-center rounded-lg hover:bg-fill focus-visible:bg-fill",
                  selected && "bg-fill",
                  g.blocked && "opacity-45",
                )}
              >
                <HarnessIcon id={g.harness.id} name={g.harness.name} size={18} />
                {selected && (
                  <span aria-hidden className="absolute inset-y-2 -right-1 w-0.5 rounded-full bg-accent" />
                )}
              </button>
            </Tooltip>
          );
        })}
      </div>
      <div
        className="flex min-h-0 min-w-0 flex-1 flex-col"
        onKeyDown={(e) => {
          if (e.key === "ArrowLeft") {
            if (!caretAtStart()) return;
            e.preventDefault();
            const buttons = railButtons();
            (buttons.find((b) => b.getAttribute("aria-pressed") === "true") ?? buttons[0])?.focus();
          } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            highlight(movePickerIndex(active, e.key === "ArrowDown" ? 1 : -1, rows.length));
          } else if (e.key === "Enter") {
            if (row && !row.group.blocked) {
              e.preventDefault();
              onPick(row.group, row.model.value);
            }
          }
        }}
      >
        <div className="p-1">
          <MenuSearchField
            value={filter}
            onChange={setFilter}
            placeholder="Search models"
            label="Search models"
            inputRef={field}
            controls="model-list"
            activeDescendant={rows[active] ? `model-row-${active}` : undefined}
          />
        </div>
        <div
          ref={scroller}
          id="model-list"
          role="listbox"
          aria-label="Models"
          className="scroll-stable min-h-0 min-w-0 flex-1 p-1"
        >
          {rows.map((item, i) => (
            <div
              key={`${item.group.harness.id}:${item.model.value}`}
              id={`model-row-${i}`}
              role="option"
              tabIndex={-1}
              aria-selected={item.group.harness.id === groups[0]?.harness.id && item.model.value === current}
              aria-disabled={!!item.group.blocked}
              onMouseEnter={() => highlight(i)}
              onClick={() => !item.group.blocked && onPick(item.group, item.model.value)}
              className={cx(
                "flex cursor-pointer items-center gap-2 rounded-md px-2 py-1",
                i === active && "bg-fill",
                item.group.blocked && "opacity-45",
              )}
            >
              {searching && (
                <HarnessIcon id={item.group.harness.id} name={item.group.harness.name} size={14} />
              )}
              <span className="min-w-0 flex-1 truncate text-body">{item.model.name || item.model.value}</span>
              {item.group.harness.id === groups[0]?.harness.id && item.model.value === current && (
                <Check size={13} className="shrink-0 text-accent" />
              )}
            </div>
          ))}
          {!rows.length && (
            <div className="px-2 py-3 text-center text-callout text-fg-muted">
              {searching
                ? "No matching models"
                : (groups.find((g) => g.harness.id === provider)?.blocked ?? "No models listed")}
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

/** The model pill's name: the model's, clipped in a compact composer. The effort has its own pill. */
export function pillLabel(model?: string | null, compact = false): string {
  const name = model || "Default model";
  return compact && name.length > 14 ? `${clip(name, 13)}…` : name;
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

  // The harnesses worth listing: RustCode turned off in Settings › Harnesses leaves the list, and a
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
   * switched to with its remembered login, as its model row always did. The
   * card closes on the pick; the effort pill beside it then names the level.
   */
  const pickModel = (group: ModelGroup, value: string) => {
    if (group.blocked) return;
    setOpen(false);
    // The level the model was last left on, so picking it again returns to that level; a level
    // its list no longer offers is left to the model's default (`clampEffort`).
    const entry = group.harness.models.find((m) => m.id === value);
    const remembered = rememberedEffort(effortMemory(), group.harness.id, value);
    const levels = orderEfforts(entry?.efforts ?? []);
    const same = group.harness.id === providerId;
    if (same) {
      if (thread) void writeModelPick(thread.id, modelThreadWrites(entry, value, effort, remembered, true));
      else {
        // A harness that lists no levels for the model keeps the effort it had.
        const next = clampEffort(entry, levels, effort, remembered);
        setChoice(levels.length ? { model: value, effort: next } : { model: value });
      }
    } else if (isNew) {
      // Another harness's model brings the level it was last left on, never this harness's
      // effort: the choice normalizes it, and its remembered account goes with it.
      setChoice({
        harness: group.harness.id,
        model: value,
        effort: clampEffort(entry, levels, null, remembered),
      });
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
  };

  // A catalog name is shown as it is; a bare id reads without its `provider/` prefix.
  const catalogName = modelEntry?.name || models.find((item) => item.value === model)?.name;
  const displayName = catalogName && catalogName !== model ? catalogName : modelName(model ?? undefined);
  const label = pillLabel(displayName, compact);
  // Why the current harness's model is not this card's to change (one that picks its own).
  const modelReason = controlDisabledReason("model", caps, harnessName);
  const traits = traitsLabel({
    levels: efforts,
    effort,
    effortReason,
    accounts: accountRows,
    fast: !!fast,
  });

  // Nothing to show until the engine lists harnesses (or the thread its options): a placeholder pill.
  if (!harnesses && !thread?.configOptions.length) {
    return (
      <span role="status" aria-label="Loading models" className="flex h-[30px] items-center">
        <Skeleton className="h-[30px] w-[132px] rounded-full" />
      </span>
    );
  }

  return (
    <div className="flex min-w-0 items-center gap-0.5">
      <CardPopover
        open={open}
        onOpenChange={setOpen}
        role="dialog"
        label="Model"
        // The card focuses its own control (the search field, which the panel does on mount) so
        // the arrow keys act on what the card is for the moment it opens.
        onOpenAutoFocus={(event) => event.preventDefault()}
        // As tall as the longest list up to the room the window has: the model list is the one
        // part that scrolls.
        className="flex w-[min(360px,calc(100vw-32px))] flex-col overflow-hidden p-0"
        style={{ maxHeight: CARD_MAX }}
        trigger={
          <button
            type="button"
            aria-label="Model"
            className="flex h-[30px] min-w-0 max-w-[220px] items-center gap-1.5 rounded-full px-2 text-callout text-fg hover:bg-fill data-[state=open]:bg-fill-strong @max-lg/composer:max-w-[160px]"
          >
            <HarnessIcon id={harness?.id ?? "rustcode"} name={harness?.name} size={14} />
            <span className="min-w-0 truncate">{label}</span>
            <ChevronUp
              size={11}
              strokeWidth={2.5}
              className={cx("shrink-0 transition-transform duration-200 ease-snappy", open && "rotate-180")}
            />
          </button>
        }
      >
        <ModelPanel key={providerId} groups={groups} current={model} onPick={pickModel} />
        {modelReason && (
          <p className="flex shrink-0 items-start gap-1.5 border-border border-t px-3 py-2 text-caption text-fg-faint">
            <Lock size={10} className="mt-[2px] shrink-0" />
            <span>{modelReason}</span>
          </p>
        )}
      </CardPopover>
      {traits && (
        <EffortMenu
          label={traits}
          levels={efforts}
          effort={effort}
          defaultLevel={defaultLevel}
          effortReason={effortReason}
          fast={!!fast}
          fastOn={!!fast?.currentBool}
          accounts={accountRows}
          accountsDisabled={accountBlocked || (!!harness && !!unavailableReason(harness))}
          onEffort={pickEffort}
          onFast={(on) => thread && void setConfigOption(thread.id, "fast", on)}
          onPickAccount={pickAccount}
        />
      )}
    </div>
  );
}
