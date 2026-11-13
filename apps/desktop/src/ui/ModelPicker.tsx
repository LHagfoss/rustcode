// The composer's model pill: the model and, beside its name, its effort. It opens one card for
// everything the pill says: a rail of harness icons on the left (the current harness first), the
// browsed harness's models in the middle (a search over them, the current one checked; typing
// searches every harness), and on the right the current model's effort levels, Fast mode and the
// harness's logins with what is left of each plan. Nothing switches between a model step and an
// effort step: picking a model keeps the card open and moves the keyboard to its levels. The card
// is as tall as the longest list it can show, up to the room the window has, and only the lists
// scroll. What the thread's harness cannot do is greyed out with the reason, from capability flags.

import { Check, ChevronUp, Lock, RotateCcw, UserRound, Zap } from "lucide-react";
import { type RefObject, useEffect, useMemo, useRef, useState } from "react";
import { harnessPlanKey, leftPercent, shownWindow } from "../acp/rustcode";
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
  type CachedPlan,
  effortMemory,
  ensureHarnessModels,
  harnessesOn,
  lastAccountFor,
  loadMachineHarnesses,
  type NewThreadChoice,
  newThreadChoice,
  pickNewThreadAccount,
  pickNewThreadChoice,
  setConfigOption,
  shownPlan,
  switchThreadAccount,
  switchThreadHarness,
  useApp,
} from "../state/store";
import type { ThreadState } from "../state/thread";
import { splitThreadKey } from "../state/threadKeys";
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

/** The reason note under a list that cannot be changed: its 14px line, 16px of padding, a 1px rule. */
const NOTE_PX = 31;

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
  /** What is left of the login's shortest plan window, in percent, when a plan read has it. */
  left?: number | null;
}

/**
 * What is left of a login's plan, in percent: its shortest window, read from the plan cache the
 * composer's meter reads. Null when no plan was read for it (or the login is gone).
 */
export function accountLeft(
  cache: Record<string, CachedPlan>,
  harness: string,
  account: string | null | undefined,
): number | null {
  if (account === undefined) return null;
  const { plan } = shownPlan(undefined, cache[harnessPlanKey(harness, account)]);
  const window = plan ? shownWindow(plan) : undefined;
  return window ? leftPercent(window) : null;
}

/** A harness's logins, only when there is a choice to make between them. */
export function shownAccounts(chips: AccountChip[]): AccountChip[] {
  return chips.length > 1 ? chips : [];
}

/** Whether a chip can be pressed: not while a turn runs, not a login that is gone or signed out. */
function chipPressable(chip: AccountChip, disabled: boolean): boolean {
  return !disabled && !chip.gone && !chip.signedOut;
}

/**
 * A model row's own name: the catalog's when it has one, else its id. Whitespace is collapsed, so
 * two listings of one name compare as one.
 */
export function modelDisplayName(model: Choice): string {
  const name = (model.name || model.value).trim().replace(/\s+/g, " ");
  return name || model.value;
}

/**
 * The provider an id names, when it carries one: `anthropic/claude-sonnet` is Anthropic's, and
 * `openrouter/anthropic/claude-sonnet` is the same model through OpenRouter, a different one to
 * pick. Null for an id without a `provider/` prefix, which is one harness's own list.
 */
export function modelProviderOf(value: string): string | null {
  const at = value.lastIndexOf("/");
  return at > 0 ? value.slice(0, at) : null;
}

/**
 * One row per model. A harness can list the same model twice: one id twice (a cached listing merged
 * with a fresh one), or two ids under one name (an alias beside the dated id it names). The harness's
 * own order is kept, so the model it puts first stays first; of two rows sharing a name the one it
 * recommends (`recommended`, the model the harness runs or was last left on) wins, else the first it
 * listed. Ids are matched exactly, names case- and space-insensitively.
 *
 * A name only holds within one provider: two providers can each list a `Claude Sonnet`
 * (`anthropic/…` and `openrouter/anthropic/…`), and hiding either would lose a model the user came
 * to pick. Such a pair stays two rows, which `repeatedModelNames` marks so the row can say which
 * provider it comes from.
 */
export function dedupeModels(models: Choice[], recommended?: string | null): Choice[] {
  const seen = new Set<string>();
  const byName = new Map<string, number>();
  const rows: Choice[] = [];
  for (const model of models) {
    if (seen.has(model.value)) continue;
    seen.add(model.value);
    const key = `${modelProviderOf(model.value) ?? ""}\u0000${modelDisplayName(model).toLowerCase()}`;
    const at = byName.get(key);
    if (at === undefined) {
      byName.set(key, rows.length);
      rows.push(model);
    } else if (recommended && model.value === recommended && rows[at]!.value !== recommended) {
      // The name keeps the harness's place for it; the recommended id takes that place.
      rows[at] = model;
    }
  }
  return rows;
}

/**
 * The names one harness's list holds more than once, so a row among them can name its provider
 * instead of reading as its neighbour.
 */
export function repeatedModelNames(models: Choice[]): Set<string> {
  const counts = new Map<string, number>();
  for (const model of models) {
    const key = modelDisplayName(model).toLowerCase();
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  return new Set([...counts].filter(([, count]) => count > 1).map(([name]) => name));
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
/** What the right column needs to show a model's levels and its logins without scrolling much. */
const COLUMN_PX = 264;
/** A rail button (`size-9`) and the gap under it, and the rail's own padding. */
const RAIL_ITEM_PX = 40;
const RAIL_CHROME_PX = 8;

/**
 * The card's floor: as tall as the longest harness's list (or the rail, when it is longer, or the
 * effort column), so moving along the rail never makes the card jump. `CARD_MAX` still caps it to
 * the window's room.
 */
export function panelHeight(groups: { models: unknown[] }[]): number {
  const rows = Math.max(1, ...groups.map((group) => group.models.length));
  return Math.max(LIST_CHROME_PX + rows * ROW_PX, RAIL_CHROME_PX + groups.length * RAIL_ITEM_PX, COLUMN_PX);
}

/**
 * The card's right column: the current model's effort levels from lowest to highest with the
 * default marked, a reset, Fast mode when the model has it, and the harness's logins with what is
 * left of each one's plan. It sits beside the model list, so the model, its effort and its account
 * are chosen in one card with no step between them. A harness without an effort control says why in
 * the levels' place. ↑↓ walk its rows, ← returns to the search, and a printed character goes back to
 * the search to filter (the card's `onKeyDown`).
 */
function TraitsColumn({
  model,
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
  onSearch,
  focusAsk,
  askTick,
}: {
  model: string;
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
  /** Hands the keyboard back to the model search. */
  onSearch: () => void;
  /** Set while a model was just picked: focus follows the checked level as it settles. */
  focusAsk: RefObject<boolean>;
  /** Changes with every model pick, so picking the model already run asks again. */
  askTick: number;
}) {
  const column = useRef<HTMLDivElement>(null);
  const showLevels = !effortReason && levels.length > 0;
  const choosable = accounts.length > 1;
  const empty = !showLevels && !effortReason && !fast && accounts.length === 0;
  // The card hands the keyboard to the column's checked row (or its first) after a model pick, and
  // again when the level settles a render later (a model brings the level it was last left on).
  // biome-ignore lint/correctness/useExhaustiveDependencies: refocus when the model or level changes
  useEffect(() => {
    if (!focusAsk.current) return;
    const root = column.current;
    const row =
      root?.querySelector<HTMLElement>("[data-menu-row][aria-checked='true']:not([disabled])") ??
      root?.querySelector<HTMLElement>("[data-menu-row]:not([disabled])");
    row?.focus({ preventScroll: true });
  }, [model, effort, askTick]);
  return (
    <div
      ref={column}
      role="group"
      aria-label={`Effort and account for ${model}`}
      data-traits=""
      onKeyDown={(event) => {
        if (event.altKey || event.ctrlKey || event.metaKey) return;
        if (event.key === "ArrowLeft") {
          event.preventDefault();
          onSearch();
        } else if (event.key.length === 1 && event.key !== " ") {
          // Type to filter: the character lands in the search the focus moves to.
          onSearch();
        } else menuKeys(event);
      }}
      className="flex w-[208px] shrink-0 flex-col border-border border-s bg-fill-subtle max-sm:w-[160px]"
    >
      <div className="scroll-stable min-h-0 flex-1 p-1">
        {(showLevels || effortReason) && (
          <div className={cx(menuLabel, "flex items-baseline gap-1.5")}>
            <span>Effort</span>
            <span className="min-w-0 truncate text-fg-muted">{model}</span>
          </div>
        )}
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
                close={false}
                onSelect={() => onEffort(level)}
              >
                <span className="min-w-0 flex-1 truncate">{effortLabel(level)}</span>
                {level === defaultLevel && (
                  <span className="shrink-0 text-caption text-fg-faint">Default</span>
                )}
                <Check
                  size={13}
                  strokeWidth={2.5}
                  className={cx("shrink-0 text-accent", level !== effort && "invisible")}
                />
              </CardMenuItem>
            ))}
            <CardMenuItem
              close={false}
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
              {accountsDisabled && choosable && (
                <span className="ms-auto min-w-0 truncate">Stop the turn to switch</span>
              )}
            </div>
            {accounts.map((chip) => {
              const pressable = choosable && chipPressable(chip, accountsDisabled);
              const row = (
                <>
                  <UserRound size={12} strokeWidth={1.75} className="shrink-0 text-fg-muted" />
                  <span title={chip.name} className="min-w-0 flex-1 truncate">
                    {chip.name}
                  </span>
                  {chip.signedOut ? (
                    <span className="shrink-0 text-caption text-fg-faint">Signed out</span>
                  ) : (
                    chip.left != null && (
                      <span
                        className={cx(
                          "shrink-0 text-caption tabular",
                          chip.left < 20 ? "text-warning" : "text-fg-faint",
                        )}
                      >
                        {chip.left}% left
                      </span>
                    )
                  )}
                  {choosable && (
                    <Check
                      size={13}
                      strokeWidth={2.5}
                      className={cx("shrink-0 text-accent", !chip.selected && "invisible")}
                    />
                  )}
                </>
              );
              // One login is a line of the card, not a choice to make.
              if (!choosable)
                return (
                  <div
                    key={chip.id ?? chip.name}
                    data-menu-static
                    className="flex min-h-7 items-center gap-2 px-2 py-1 text-body"
                  >
                    {row}
                  </div>
                );
              return (
                <CardMenuItem
                  key={chip.id ?? chip.name}
                  role="menuitemradio"
                  aria-checked={chip.selected}
                  aria-label={chip.signedOut ? `${chip.name} (not signed in)` : undefined}
                  close={false}
                  disabled={!pressable}
                  onSelect={() => {
                    if (chip.id !== undefined) onPickAccount(chip.id);
                  }}
                  className={cx(
                    "disabled:cursor-default",
                    !pressable && !chip.selected && "opacity-40",
                    chip.signedOut && "text-fg-faint",
                    chip.gone && "italic",
                  )}
                >
                  {row}
                </CardMenuItem>
              );
            })}
          </>
        )}
        {empty && (
          <p data-menu-static className="px-2 py-1.5 text-caption text-fg-faint">
            Nothing to set for this model.
          </p>
        )}
      </div>
    </div>
  );
}

/**
 * The card: a rail of harness icons (each names its harness on hover, the browsed one filled and
 * marked) and beside it a search that opens focused over that harness's models, the current model
 * checked; typing searches every harness. ↑↓ move the highlight (the current model until the user
 * moves), ⏎ picks it, ← at the start of the search moves to the rail, where ↑↓ move between
 * harnesses and → returns to the search; → at the end of the search moves to the effort column.
 */
function ModelPanel({
  groups,
  current,
  onPick,
  onColumn,
  keepFocus,
  onSearchFocus,
  fieldRef,
  maxHeight = CARD_MAX,
}: {
  groups: ModelGroup[];
  current?: string | null;
  onPick: (group: ModelGroup, value: string) => void;
  /** Moves the keyboard to the effort column; absent when the card has none. */
  onColumn?: () => void;
  /** A model was just picked: the card's focus is in the effort column and stays there. */
  keepFocus: RefObject<boolean>;
  /** The search took the keyboard: nothing is waiting for the effort column any more. */
  onSearchFocus: () => void;
  /** The search field, so the effort column can hand the keyboard back to it. */
  fieldRef: RefObject<HTMLInputElement | null>;
  /** The card's ceiling: less the note under it when the list has one. */
  maxHeight?: string;
}) {
  const [filter, setFilter] = useState("");
  // The harness the user is browsing, until a switch makes another one current.
  const [browsed, setBrowsed] = useState<string | null>(null);
  // The row the user moved to, named rather than numbered: the rows change under it when the
  // provider, the filter or the current harness changes.
  const [picked, setPicked] = useState<{ harness: string; model: string } | null>(null);
  const field = fieldRef;
  const scroller = useRef<HTMLDivElement>(null);
  const rail = useRef<HTMLDivElement>(null);
  const searching = !!filter.trim();
  const provider = groups.some((g) => g.harness.id === browsed)
    ? (browsed as string)
    : (groups[0]?.harness.id ?? "");
  const rows = useMemo(() => filterModelRows(groups, provider, filter), [groups, provider, filter]);
  // A name two providers share: its rows say which provider they come from.
  const shared = useMemo(
    () => new Map(groups.map((group) => [group.harness.id, repeatedModelNames(group.models)])),
    [groups],
  );
  // Until the user moves, the list highlights the model the picker's owner runs; a highlight the
  // current rows no longer hold (another harness browsed) falls back to the first row.
  const marker = picked ?? (current ? { harness: groups[0]?.harness.id ?? "", model: current } : null);
  const found = marker
    ? rows.findIndex((r) => r.group.harness.id === marker.harness && r.model.value === marker.model)
    : -1;
  const active = Math.max(found, 0);
  const row = rows[active];
  // The card opens with the search focused, unless a model pick just remounted the list (another
  // harness's model) and the keyboard belongs to the levels it brought.
  // biome-ignore lint/correctness/useExhaustiveDependencies: on mount only
  useEffect(() => {
    if (!keepFocus.current) field.current?.focus();
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
    <div
      className="flex min-h-0 min-w-0 flex-1 flex-col"
      style={{ minHeight: `min(${panelHeight(groups)}px, ${maxHeight})` }}
    >
      <div className="flex min-h-0 flex-1">
        <div
          ref={rail}
          role="toolbar"
          aria-orientation="vertical"
          aria-label="Providers"
          className="flex w-11 shrink-0 flex-col gap-1 overflow-y-auto p-1 [scrollbar-width:none]"
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
          onFocus={(e) => {
            if (e.target === field.current) onSearchFocus();
          }}
          onKeyDown={(e) => {
            if (e.key === "ArrowLeft") {
              if (!caretAtStart()) return;
              e.preventDefault();
              const buttons = railButtons();
              (buttons.find((b) => b.getAttribute("aria-pressed") === "true") ?? buttons[0])?.focus();
            } else if (e.key === "ArrowRight") {
              // → from the end of the typed text leaves the list for the effort column.
              const input = field.current;
              const atEnd =
                !input ||
                document.activeElement !== input ||
                (input.selectionStart === input.value.length && input.selectionEnd === input.value.length);
              if (onColumn && atEnd) {
                e.preventDefault();
                onColumn();
              }
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
            {rows.map((item, i) => {
              const name = modelDisplayName(item.model);
              // A name the list holds twice is only told apart by the provider behind it.
              const providerName = shared.get(item.group.harness.id)?.has(name.toLowerCase())
                ? modelProviderOf(item.model.value)
                : null;
              return (
                <div
                  key={`${item.group.harness.id}:${item.model.value}`}
                  id={`model-row-${i}`}
                  role="option"
                  tabIndex={-1}
                  aria-selected={
                    item.group.harness.id === groups[0]?.harness.id && item.model.value === current
                  }
                  aria-disabled={!!item.group.blocked}
                  onMouseEnter={() => highlight(i)}
                  onClick={() => !item.group.blocked && onPick(item.group, item.model.value)}
                  className={cx(
                    "flex min-h-7 cursor-pointer items-center gap-2 rounded-xl px-2 py-1",
                    i === active && "bg-fill",
                    item.group.blocked && "opacity-45",
                  )}
                >
                  {searching && (
                    <HarnessIcon id={item.group.harness.id} name={item.group.harness.name} size={14} />
                  )}
                  <span className="min-w-0 flex-1 truncate text-body">{name}</span>
                  {providerName && (
                    <span className="shrink-0 text-caption text-fg-faint">{providerName}</span>
                  )}
                  {item.group.harness.id === groups[0]?.harness.id && item.model.value === current && (
                    <Check size={13} className="shrink-0 text-accent" />
                  )}
                </div>
              );
            })}
            {!rows.length &&
              (searching ? (
                <div role="presentation" className="px-3 py-6 text-center">
                  <p className="text-callout text-fg">No models match “{filter.trim()}”</p>
                  <p className="mt-1 text-caption text-fg-faint">
                    Search every harness, by name or provider.
                  </p>
                  <button
                    type="button"
                    onClick={() => {
                      setFilter("");
                      // The button goes with the empty list: the field it cleared takes the caret.
                      field.current?.focus();
                    }}
                    className="mt-2 rounded-md px-2 py-1 text-callout text-accent hover:bg-fill focus-visible:bg-fill"
                  >
                    Clear search
                  </button>
                </div>
              ) : (
                <div role="presentation" className="px-2 py-3 text-center text-callout text-fg-muted">
                  {groups.find((g) => g.harness.id === provider)?.blocked ?? "No models listed"}
                </div>
              ))}
          </div>
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
      ? // The id is searched beside the name: on a harness whose models are `provider/model`, the
        // provider is only in the id, and that is what the user types to narrow the list.
        (searching
          ? matchModels(
              filter,
              group.models.map((model) => ({ ...model, id: model.value })),
            ).shown
          : group.models
        ).map((model) => ({ group, model }))
      : [],
  );
}

/** The model pill's name: the model's, clipped in a compact composer. The effort has its own pill. */
export function pillLabel(model?: string | null, compact = false): string {
  const name = model || "Default model";
  return compact && name.length > 14 ? `${clip(name, 13)}…` : name;
}

const NO_CONFIG: ConfigOption[] = [];
const NO_PLANS: Record<string, CachedPlan> = {};

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
  // The computer this picker speaks for: an open thread's own machine, or for a new thread the one
  // **Run on** names. Its harnesses, models and logins are that machine's, read over its connection
  // (docs/desktop.md › Several machines).
  const host = useApp((s) => (thread ? splitThreadKey(thread.id).host : s.newThreadHost));
  const harnesses = useApp((s) => harnessesOn(s, host));
  const choice = useApp((s) => s.newThread);
  // The first open thread's config options: stable while a reply streams, so the picker does not re-render per chunk.
  const knownConfig = useApp((s) => firstConfig(s.threads));
  // A machine's list is read when its picker appears, and kept: only this computer's engine reads
  // its own on connect (state/store.ts `refreshHarnesses`).
  useEffect(() => {
    if (host) void loadMachineHarnesses(host, { report: false });
  }, [host]);
  const [open, setOpenState] = useState(false);
  // Bumped by a model pick so the column re-reads its focus request (a ref alone renders nothing).
  const [columnAsk, setColumnAsk] = useState(0);
  // The search field and the effort column share the keyboard: a model pick hands it to the levels
  // (`focusColumn`), and the search takes it back (← from the column, a printed character, a click).
  const field = useRef<HTMLInputElement | null>(null);
  const columnFocus = useRef(false);
  // Every opening starts on the search.
  const setOpen = (next: boolean) => {
    if (next) columnFocus.current = false;
    setOpenState(next);
  };
  // `/model` asks the picker to open (store.runCommandAction).
  const ask = useApp((s) => s.modelPickerAsk);
  const seenAsk = useRef(ask);
  useEffect(() => {
    if (ask === seenAsk.current) return;
    seenAsk.current = ask;
    columnFocus.current = false;
    setOpenState(true);
  }, [ask]);

  const isNew = !thread;
  // The project's own default belongs to this computer's folders, so a machine's picker never
  // reads it; a pick is still the draft, and wins over it (`state/store.ts` `newThreadChoice`).
  const projectCwd = useApp((s) => (host ? "" : s.projectCwd));
  const effective: NewThreadChoice = newThreadChoice({ newThread: choice, harnesses, projectCwd }) ?? {
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
  // What is left of each login's plan: this computer's plan read, which a paired machine's logins
  // have no part in.
  const planCache = useApp((s) => (host ? NO_PLANS : s.planCache));
  const engineReady = useApp((s) => (host ? s.connectedHosts.includes(host) : s.status === "ready"));

  const { models, efforts, model, effort } = pickerModels(thread, harness, effective, knownConfig);
  // The model's own entry: what its effort levels are, and where the reset returns to.
  const modelEntry = harness?.models.find((m) => m.id === model);
  const defaultLevel = defaultEffort(modelEntry, efforts);
  const fast = thread?.configOptions.find((o) => o.id === "fast" && o.type === "boolean");
  const harnessName = harness?.name ?? "This harness";
  const effortReason = controlDisabledReason("effort", caps, harnessName);

  const setChoice = (next: Partial<NewThreadChoice>) => {
    const merged = { ...effective, model: model ?? null, effort: effort ?? null, ...next };
    pickNewThreadChoice(harnesses ? normalizeDefault(harnesses, merged) : merged, host);
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
      left: accountLeft(planCache, h.id, a.id),
      // A definite `needs_login` is as signed out as a row the plan read saw.
      signedOut: a.auth === "needs_login" || accountSignedOut(h.id, a.id, signedOut),
    });
    // A new thread starts on one of the named logins while the harness has any.
    if (isNew) return named.map((a) => chip(a, a.id === account));
    const own = account;
    const chips: AccountChip[] = [];
    // A thread from before this harness had accounts keeps its own sign-in:
    // it stays readable and is only moved by hand.
    if (!own && named.length)
      chips.push({
        id: null,
        name: "Original login",
        selected: true,
        left: accountLeft(planCache, h.id, null),
      });
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
    const listed = isCurrent ? models : h.models.map((m) => ({ value: m.id, name: m.name }));
    return {
      harness: h,
      blocked: unavailableReason(h) ?? (isCurrent || canSwitch ? undefined : "Stop the turn to switch"),
      // One row per model: the model this thread runs, else the one its last pick was, is the row a
      // name shared by two ids keeps.
      models: dedupeModels(listed, isCurrent ? model : (h.last?.model ?? null)),
    };
  });
  // The current harness's logins: the column lists them all (a single one as a line with what is
  // left of its plan), while the pill names one only when there is a choice between them.
  const allAccounts = harness ? accountChips(harness) : [];
  const accountRows = shownAccounts(allAccounts);

  // An external harness lists its models only when asked: ask for every harness on show once the
  // card opens, so a group never sits empty behind a "no models" line it could have answered.
  const listing = groups
    .filter((g) => !g.blocked && !g.models.length)
    .map((g) => g.harness.id)
    .join();
  useEffect(() => {
    if (!open || !engineReady || !listing) return;
    for (const id of listing.split(",")) void ensureHarnessModels(id, host);
  }, [open, engineReady, listing, host]);

  /**
   * A model row. In the current harness it is set in place and the level the
   * new model does not offer is clamped to its default; another harness is
   * switched to with its remembered login, as its model row always did. A
   * model with levels turns the card to its effort step; one without closes it.
   */
  const pickModel = (group: ModelGroup, value: string) => {
    if (group.blocked) return;
    // The level the model was last left on, so picking it again returns to that level; a level
    // its list no longer offers is left to the model's default (`clampEffort`).
    const entry = group.harness.models.find((m) => m.id === value);
    const remembered = rememberedEffort(effortMemory(), group.harness.id, value);
    const levels = orderEfforts(entry?.efforts ?? []);
    // A model with levels or logins keeps the card open for them; the keyboard follows to its levels.
    if (levels.length) {
      columnFocus.current = true;
      setColumnAsk((n) => n + 1);
    } else setOpen(false);
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

  // The note under a list takes its room from the card's ceiling, never from the list's last row.
  const cardMax = modelReason ? `calc(${CARD_MAX} - ${NOTE_PX}px)` : CARD_MAX;
  const focusSearch = () => field.current?.focus();
  const focusColumn = () => {
    const root = document.querySelector<HTMLElement>("[data-traits]");
    const row =
      root?.querySelector<HTMLElement>("[data-menu-row][aria-checked='true']:not([disabled])") ??
      root?.querySelector<HTMLElement>("[data-menu-row]:not([disabled])");
    row?.focus({ preventScroll: true });
  };
  return (
    <CardPopover
      open={open}
      onOpenChange={setOpen}
      role="dialog"
      label="Model"
      // The card focuses its own control (the search field, which the panel does on mount) so the
      // arrow keys act on what the card is for the moment it opens.
      onOpenAutoFocus={(event) => event.preventDefault()}
      // One card, so Escape closes it from wherever the keyboard is.
      // The card is as wide as its three parts need, and never wider than the window allows; only
      // the lists scroll. Its own ceiling is the full card, note included: the list gives the note
      // its room (`cardMax`), so the two together never pass it.
      className="flex flex-col overflow-hidden! p-0"
      style={{ maxHeight: CARD_MAX }}
      trigger={
        <button
          type="button"
          aria-label="Model"
          className="flex h-[30px] min-w-0 max-w-[280px] items-center gap-1.5 rounded-full px-2 text-callout text-fg hover:bg-fill data-[state=open]:bg-fill-strong @max-lg/composer:max-w-[200px]"
        >
          <HarnessIcon id={harness?.id ?? "rustcode"} name={harness?.name} size={14} />
          <span className="min-w-0 shrink truncate">{label}</span>
          {fast?.currentBool && <Zap size={11} className="shrink-0" fill="currentColor" />}
          {traits && <span className="min-w-0 shrink-[2] truncate text-fg-muted">{traits}</span>}
          <ChevronUp
            size={11}
            strokeWidth={2.5}
            className={cx("shrink-0 transition-transform duration-200 ease-snappy", open && "rotate-180")}
          />
        </button>
      }
    >
      <div
        style={{ maxHeight: CARD_MAX }}
        className="flex min-h-0 w-[min(560px,calc(100vw-32px))] flex-col overflow-hidden"
      >
        <div className="flex min-h-0 flex-1">
          <ModelPanel
            key={providerId}
            groups={groups}
            current={model}
            onPick={pickModel}
            onColumn={focusColumn}
            keepFocus={columnFocus}
            onSearchFocus={() => {
              columnFocus.current = false;
            }}
            fieldRef={field}
            maxHeight={cardMax}
          />
          <TraitsColumn
            model={displayName ?? label}
            levels={efforts}
            effort={effort}
            defaultLevel={defaultLevel}
            effortReason={effortReason}
            fast={!!fast}
            fastOn={!!fast?.currentBool}
            accounts={allAccounts}
            accountsDisabled={accountBlocked || (!!harness && !!unavailableReason(harness))}
            onEffort={pickEffort}
            onFast={(on) => thread && void setConfigOption(thread.id, "fast", on)}
            onPickAccount={pickAccount}
            onSearch={focusSearch}
            focusAsk={columnFocus}
            askTick={columnAsk}
          />
        </div>
        {modelReason && (
          <p className="flex shrink-0 items-start gap-1.5 border-border border-t px-3 py-2 text-caption text-fg-faint">
            <Lock size={10} className="mt-[2px] shrink-0" />
            <span>{modelReason}</span>
          </p>
        )}
      </div>
    </CardPopover>
  );
}
