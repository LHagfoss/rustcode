// The composer's meters, after UsageCapsule.swift and PlanUsageMeter.swift:
// a small ring filling as the context window fills, and the plan's
// shortest window (5-hour) as a short bar with the percentage left. Clicking
// the ring opens the context inspector; clicking the bar opens Settings' Usage
// page; hovering the bar opens every plan window at once. They read the store
// themselves: the composer does not re-render for usage.

import { HoverCard } from "radix-ui";
import {
  leftPercent,
  type PlanUsage,
  resetLabel,
  shownWindow,
  type UsageWindow,
  windowTitle,
} from "../acp/rustcode";
import type { SessionUsage } from "../acp/types";
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
import { cx, Pill } from "./primitives";

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

export function ContextRing({ threadId }: { threadId: string }) {
  const usage = useApp((s) => s.threads[threadId]?.usage);
  const running = useApp((s) => s.threads[threadId]?.running ?? false);
  if (!usage?.size) return null;
  const fraction = Math.min(1, usage.used / usage.size);
  const r = 6.5;
  const c = 2 * Math.PI * r;
  // Opens on hover like the plan meter: a glance, not a click. A click target
  // inside a tooltip trigger stopped receiving the click.
  return (
    <HoverCard.Root openDelay={60} closeDelay={120}>
      <HoverCard.Trigger asChild>
        <span
          role="img"
          aria-label={`Context: ${percentOf(usage.used, usage.size)} used, ${formatTokens(usage.used)} of ${formatTokens(usage.size)} tokens`}
          className="flex size-[30px] cursor-default items-center justify-center rounded-full hover:bg-fill data-[state=open]:bg-fill"
        >
          <svg width="17" height="17" viewBox="0 0 17 17" aria-hidden="true" className="-rotate-90">
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
        </span>
      </HoverCard.Trigger>
      <HoverCard.Portal>
        <HoverCard.Content
          side="top"
          align="end"
          sideOffset={8}
          collisionPadding={10}
          className="elevated z-40 w-[320px] p-3 rc-in"
        >
          <ContextInspector usage={usage} onCompact={running ? undefined : () => void submit("/compact")} />
        </HoverCard.Content>
      </HoverCard.Portal>
    </HoverCard.Root>
  );
}

/** What fills the context window: a stacked bar, then each category with its share. */
export function ContextInspector({ usage, onCompact }: { usage: SessionUsage; onCompact?: () => void }) {
  const detail = usage.context;
  const shown = detail?.categories.filter((c) => c.tokens > 0) ?? [];
  const free = Math.max(0, detail?.free ?? usage.size - usage.used);
  const fraction = Math.min(1, usage.used / usage.size);
  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex items-baseline justify-between gap-3 px-0.5">
        <span className="font-semibold text-body">Context</span>
        <span className="text-callout text-fg-muted tabular">
          <span className="font-medium text-fg">{percentOf(usage.used, usage.size)} used</span>
          <span className="text-fg-faint">
            {" "}
            · {formatTokens(usage.used)} of {formatTokens(usage.size)}
          </span>
        </span>
      </div>
      <div
        role="meter"
        aria-label="Context used"
        aria-valuemin={0}
        aria-valuemax={usage.size}
        aria-valuenow={usage.used}
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
      {shown.length === 0 && (
        <p className="px-0.5 text-caption text-fg-muted">
          This harness reports its totals only, not what fills the window. It compacts on its own when the
          window runs low.
        </p>
      )}
      {shown.length > 0 && (
        <ul className="flex flex-col">
          {shown.map((c) => (
            <li key={c.name} className="flex items-center gap-2 px-0.5 py-[3px] text-callout">
              <span className="size-2 shrink-0 rounded-xs" style={{ background: categoryColor(c.name) }} />
              <span className="flex-1 truncate">{c.name}</span>
              <span className="text-fg-muted tabular">{formatTokens(c.tokens)}</span>
              <span className="w-9 text-right text-fg-faint tabular">{percentOf(c.tokens, usage.size)}</span>
            </li>
          ))}
          <li className="flex items-center gap-2 px-0.5 py-[3px] text-callout">
            <span className="size-2 shrink-0 rounded-xs bg-fill-strong shadow-[inset_0_0_0_0.5px_var(--rc-border-strong)]" />
            <span className="flex-1 text-fg-muted">Free</span>
            <span className="text-fg-muted tabular">{formatTokens(free)}</span>
            <span className="w-9 text-right text-fg-faint tabular">{percentOf(free, usage.size)}</span>
          </li>
        </ul>
      )}
      <ContextFooter usage={usage} />
      {onCompact && (
        <Pill onClick={onCompact} className="self-start">
          Compact now
        </Pill>
      )}
    </div>
  );
}

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
 * The plan's usage. A thread shows its own limits (`_rustcode/usage/limits`)
 * when it has them; otherwise the harness's plan as last learnt, from Settings'
 * plans call or another thread on that harness, marked with its age. (Claude
 * Code's limits answer null: only the plans call can read its Keychain.) The
 * numbers refresh on hover, when the thread is selected, on a turn's end, and
 * from the app's three-minute poll, so a new thread knows the limits before its
 * first message and a named account's meter appears without visiting Settings.
 * Each login keeps its own numbers. Clicking the meter opens Settings' Usage
 * page, the app's own page for it now that the sidebar's footer no longer
 * carries a button.
 */
export function PlanMeter({ threadId }: { threadId?: string }) {
  const own = useApp((s) => (threadId ? s.planUsage[threadId] : undefined));
  const usageOpen = useApp((s) => s.settings === "usage");
  // The thread's own account has its own numbers: the meter never shows
  // another login's plan, not even the same harness's default one.
  const cached = useApp((s) => {
    const key = planKeyFor(s, threadId);
    if (!key) return onlyCachedPlan(s.planCache);
    if (key.includes("#")) return s.planCache[key];
    return s.planCache[key] ?? (threadId ? undefined : onlyCachedPlan(s.planCache));
  });
  const { plan, cached: fromCache } = shownPlan(own, cached);
  const shown = plan ? shownWindow(plan) : undefined;
  if (!plan || !shown) return null;
  const left = leftPercent(shown);
  const low = plan.windows.find((w) => w !== shown && leftPercent(w) < 20);
  return (
    <HoverCard.Root
      openDelay={60}
      closeDelay={120}
      onOpenChange={(open) => {
        if (!open) return;
        // The thread's own numbers, when it has any; for a harness that answers
        // none (Claude Code) the plans call is the only way to refresh the cache.
        if (threadId) void refreshPlanUsage(threadId);
        if (fromCache) void refreshPlanCache();
      }}
    >
      <HoverCard.Trigger asChild>
        <button
          type="button"
          aria-label={`Plan usage: ${left}% left of the ${windowTitle(shown)} window. Opens Usage in Settings`}
          aria-pressed={usageOpen}
          onClick={() => openSettings(usageOpen ? null : "usage")}
          className="flex h-[30px] cursor-pointer items-center gap-1.5 rounded-full px-2 hover:bg-fill active:bg-fill-strong data-[state=open]:bg-fill"
        >
          <span className="h-[5px] w-4 overflow-hidden rounded-full bg-fg-faint">
            <span
              className={cx(
                "block h-full rounded-full transition-[width] duration-300 ease-snappy",
                planTint(left),
              )}
              style={{ width: `max(3px, ${left}%)` }}
            />
          </span>
          <span className="font-medium text-caption text-fg-muted tabular">{left}%</span>
          {low && <span className="size-1.5 rounded-full bg-warning" />}
        </button>
      </HoverCard.Trigger>
      <HoverCard.Portal>
        <HoverCard.Content
          side="top"
          align="end"
          sideOffset={8}
          collisionPadding={10}
          className="elevated z-40 w-[300px] p-3 rc-in"
        >
          <PlanCard plan={plan} cached={fromCache} />
        </HoverCard.Content>
      </HoverCard.Portal>
    </HoverCard.Root>
  );
}

function PlanCard({ plan, cached }: { plan: PlanUsage; cached?: boolean }) {
  const windows = [...plan.windows].sort((a, b) => (a.minutes ?? Infinity) - (b.minutes ?? Infinity));
  const age = plan.fetchedAt
    ? Date.now() - plan.fetchedAt < 60_000
      ? "just now"
      : relativeAge(plan.fetchedAt)
    : undefined;
  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-baseline justify-between gap-3 px-0.5">
        <span className="font-semibold text-body">Plan usage</span>
        {plan.source === "codex" && <span className="text-caption text-fg-faint">from the Codex CLI</span>}
      </div>
      {windows.map((w) => (
        <PlanWindow key={windowTitle(w)} window={w} />
      ))}
      {plan.fetchedAt && (
        <span className={cx("px-0.5 text-caption", plan.stale ? "text-warning" : "text-fg-faint")}>
          {plan.stale ? "Could not refresh · " : ""}
          {cached ? `Numbers from ${age}` : `Updated ${age}`}
        </span>
      )}
      {/* The meter's tooltip says what a click does; the card is the meter's tooltip. */}
      <span className="px-0.5 text-caption text-fg-faint">Click to open Usage in Settings</span>
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
