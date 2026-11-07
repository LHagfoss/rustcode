// The composer's one usage meter, after UsageCapsule.swift: the plan's shortest window (5-hour) as
// a short bar with the percentage left, and the context window as a small ring beside it. One
// glance, one card (both, with each plan window's reset), and a way on to Settings' Usage page.
// It reads the store itself, so the composer never re-renders for usage.

import { HoverCard } from "radix-ui";
import { useState } from "react";
import {
  leftPercent,
  type PlanUsage,
  resetLabel,
  shownWindow,
  type UsageWindow,
  windowTitle,
} from "../acp/rustcode";
import type { SessionUsage } from "../acp/types";
import { layoutTiles } from "../lib/treemap";
import {
  onlyCachedPlan,
  openSettings,
  planKeyFor,
  refreshPlanCache,
  refreshPlanUsage,
  shownPlan,
  submit,
  useApp,
} from "../state/store";
import { formatTokens, relativeAge } from "../state/thread";
import { cx, Pill, Segments } from "./primitives";

/** Brand while there is room; amber, then red, as the window fills up. */
function contextTint(fraction: number) {
  return fraction < 0.8 ? "var(--rc-accent)" : fraction < 0.95 ? "var(--rc-warning)" : "var(--rc-danger)";
}

/** Brand while there is room; amber under a fifth, red under a twentieth. */
function planTint(left: number) {
  return left < 5 ? "bg-danger" : left < 20 ? "bg-warning" : "bg-accent";
}

/**
 * The conversation (what a compaction can shrink) in shades of the accent,
 * the fixed overhead sent with every request in greys.
 */
const CATEGORY_COLOR: Record<string, string> = {
  "User messages": "var(--rc-accent)",
  "Agent responses": "color-mix(in srgb, var(--rc-accent) 72%, transparent)",
  "Tool calls": "color-mix(in srgb, var(--rc-accent) 50%, transparent)",
  Subagents: "color-mix(in srgb, var(--rc-accent) 36%, transparent)",
  "Compacted history": "color-mix(in srgb, var(--rc-accent) 24%, transparent)",
  "System prompt": "var(--rc-text-muted)",
  "System tools": "color-mix(in srgb, var(--rc-text-muted) 65%, transparent)",
  Skills: "color-mix(in srgb, var(--rc-text-muted) 40%, transparent)",
};
const categoryColor = (name: string) => CATEGORY_COLOR[name] ?? "var(--rc-text-faint)";

const percentOf = (part: number, whole: number) =>
  whole > 0 ? `${part / whole < 0.01 && part > 0 ? "<1" : Math.round((part / whole) * 100)}%` : "";

/** The context window as a small ring: brand while there is room, amber, then red near the cap. */
function ContextGauge({ fraction }: { fraction: number }) {
  const r = 6.5;
  const c = 2 * Math.PI * r;
  return (
    <svg width="16" height="16" viewBox="0 0 17 17" aria-hidden="true" className="-rotate-90">
      <circle cx="8.5" cy="8.5" r={r} fill="none" stroke="var(--rc-fill-strong)" strokeWidth="2" />
      <circle
        cx="8.5"
        cy="8.5"
        r={r}
        fill="none"
        stroke={contextTint(fraction)}
        strokeWidth="2"
        strokeLinecap="round"
        strokeDasharray={c}
        strokeDashoffset={c * (1 - fraction)}
        className="transition-[stroke-dashoffset] duration-300 ease-snappy"
      />
    </svg>
  );
}

/**
 * Everything the composer says about usage, as one meter: the plan's shortest window and the
 * context ring side by side, and one card that carries the plan's windows with their resets and
 * what fills the context. Hover or click the meter to open it; the card links on to Settings' Usage
 * page. Two separate meters (and two cards) said the same thing twice.
 */
export function UsageMeter({ threadId }: { threadId?: string }) {
  const own = useApp((s) => (threadId ? s.planUsage[threadId] : undefined));
  const usageOpen = useApp((s) => s.settings === "usage");
  const [open, setOpen] = useState(false);
  // The context as it was when Compact was pressed, so the card can compare it with what the
  // harness reports afterwards. It is a snapshot, not a second set of numbers.
  const [compactSnapshot, setCompactSnapshot] = useState<{
    threadId?: string;
    usage: SessionUsage;
  } | null>(null);
  const before = compactSnapshot && compactSnapshot.threadId === threadId ? compactSnapshot.usage : null;
  // The thread's own account has its own numbers: the meter never shows
  // another login's plan, not even the same harness's default one.
  const cached = useApp((s) => {
    const key = planKeyFor(s, threadId);
    if (!key) return onlyCachedPlan(s.planCache);
    if (key.includes("#")) return s.planCache[key];
    return s.planCache[key] ?? (threadId ? undefined : onlyCachedPlan(s.planCache));
  });
  const usage = useApp((s) => (threadId ? s.threads[threadId]?.usage : undefined));
  const running = useApp((s) => (threadId ? (s.threads[threadId]?.running ?? false) : false));
  const { plan, cached: fromCache } = shownPlan(own, cached);
  const shown = plan ? shownWindow(plan) : undefined;
  const left = shown ? leftPercent(shown) : null;
  const low = plan?.windows.find((w) => w !== shown && leftPercent(w) < 20);
  const fraction = usage?.size ? Math.min(1, usage.used / usage.size) : null;
  if (left === null && fraction === null) return null;
  const said = [
    left === null ? null : `${left}% of the ${windowTitle(shown!)} window left`,
    fraction === null ? null : `context ${percentOf(usage!.used, usage!.size)} full`,
  ].filter(Boolean);
  return (
    <HoverCard.Root
      open={open}
      openDelay={60}
      closeDelay={120}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) return;
        // The thread's own numbers, when it has any; for a harness that answers
        // none (Claude Code) the plans call is the only way to refresh the cache.
        if (threadId) void refreshPlanUsage(threadId);
        if (fromCache) void refreshPlanCache();
      }}
    >
      <HoverCard.Trigger asChild>
        <button
          type="button"
          aria-label={`${said.join(", ")}. Shows what fills the context window`}
          aria-expanded={open}
          onClick={(event) => {
            // The card, not Settings: a press is the second way into the breakdown (hover is the
            // first), and the card links on to the full Usage page.
            event.preventDefault();
            setOpen(!open);
          }}
          className="flex h-[30px] cursor-pointer items-center gap-2 rounded-full px-2 hover:bg-fill active:bg-fill-strong data-[state=open]:bg-fill"
        >
          {left !== null && (
            <span className="flex items-center gap-1.5">
              <span className="h-[5px] w-4 overflow-hidden rounded-full bg-fg-faint">
                <span
                  className={cx(
                    "block h-full rounded-full transition-[width] duration-300 ease-snappy",
                    planTint(left),
                  )}
                  style={{ width: `max(3px, ${left}%)` }}
                />
              </span>
              <span
                className={cx(
                  "font-medium text-caption text-fg-muted tabular",
                  // Live while the turn runs, then settles on the number it ended with.
                  running && "rc-shimmer",
                )}
              >
                {left}%
              </span>
            </span>
          )}
          {fraction !== null && <ContextGauge fraction={fraction} />}
          {low && <span className="size-1.5 rounded-full bg-warning" />}
        </button>
      </HoverCard.Trigger>
      <HoverCard.Portal>
        <HoverCard.Content
          side="top"
          align="end"
          sideOffset={8}
          collisionPadding={10}
          className="elevated z-40 max-h-[var(--radix-popper-available-height,80vh)] w-[320px] overflow-y-auto p-3 rc-in"
        >
          <UsageCard
            plan={plan}
            cached={fromCache}
            usage={usage}
            before={before}
            openSettings={usageOpen ? undefined : () => openSettings("usage")}
            onCompact={
              running || !usage?.size
                ? undefined
                : () => {
                    setCompactSnapshot({ threadId, usage });
                    void submit("/compact");
                  }
            }
          />
        </HoverCard.Content>
      </HoverCard.Portal>
    </HoverCard.Root>
  );
}

/**
 * What fills the context window: a stacked bar with the automatic compaction point, a tile map of
 * the categories, and the biggest items in the category under the pointer. A harness that reports
 * only a total keeps the total-only view and its one-line explanation; nothing is invented.
 */
export function ContextInspector({
  usage,
  before,
  onCompact,
}: {
  usage: SessionUsage;
  before?: SessionUsage | null;
  onCompact?: () => void;
}) {
  const [view, setView] = useState<"before" | "after">("after");
  const [picked, setPicked] = useState<string | null>(null);
  const snapshot = view === "before" && before ? before : usage;
  const detail = snapshot.context;
  const used = snapshot.used;
  const shown = detail?.categories.filter((c) => c.tokens > 0) ?? [];
  const free = Math.max(0, detail?.free ?? usage.size - used);
  const fraction = Math.min(1, used / usage.size);
  const largest = shown.reduce<(typeof shown)[number] | undefined>(
    (best, category) => (!best || category.tokens > best.tokens ? category : best),
    undefined,
  );
  const active = shown.find((c) => c.name === picked) ?? largest;
  const compactAt = detail?.compactAt;
  const compactedAfter = !!before && !!before.context && !!detail;
  const comparable =
    !!before &&
    (compactedAfter
      ? (detail?.compactions ?? 0) > (before.context?.compactions ?? 0) || usage.used < before.used
      : usage.used < before.used);
  const tiles = layoutTiles(
    shown.map((category) => ({ key: category.name, value: category.tokens })),
    100,
    100,
    1.4,
  );
  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex items-baseline justify-between gap-3 px-0.5">
        <span className="font-semibold text-body">Context</span>
        <span className="text-callout text-fg-muted tabular">
          <span className="font-medium text-fg">{percentOf(used, usage.size)} used</span>
          <span className="text-fg-faint">
            {" "}
            · {formatTokens(used)} of {formatTokens(usage.size)}
          </span>
        </span>
      </div>
      {comparable && (
        <Segments
          value={view}
          options={[
            { value: "before", label: "Before" },
            { value: "after", label: "After" },
          ]}
          onChange={setView}
          label="Compare the context before and after compaction"
        />
      )}
      <div className="relative">
        <div
          role="meter"
          aria-label="Context used"
          aria-valuemin={0}
          aria-valuemax={usage.size}
          aria-valuenow={used}
          className="flex h-[8px] gap-px overflow-hidden rounded-full bg-fill-strong"
        >
          {shown.length ? (
            shown.map((c) => (
              <span
                key={c.name}
                className="h-full transition-[width] duration-300 ease-snappy first:rounded-l-full"
                style={{ width: `${(c.tokens / usage.size) * 100}%`, background: categoryColor(c.name) }}
              />
            ))
          ) : (
            <span
              className="h-full rounded-full transition-[width] duration-300 ease-snappy"
              style={{ width: `${fraction * 100}%`, background: contextTint(fraction) }}
            />
          )}
        </div>
        {compactAt !== undefined && compactAt > 0 && compactAt < usage.size && (
          <span
            aria-hidden
            title={`Automatic compaction at ${formatTokens(compactAt)}`}
            className="absolute inset-y-0 w-px bg-bg/80"
            style={{ left: `${(compactAt / usage.size) * 100}%` }}
          />
        )}
      </div>
      {compactAt !== undefined && compactAt > 0 && compactAt < usage.size && (
        <p className="px-0.5 text-caption text-fg-faint">Compacts itself at {formatTokens(compactAt)}</p>
      )}
      {shown.length === 0 && (
        <p className="px-0.5 text-caption text-fg-muted">
          This harness reports its totals only, not what fills the window. It compacts on its own when the
          window runs low.
        </p>
      )}
      {shown.length > 0 && (
        <div className="relative h-[132px] w-full overflow-hidden rounded-lg bg-fill-subtle">
          {tiles.map((tile) => {
            const category = shown.find((c) => c.name === tile.key)!;
            const lit = active?.name === category.name;
            return (
              <button
                key={tile.key}
                type="button"
                aria-label={`${category.name}: ${formatTokens(category.tokens)}, ${percentOf(category.tokens, usage.size)}`}
                aria-pressed={lit}
                onPointerEnter={() => setPicked(category.name)}
                onFocus={() => setPicked(category.name)}
                onClick={() => setPicked(category.name)}
                className={cx(
                  "absolute cursor-pointer overflow-hidden rounded-xs text-left transition-[left,top,width,height] duration-300 ease-snappy motion-reduce:transition-none",
                  lit && "z-10 outline outline-1 outline-fg/70",
                )}
                style={{
                  left: `${tile.x}%`,
                  top: `${tile.y}%`,
                  width: `${tile.width}%`,
                  height: `${tile.height}%`,
                  background: categoryColor(category.name),
                }}
              >
                {tile.height > 18 && (
                  <span className="pointer-events-none absolute inset-x-1 top-1 truncate rounded-xs bg-bg/75 px-1 py-0.5 font-medium text-caption text-fg">
                    {category.name}
                  </span>
                )}
              </button>
            );
          })}
        </div>
      )}
      {active && (
        <div className="flex flex-col gap-1 border-border border-t px-0.5 pt-2">
          <div className="flex items-baseline justify-between gap-3">
            <span className="text-callout font-medium">{active.name}</span>
            <span className="text-caption text-fg-muted tabular">
              {formatTokens(active.tokens)} · {percentOf(active.tokens, usage.size)}
            </span>
          </div>
          {active.items?.length ? (
            <ul className="flex flex-col">
              {active.items.slice(0, 5).map((item, index) => (
                <li key={`${item.name}-${index}`} className="flex items-center gap-2 py-[2px] text-caption">
                  <span className="flex-1 truncate text-fg-muted" title={item.name}>
                    {item.name}
                  </span>
                  <span className="w-12 text-right text-fg-faint tabular">
                    {item.tokens === undefined ? "—" : formatTokens(item.tokens)}
                  </span>
                </li>
              ))}
              {active.items.length > 5 && (
                <li className="py-[2px] text-caption text-fg-faint">+{active.items.length - 5} more</li>
              )}
            </ul>
          ) : (
            <p className="text-caption text-fg-muted">This harness reports the category's total only.</p>
          )}
        </div>
      )}
      {shown.length > 0 && (
        <ul className="flex flex-col border-border border-t pt-1">
          <li className="flex items-center gap-2 px-0.5 py-[2px] text-caption">
            <span className="size-2 shrink-0 rounded-xs bg-fill-strong shadow-[inset_0_0_0_0.5px_var(--rc-border-strong)]" />
            <span className="flex-1 text-fg-muted">Free</span>
            <span className="text-fg-muted tabular">{formatTokens(free)}</span>
            <span className="w-9 text-right text-fg-faint tabular">{percentOf(free, usage.size)}</span>
          </li>
        </ul>
      )}
      <ContextFooter usage={usage} />
      {onCompact && (
        <div className="flex items-center justify-between gap-2">
          <Pill onClick={onCompact} className="self-start">
            Compact now
          </Pill>
          <span className="text-caption text-fg-faint">Keeps a summary of the dropped work</span>
        </div>
      )}
    </div>
  );
}

/** Everything the card's context section needs, including the comparison snapshot. */
function ContextFooter({ usage }: { usage: SessionUsage }) {
  const parts = [
    usage.inputTokens !== undefined && `${formatTokens(usage.inputTokens)} in`,
    usage.outputTokens !== undefined && `${formatTokens(usage.outputTokens)} out`,
    !!usage.cachedTokens && `${formatTokens(usage.cachedTokens)} cached`,
  ].filter(Boolean);
  const compactions = usage.context?.compactions ?? 0;
  const notes = [
    usage.context?.model,
    usage.estimated && "estimated until the next reply",
    compactions > 0 &&
      `compacted ${compactions === 1 ? "once" : `${compactions} times`}${usage.context?.reclaimed ? `, ${formatTokens(usage.context.reclaimed)} freed` : ""}`,
  ].filter(Boolean);
  if (!parts.length && !notes.length) return null;
  return (
    <div className="flex flex-col gap-0.5 border-border border-t px-0.5 pt-2 text-caption text-fg-muted">
      {parts.length > 0 && <span className="tabular">Thread total: {parts.join(" · ")}</span>}
      {notes.length > 0 && <span className="text-fg-faint">{notes.join(" · ")}</span>}
    </div>
  );
}

/**
 * The meter's card: every plan window with its reset, then the context window, so one hover answers
 * both questions, and a way on to the full Usage page.
 */
function UsageCard({
  plan,
  cached,
  usage,
  before,
  openSettings: onOpenSettings,
  onCompact,
}: {
  plan?: PlanUsage;
  cached?: boolean;
  usage?: SessionUsage;
  before?: SessionUsage | null;
  openSettings?: () => void;
  onCompact?: () => void;
}) {
  const windows = plan
    ? [...plan.windows].sort((a, b) => (a.minutes ?? Infinity) - (b.minutes ?? Infinity))
    : [];
  const age = plan?.fetchedAt
    ? Date.now() - plan.fetchedAt < 60_000
      ? "just now"
      : relativeAge(plan.fetchedAt)
    : undefined;
  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-baseline justify-between gap-3 px-0.5">
        <span className="font-semibold text-body">Usage</span>
        {plan?.source === "codex" && <span className="text-caption text-fg-faint">from the Codex CLI</span>}
      </div>
      {windows.map((w) => (
        <PlanWindow key={windowTitle(w)} window={w} />
      ))}
      {plan?.fetchedAt && (
        <span className={cx("px-0.5 text-caption", plan.stale ? "text-warning" : "text-fg-faint")}>
          {plan.stale ? "Could not refresh · " : ""}
          {cached ? `Numbers from ${age}` : `Updated ${age}`}
        </span>
      )}
      {usage?.size ? (
        <div className="border-border border-t pt-3">
          <ContextInspector usage={usage} before={before} onCompact={onCompact} />
        </div>
      ) : null}
      {onOpenSettings && (
        <button
          type="button"
          onClick={onOpenSettings}
          className="cursor-pointer px-0.5 text-left text-caption text-fg-faint hover:text-fg-muted"
        >
          Open Usage in Settings
        </button>
      )}
    </div>
  );
}

/** One window: its name and reset, a bar of what is left (it drains) with quarter ticks, the numbers. */
export function PlanWindow({ window: w, title }: { window: UsageWindow; title?: string }) {
  const left = leftPercent(w);
  const used = 100 - left;
  const reset = resetLabel(w);
  return (
    <div className="flex flex-col gap-1.5 px-0.5">
      <div className="flex items-baseline justify-between gap-3 text-callout">
        <span className="font-medium">{title ?? windowTitle(w)}</span>
        {reset && <span className="text-caption text-fg-muted">{reset}</span>}
      </div>
      <div
        role="meter"
        aria-label={`${title ?? windowTitle(w)} left`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={left}
        aria-valuetext={`${left}% left`}
        className="relative h-[6px] overflow-hidden rounded-full bg-fill-strong"
      >
        <span
          className={cx(
            "absolute inset-y-0 left-0 rounded-full transition-[width] duration-300 ease-snappy",
            planTint(left),
          )}
          style={{ width: `max(${left > 0 ? 3 : 0}px, ${left}%)` }}
        />
        {[25, 50, 75].map((tick) => (
          <span key={tick} className="absolute inset-y-0 w-px bg-bg/70" style={{ left: `${tick}%` }} />
        ))}
      </div>
      <div className="flex justify-between text-caption tabular">
        <span className={cx("font-medium", left < 20 ? "text-warning" : "text-fg")}>{left}% left</span>
        <span className="text-fg-faint">{used}% used</span>
      </div>
    </div>
  );
}
