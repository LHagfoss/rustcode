// The composer's model pill: the model and, beside its name, its effort. It opens a narrow card: a
// rail of harness icons on the left (the current harness first) and, beside it, a search over the
// browsed harness's models with the current one checked; typing searches every harness. The card
// is as tall as the longest list it can show, up to the room the window has, and only the list
// scrolls. Picking a model turns the card to its second step: the model's levels, Fast mode and
// the harness's logins (a row at the foot of the list goes there too). What the thread's harness
// cannot do is greyed out with the reason, from capability flags. The two steps are different
// sizes (the list is wider than the effort menu), so the card measures the step it shows and
// glides between the two, anchored to the pill.

import { Check, ChevronLeft, ChevronRight, ChevronUp, Lock, RotateCcw, UserRound, Zap } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
import { observeSize } from "../lib/resize";
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
  harnessesOn,
  lastAccountFor,
  loadMachineHarnesses,
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
}

/** A harness's logins, only when there is a choice to make between them. */
export function shownAccounts(chips: AccountChip[]): AccountChip[] {
  return chips.length > 1 ? chips : [];
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

/** The card's own size in px: the step it shows, measured rather than guessed. */
export interface CardSize {
  width: number;
  height: number;
}

/**
 * Reports the size `node` has as soon as it is given one, and again whenever it changes, until the
 * returned call. The card's step is in a portal Radix mounts after this component's own effects, so
 * an effect on `open` runs before there is anything to measure: the card measures itself from its
 * ref, which React calls exactly when the node mounts and with `null` when it goes.
 */
export function measureCard(node: HTMLElement | null, onSize: (size: CardSize) => void): () => void {
  if (!node) return () => {};
  const measure = () => {
    const rect = node.getBoundingClientRect();
    onSize({ width: rect.width, height: rect.height });
  };
  // The opening size is measured mid-commit, so it paints without the transition; later changes
  // come from `observeSize`, which reports on a frame of its own rather than inside the observer's
  // delivery (writing layout there is what raises a ResizeObserver loop warning).
  measure();
  return observeSize(node, measure);
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
/** The effort and account row above the search: its 28px and the card's 4px gutter. */
const TRAITS_ROW_PX = 32;
/** A rail button (`size-9`) and the gap under it, and the rail's own padding. */
const RAIL_ITEM_PX = 40;
const RAIL_CHROME_PX = 8;

/**
 * The card's floor: as tall as the longest harness's list (or the rail, when it is longer), so
 * moving along the rail never makes the card jump. `CARD_MAX` still caps it to the window's room.
 */
export function panelHeight(groups: { models: unknown[] }[], hasTraits = false): number {
  const rows = Math.max(1, ...groups.map((group) => group.models.length));
  return (
    Math.max(LIST_CHROME_PX + rows * ROW_PX, RAIL_CHROME_PX + groups.length * RAIL_ITEM_PX) +
    (hasTraits ? TRAITS_ROW_PX : 0)
  );
}

/**
 * The card's second step, for the model the thread runs: its levels from lowest to highest with the
 * default marked, a reset, Fast mode when the model has it, and the harness's logins while it has
 * more than one. A harness without an effort control says why in the levels' place. Its header
 * goes back to the models.
 */
function EffortPanel({
  model,
  onBack,
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
  model: string;
  onBack: () => void;
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
  const panel = useRef<HTMLDivElement>(null);
  // The card opens on this step's checked row (or its first), so ↑↓ and ⏎ act at once; a model just
  // picked settles its level a render later, and focus follows it there.
  // biome-ignore lint/correctness/useExhaustiveDependencies: refocus when the checked level changes
  useEffect(() => {
    const root = panel.current;
    const row =
      root?.querySelector<HTMLElement>("[data-menu-row][aria-checked='true']:not([disabled])") ??
      root?.querySelector<HTMLElement>("[data-menu-row]:not([disabled])");
    row?.focus({ preventScroll: true });
  }, [effort]);
  return (
    <div
      ref={panel}
      onKeyDown={(event) => {
        if (event.key === "ArrowLeft" || event.key === "Backspace") {
          event.preventDefault();
          onBack();
        } else menuKeys(event);
      }}
      style={{ maxHeight: CARD_MAX }}
      className="flex w-[240px] flex-col overflow-y-auto p-1"
    >
      <button
        type="button"
        onClick={onBack}
        className="flex h-7 w-full min-w-0 items-center gap-1 rounded-xl px-2 text-body text-fg-muted hover:bg-fill hover:text-fg focus-visible:bg-fill"
      >
        <ChevronLeft size={13} strokeWidth={2.5} className="shrink-0" />
        <span className="min-w-0 truncate">{model}</span>
      </button>
      <div className={menuSeparator} />
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
    </div>
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
  traits,
  onTraits,
  maxHeight = CARD_MAX,
}: {
  groups: ModelGroup[];
  current?: string | null;
  onPick: (group: ModelGroup, value: string) => void;
  /** What the second step holds for the current model (its effort), and the row that opens it. */
  traits?: string | null;
  onTraits?: () => void;
  /** The card's ceiling for this step: less the note under it when the list has one. */
  maxHeight?: string;
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
    <div
      className="flex min-h-0 w-[min(360px,calc(100vw-32px))] flex-1 flex-col"
      style={{ minHeight: `min(${panelHeight(groups, !!traits)}px, ${maxHeight})` }}
    >
      {traits && onTraits && (
        <div className="shrink-0 p-1 pb-0">
          <button
            type="button"
            onClick={onTraits}
            className="flex h-7 w-full min-w-0 items-center gap-2 rounded-xl px-2 text-body hover:bg-fill focus-visible:bg-fill"
          >
            <span className="min-w-0 flex-1 truncate text-start text-fg-muted">Effort and account</span>
            <span className="shrink-0 text-fg">{traits}</span>
            <ChevronRight size={13} strokeWidth={2.5} className="shrink-0 text-fg-faint" />
          </button>
        </div>
      )}
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
  // The card's step: the model list, or the picked model's effort, Fast mode and logins.
  const [step, setStep] = useState<"models" | "traits">("models");
  // Every opening starts on the models, drawn at the size it needs rather than the last one's.
  const setOpen = (next: boolean) => {
    if (next) {
      setStep("models");
      setBox(null);
      setGliding(false);
    }
    setOpenState(next);
  };
  // `/model` asks the picker to open (store.runCommandAction).
  const ask = useApp((s) => s.modelPickerAsk);
  const seenAsk = useRef(ask);
  useEffect(() => {
    if (ask === seenAsk.current) return;
    seenAsk.current = ask;
    setStep("models");
    setBox(null);
    setGliding(false);
    setOpenState(true);
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
    const listed = isCurrent ? models : h.models.map((m) => ({ value: m.id, name: m.name }));
    return {
      harness: h,
      blocked: unavailableReason(h) ?? (isCurrent || canSwitch ? undefined : "Stop the turn to switch"),
      // One row per model: the model this thread runs, else the one its last pick was, is the row a
      // name shared by two ids keeps.
      models: dedupeModels(listed, isCurrent ? model : (h.last?.model ?? null)),
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
    if (levels.length) setStep("traits");
    else setOpen(false);
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

  // The card's own size: whatever step it shows, measured rather than guessed, so the two steps
  // (the wide list, the narrow effort menu) glide into each other instead of jumping. The size an
  // opening lands on is painted without the transition; only a step changing after it animates.
  const [box, setBox] = useState<CardSize | null>(null);
  const [gliding, setGliding] = useState(false);
  const stopMeasuring = useRef<(() => void) | null>(null);
  const glideFrame = useRef(0);
  // The card's step mounts with the portal, after this component's own effects, so the measurement
  // hangs off the node's ref instead of an effect on `open`: it starts when the node is there.
  const measureBody = useCallback((node: HTMLDivElement | null) => {
    stopMeasuring.current?.();
    stopMeasuring.current = null;
    cancelAnimationFrame(glideFrame.current);
    if (!node) return;
    stopMeasuring.current = measureCard(node, (size) =>
      setBox((current) =>
        current && current.width === size.width && current.height === size.height ? current : size,
      ),
    );
    // Radix publishes the room it has a frame after the card mounts, and that first size is not a
    // step change: it is committed with the transition still off.
    glideFrame.current = requestAnimationFrame(() => setGliding(true));
  }, []);

  // Nothing to show until the engine lists harnesses (or the thread its options): a placeholder pill.
  if (!harnesses && !thread?.configOptions.length) {
    return (
      <span role="status" aria-label="Loading models" className="flex h-[30px] items-center">
        <Skeleton className="h-[30px] w-[132px] rounded-full" />
      </span>
    );
  }

  const traitsStep = step === "traits" && !!traits;
  // The note under a list takes its room from the card's ceiling, never from the list's last row.
  const cardMax = !traitsStep && modelReason ? `calc(${CARD_MAX} - ${NOTE_PX}px)` : CARD_MAX;
  return (
    <CardPopover
      open={open}
      onOpenChange={setOpen}
      role="dialog"
      label="Model"
      // The card focuses its own control (the search field, which the panel does on mount; the
      // effort step its checked row) so the arrow keys act on what the card is for the moment it opens.
      onOpenAutoFocus={(event) => event.preventDefault()}
      // Escape steps back out of the effort menu before it closes the card; the models step has no
      // step behind it, so there Escape closes as it always did.
      onEscapeKeyDown={(event) => {
        if (!traitsStep) return;
        event.preventDefault();
        setStep("models");
      }}
      // The card takes the size its step measured; the step itself is never wider than the window
      // allows, and only the model list scrolls. Its own ceiling is the full card, note included:
      // the list gives the note its room (`cardMax`), so the two together never pass it.
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
        // The card itself never scrolls (the popover's own `overflow-y-auto` would win over a plain
        // `overflow-hidden`): only the model list does, so the header, search and rail stay put. While
        // the two steps swap places the card clips what it is growing out of or into.
        className={cx(
          "min-h-0 overflow-hidden",
          gliding && "transition-[width,height] duration-200 ease-snappy motion-reduce:transition-none",
        )}
        style={box ? { width: box.width, height: box.height } : undefined}
      >
        {/* The ceiling matters as much as the measurement: a long list that grew the body past it
            would make the clipped card taller than the room it has, and nothing would scroll. Capped,
            the body hands the list the room left under the note, and the list scrolls in it. */}
        <div
          ref={measureBody}
          style={{ maxHeight: CARD_MAX }}
          className="flex w-max max-w-[min(360px,calc(100vw-32px))] flex-col"
        >
          {traitsStep ? (
            <EffortPanel
              model={displayName ?? label}
              onBack={() => setStep("models")}
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
          ) : (
            <>
              <ModelPanel
                key={providerId}
                groups={groups}
                current={model}
                onPick={pickModel}
                traits={traits}
                onTraits={() => setStep("traits")}
                maxHeight={cardMax}
              />
              {modelReason && (
                <p className="flex shrink-0 items-start gap-1.5 border-border border-t px-3 py-2 text-caption text-fg-faint">
                  <Lock size={10} className="mt-[2px] shrink-0" />
                  <span>{modelReason}</span>
                </p>
              )}
            </>
          )}
        </div>
      </div>
    </CardPopover>
  );
}
