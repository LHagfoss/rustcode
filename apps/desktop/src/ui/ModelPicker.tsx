// The composer's model pill and its panel: one list, every harness's models under a sticky header
// with the current one on top, its accounts as chips on the right of that header, a search that
// opens focused, the current model checked with its effort steps on its own row, and Fast mode
// under the list. Nothing drills into another pane, so changing a model, an effort, an account or
// a harness is one move in one place. What the thread's harness cannot do is greyed out with the
// reason, from capability flags.

import { Check, ChevronUp, Lock, Zap } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { type ConfigOption, effortLabel, type Harness, type HarnessAccount, modelName } from "../acp/types";
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
  pickNewThreadAccountOn,
  pickNewThreadChoice,
  setConfigOption,
  switchThreadAccount,
  switchThreadHarness,
  useApp,
} from "../state/store";
import type { ThreadState } from "../state/thread";
import { CardPopover } from "./cardPopover";
import { HarnessIcon } from "./HarnessIcon";
import { cx, MenuSearchField, Skeleton } from "./primitives";

interface Choice {
  value: string;
  name: string;
  description?: string;
}

/**
 * The effort steps from lowest to highest, so `off` sits at the left of the row's steps whatever
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

/** One login chip in a harness's header: a named account, or a thread's own state. */
export interface AccountChip {
  /** The account to run on; `null` is the harness's own sign-in, `undefined` a login that is gone. */
  id: string | null | undefined;
  name: string;
  selected: boolean;
  /** A login removed from Settings: shown as the thread's state, never pressed. */
  gone?: boolean;
  /** The plan read found this login signed out: chips say so and are muted. */
  signedOut?: boolean;
}

/** One harness's models in the panel, with what the panel needs to offer them. */
interface ModelGroup {
  harness: Harness;
  /** Why none of them can be chosen now: not installed, or a running turn keeps its harness. */
  blocked?: string;
  models: Choice[];
  /** Its named logins, right-aligned in the group's sticky header; empty when it has none. */
  accounts: AccountChip[];
  /** The effort steps one of its models offers, lowest first. */
  effortsOf: (value: string) => string[];
}

/** The effort steps as a compact segmented control on the model's own row: one move, in place. */
function EffortSteps({
  efforts,
  current,
  onChange,
}: {
  efforts: string[];
  current?: string | null;
  onChange: (effort: string) => void;
}) {
  return (
    <span
      role="radiogroup"
      aria-label="Reasoning effort"
      className="flex shrink-0 items-center gap-px rounded-full bg-fill-subtle p-[2px]"
    >
      {efforts.map((effort) => (
        <button
          key={effort}
          type="button"
          role="radio"
          aria-checked={effort === current}
          tabIndex={-1}
          onClick={(e) => {
            e.stopPropagation();
            onChange(effort);
          }}
          className={cx(
            "h-[18px] rounded-full px-1.5 text-caption",
            effort === current ? "bg-surface-3 font-medium text-fg shadow-1" : "text-fg-muted hover:text-fg",
          )}
        >
          {effortLabel(effort)}
        </button>
      ))}
    </span>
  );
}

/**
 * The list itself: a search that opens focused, every harness's models under a sticky header
 * (the current harness first), the current model checked with its effort steps on its row.
 * ↑↓ move the highlight, Home/End the ends, PageUp/PageDown a screen, ⏎ picks, ←→ step the
 * current model's effort, and Escape clears the search before it closes the panel.
 */
function ModelPanel({
  groups,
  current,
  effort,
  accountsDisabled,
  onPick,
  onEffort,
  onPickAccount,
}: {
  groups: ModelGroup[];
  current?: string | null;
  effort?: string | null;
  accountsDisabled?: boolean;
  onPick: (group: ModelGroup, value: string) => void;
  onEffort: (value: string) => void;
  onPickAccount: (harness: Harness, account: string | null) => void;
}) {
  const [filter, setFilter] = useState("");
  const [active, setActive] = useState(0);
  const field = useRef<HTMLInputElement>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const searching = !!filter.trim();
  const { sections, rows, total } = useMemo(() => {
    const all = groups.flatMap((g) => g.models.map((model) => ({ group: g, model })));
    if (!searching) {
      return {
        sections: groups.map((g) => ({ group: g, rows: g.models.map((model) => ({ group: g, model })) })),
        rows: all,
        total: all.length,
      };
    }
    const hits = groups.flatMap((g) =>
      matchModels(filter, g.models).shown.map((model) => ({ group: g, model })),
    );
    const matched = groups
      .map((g) => ({
        group: g,
        rows: matchModels(filter, g.models).shown.map((model) => ({ group: g, model })),
      }))
      .filter((s) => s.rows.length);
    return { sections: matched, rows: hits, total: hits.length };
  }, [groups, filter, searching]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: a new query starts at the top
  useEffect(() => setActive(0), [filter]);
  // The panel is new to the pointer but not to the thread: open on the model it runs.
  useEffect(() => {
    field.current?.focus();
  }, []);
  // biome-ignore lint/correctness/useExhaustiveDependencies: the highlight moved, so follow it
  useEffect(() => {
    scroller.current?.querySelector('[data-active="true"]')?.scrollIntoView({ block: "nearest" });
  }, [active]);
  const move = (by: number) => setActive((i) => Math.min(Math.max(i + by, 0), Math.max(rows.length - 1, 0)));
  const step = (by: number) => {
    const row = rows[active];
    if (!row || row.model.value !== current) return false;
    const steps = row.group.effortsOf(row.model.value);
    const at = steps.indexOf(effort ?? "");
    const next = steps[at + by];
    if (!next) return false;
    onEffort(next);
    return true;
  };
  return (
    <div
      className="flex min-h-0 flex-1 flex-col"
      onKeyDown={(e) => {
        // A chip in a harness's header presses itself; the panel's keys are for the list.
        if (e.target instanceof HTMLElement && e.target.closest("[data-menu-chips]") && e.key !== "Escape")
          return;
        if (e.key === "Escape") {
          // A search clears first; the second Escape closes the panel (Radix's own dismiss).
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
        } else if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
          if (step(e.key === "ArrowRight" ? 1 : -1)) e.preventDefault();
        } else if (e.key === "Enter") {
          const row = rows[active];
          if (row && !row.group.blocked) {
            e.preventDefault();
            onPick(row.group, row.model.value);
          }
        }
      }}
    >
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
                {section.group.accounts.length ? (
                  <div
                    data-menu-chips
                    className="ms-auto flex min-w-0 flex-wrap items-center justify-end gap-1"
                  >
                    {section.group.accounts.map((chip) => (
                      <button
                        key={chip.id ?? chip.name}
                        type="button"
                        aria-pressed={chip.selected}
                        disabled={accountsDisabled || !!section.group.blocked || chip.gone || chip.signedOut}
                        title={chip.signedOut ? "This account is not signed in" : undefined}
                        aria-label={chip.signedOut ? `${chip.name} (not signed in)` : undefined}
                        onClick={(e) => {
                          e.stopPropagation();
                          if (chip.id === undefined) return;
                          onPickAccount(section.group.harness, chip.id);
                        }}
                        className={cx(
                          "h-5 max-w-[112px] shrink-0 truncate rounded-full px-1.5 text-caption",
                          chip.selected
                            ? "bg-fill-strong font-medium text-fg"
                            : "text-fg-muted hover:bg-fill hover:text-fg",
                          chip.signedOut && !chip.selected && "text-fg-faint",
                          (accountsDisabled || section.group.blocked) && "opacity-40",
                          chip.gone && "italic",
                        )}
                      >
                        {chip.name}
                      </button>
                    ))}
                  </div>
                ) : null}
                {section.group.blocked && (
                  <span className={cx("shrink-0 truncate", !section.group.accounts.length && "ms-auto")}>
                    {section.group.blocked}
                  </span>
                )}
              </div>
            )}
            {section.rows.map(({ group, model: choice }) => {
              const at = rows.findIndex(
                (r) => r.group.harness.id === group.harness.id && r.model.value === choice.value,
              );
              const isCurrent = choice.value === current && group.harness.id === groups[0]?.harness.id;
              const steps = isCurrent ? group.effortsOf(choice.value) : [];
              return (
                <div
                  key={`${group.harness.id}:${choice.value}`}
                  id={`model-${group.harness.id}-${choice.value}`}
                  role="option"
                  tabIndex={-1}
                  aria-selected={isCurrent}
                  aria-disabled={!!group.blocked}
                  data-active={at === active}
                  data-model-row=""
                  onMouseEnter={() => setActive(at)}
                  onClick={() => !group.blocked && onPick(group, choice.value)}
                  className={cx(
                    "flex min-h-7 w-full cursor-pointer items-center gap-2 rounded-md px-2 py-1 text-left",
                    at === active && "bg-fill",
                    group.blocked && "cursor-default opacity-45",
                  )}
                >
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-body">{modelName(choice.name)}</span>
                    {choice.description && choice.description !== modelName(choice.name) && (
                      <span className="block truncate text-caption text-fg-muted">{choice.description}</span>
                    )}
                  </span>
                  {steps.length > 1 && (
                    <span data-menu-static>
                      <EffortSteps efforts={steps} current={effort} onChange={onEffort} />
                    </span>
                  )}
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
        {!searching && total > 0 && <span className="sr-only">{total} models</span>}
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
  const fast = thread?.configOptions.find((o) => o.id === "fast" && o.type === "boolean");
  const harnessName = harness?.name ?? "This harness";
  const effortReason = controlDisabledReason("effort", caps, harnessName);
  const modelReason = controlDisabledReason("model", caps, harnessName);

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
   * A login chip. A new thread keeps the choice for its next one; an idle
   * thread moves to it with the conversation so far as a summary, exactly as
   * switching harness does. A chip of another harness moves there first.
   */
  const pickAccount = (target: Harness, id: string | null) => {
    if (target.id === providerId) {
      if (isNew) pickNewThreadAccount(id);
      // The chip of the login the thread is already on asks nothing of the engine.
      else if (thread && !thread.running && (thread.harness?.account ?? null) !== id)
        void switchThreadAccount(thread.id, id);
      else return;
    } else if (isNew) {
      if (id) pickNewThreadAccountOn(target.id, id);
      else return;
    } else if (thread && !thread.running) {
      void switchThreadHarness(thread.id, target.id, id);
    } else return;
    setOpen(false);
  };

  /** The chips a harness's header shows: its named logins, and a thread's own login state. */
  const chipsFor = (h: Harness): AccountChip[] => {
    const named = h.accounts ?? [];
    const chip = (a: HarnessAccount, selected: boolean): AccountChip => ({
      id: a.id,
      name: a.name,
      selected,
      // A definite `needs_login` is as signed out as a row the plan read saw.
      signedOut: a.auth === "needs_login" || accountSignedOut(h.id, a.id, signedOut),
    });
    const own = h.id === providerId ? account : null;
    if (!isNew && h.id === providerId) {
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
    }
    // A new thread selects a login; on an open thread another harness's chip
    // is where a switch would land, not where the thread runs now.
    if (!isNew) return named.map((a) => chip(a, false));
    const selected = h.id === providerId ? own : newThreadAccountOn(h, lastAccountFor(h.id), signedOut);
    return named.map((a) => chip(a, a.id === selected));
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
    const listed = h.models.map((m) => ({ value: m.id, name: m.name }));
    const effortsOf = (value: string) => {
      const own =
        isCurrent && value === model
          ? efforts
          : orderEfforts(h.models.find((m) => m.id === value)?.efforts ?? []);
      return own;
    };
    return {
      harness: h,
      blocked: unavailableReason(h) ?? (isCurrent || canSwitch ? undefined : "Stop the turn to switch"),
      models: isCurrent ? models : listed,
      accounts: chipsFor(h),
      effortsOf,
    };
  });

  // An external harness lists its models only when asked: ask for every harness on show once the
  // panel opens, so a group never sits empty behind a "no models" line it could have answered.
  const listing = groups
    .filter((g) => !g.blocked && !g.models.length)
    .map((g) => g.harness.id)
    .join();
  useEffect(() => {
    if (!open || !engineReady || !listing) return;
    for (const id of listing.split(",")) void ensureHarnessModels(id);
  }, [open, engineReady, listing]);

  const pickModel = (group: ModelGroup, value: string) => {
    if (group.blocked) return;
    if (group.harness.id === providerId) {
      if (thread) void setConfigOption(thread.id, "model", value);
      else setChoice({ model: value });
    } else if (isNew) {
      // A harness's remembered effort belongs to its own models: the choice normalizes it,
      // and its remembered account goes with it.
      setChoice({ harness: group.harness.id, model: value, effort: null });
      pickNewThreadAccount(newThreadAccountOn(group.harness, lastAccountFor(group.harness.id), signedOut));
    } else if (thread) {
      // The harness's remembered login goes with the model, as a chip would.
      const account = newThreadAccountOn(group.harness, lastAccountFor(group.harness.id), signedOut);
      void switchThreadHarness(thread.id, group.harness.id, account).then((switched) => {
        if (switched) void setConfigOption(thread.id, "model", value);
      });
    }
    setOpen(false);
  };

  const label = pillLabel(model, effortReason ? null : effort, compact);
  // The pill names the login whenever the harness has named accounts, even
  // when there is one: which subscription a thread spends is worth a word.
  // A thread from before accounts says which login it is really on.
  const accountLabel = accountName ?? (!isNew && !account && accounts.length ? "Original login" : undefined);
  const chipsShown = groups.some((g) => g.accounts.length > 0);

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
      // One size either way: the rows keep their height, so choosing a model never resizes it, and
      // a long list scrolls inside the same 430px instead of growing past the window.
      className="w-[min(520px,calc(100vw-32px))] p-0"
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
      <div className="flex min-h-0 flex-1 flex-col">
        <ModelPanel
          groups={groups}
          current={model}
          effort={effort}
          accountsDisabled={accountBlocked}
          onPick={pickModel}
          onEffort={pickEffort}
          onPickAccount={pickAccount}
        />
        {effortReason && efforts.length > 0 && (
          <div className="flex shrink-0 items-center gap-1.5 px-2.5 pb-1 text-caption text-fg-faint">
            <Lock size={10} />
            {effortReason}
          </div>
        )}
        {modelReason && <p className="shrink-0 px-2.5 pb-1 text-caption text-fg-faint">{modelReason}</p>}
        {(fast || (accountBlocked && chipsShown)) && (
          <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-border border-t px-1.5 py-1">
            {fast && (
              <FastToggle
                on={!!fast.currentBool}
                onChange={(on) => thread && void setConfigOption(thread.id, "fast", on)}
              />
            )}
            {accountBlocked && chipsShown && (
              <span className="ms-auto px-1 text-caption text-fg-faint">Stop the turn to switch account</span>
            )}
          </div>
        )}
      </div>
    </CardPopover>
  );
}
