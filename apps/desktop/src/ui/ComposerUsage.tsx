// The composer's one usage meter, after UsageCapsule.swift: the plan's shortest window (5-hour) as
// a short bar with the percentage left, and the context window as a small ring beside it. One
// glance, one hover card (both, with each plan window's reset), one press to Settings' Usage page.
// It reads the store itself, so the composer never re-renders for usage.

import { HoverCard } from "radix-ui";
import { useEffect, useState } from "react";
import {
  leftPercent,
  type PlanUsage,
  resetLabel,
  shownWindow,
  type UsageWindow,
  windowTitle,
} from "../acp/rustcode";
import type { SessionUsage } from "../acp/types";
import { useHoverTip } from "../lib/hover";
import { paceLabel, planPace } from "../lib/planPace";
import { autoCompactBeforeUsage, autoCompactThresholdFor } from "../state/autoCompactRun";
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
import { formatTokens, isRunning } from "../state/thread";
import { isRemoteThread } from "../state/threadKeys";
import { usagePressTarget } from "../state/usagePage";
import { cx, Pill, Segments, surfaceCollisionPadding, tipSurface } from "./primitives";

/** Brand while there is room; amber, then red, as the window fills up. */
function contextTint(fraction: number) {
  return fraction < 0.8 ? "var(--rc-accent)" : fraction < 0.95 ? "var(--rc-warning)" : "var(--rc-danger)";
}

/** Brand while there is room; amber under a fifth, red under a twentieth. */
/** The plan's bar and percentage stay off the composer until this much or less is left; the hover card always says it. */
const PLAN_SHOWN_BELOW = 50;

function planTint(left: number) {
  return left < 5 ? "bg-danger" : left < 20 ? "bg-warning" : "bg-accent";
}

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
 * what fills the context. A hover opens the card; a press goes on to Settings' Usage page (Enter
 * and Space too, since the meter is a button). Two separate meters (and two cards) said the same
 * thing twice.
 */
export function UsageMeter({ threadId }: { threadId?: string }) {
  const own = useApp((s) => (threadId ? s.planUsage[threadId] : undefined));
  const usageOpen = useApp((s) => s.settings === "usage");
  // The one tooltip and hover-card timing (lib/hover.ts): the card reads its numbers when it opens.
  const tip = useHoverTip("card");
  const open = tip.open;
  // The context as it was when Compact was pressed, so the card can compare it with what the
  // harness reports afterwards. It is a snapshot, not a second set of numbers.
  const [compactSnapshot, setCompactSnapshot] = useState<{
    threadId?: string;
    usage: SessionUsage;
  } | null>(null);
  const snapshot = compactSnapshot && compactSnapshot.threadId === threadId ? compactSnapshot.usage : null;
  // An automatic compaction keeps its own before, so the card compares the
  // context it started from too.
  const before = snapshot ?? (threadId ? autoCompactBeforeUsage(threadId) : undefined) ?? null;
  const autoCompactAt = useApp((s) => (threadId ? autoCompactThresholdFor(s, threadId) : undefined));
  // The thread's own account has its own numbers: the meter never shows
  // another login's plan, not even the same harness's default one.
  // A thread on a paired machine runs on that machine's login: this computer's
  // cache holds another sign-in's numbers, so only the thread's own are shown.
  const cached = useApp((s) => {
    if (threadId ? isRemoteThread(threadId) : s.newThreadHost) return undefined;
    const key = planKeyFor(s, threadId);
    if (!key) return onlyCachedPlan(s.planCache);
    if (key.includes("#")) return s.planCache[key];
    return s.planCache[key] ?? (threadId ? undefined : onlyCachedPlan(s.planCache));
  });
  const usage = useApp((s) => (threadId ? s.threads[threadId]?.usage : undefined));
  const running = useApp((s) => (threadId ? isRunning(s.threads[threadId]) : false));
  const { plan, cached: fromCache } = shownPlan(own, cached);
  // The card's numbers are read when it opens, not while it is closed.
  // biome-ignore lint/correctness/useExhaustiveDependencies: once per opening
  useEffect(() => {
    if (!open) return;
    // The thread's own numbers, when it has any; for a harness that answers
    // none (Claude Code) the plans call is the only way to refresh the cache.
    if (threadId) void refreshPlanUsage(threadId);
    if (fromCache) void refreshPlanCache();
  }, [open]);
  const shown = plan ? shownWindow(plan) : undefined;
  const left = shown ? leftPercent(shown) : null;
  const low = plan?.windows.find(
    (w) => (w !== shown && leftPercent(w) < 20) || planPace(w)?.kind === "runsOut",
  );
  const fraction = usage?.size ? Math.min(1, usage.used / usage.size) : null;
  if (left === null && fraction === null) return null;
  const planSummary =
    shown && left !== null
      ? [`${left}% of the ${windowTitle(shown)} window left`, resetLabel(shown)].filter(Boolean).join(" · ")
      : null;
  const said = [
    planSummary,
    fraction === null ? null : `context ${percentOf(usage!.used, usage!.size)} full`,
  ].filter(Boolean);
  return (
    <HoverCard.Root open={open}>
      <HoverCard.Trigger asChild {...tip.triggerProps}>
        <button
          type="button"
          aria-label={`${said.join(", ")}. Opens Usage in Settings`}
          aria-pressed={usageOpen}
          onClick={() => {
            // A press goes to Settings' Usage page, as it always has (and again closes it); the
            // card is what a hover shows. Pressing also dismisses the card, since the page is
            // where the press said to go.
            tip.dismiss();
            openSettings(usagePressTarget(usageOpen));
          }}
          className="flex h-[30px] cursor-pointer items-center gap-2 rounded-full px-2 hover:bg-fill active:bg-fill-strong data-[state=open]:bg-fill"
        >
          {left !== null && (left < PLAN_SHOWN_BELOW || fraction === null) && (
            <span className="flex items-center gap-1.5">
              <span className="h-[5px] w-4 overflow-hidden rounded-full bg-fg-faint">
                <span
                  className={cx(
                    "block h-full rounded-full transition-[width] duration-300 ease-snappy",
                    planTint(left),
                  )}
                  style={{ width: `max(${left > 0 ? 3 : 0}px, ${left}%)` }}
                />
              </span>
              <span className="font-medium text-caption text-fg-muted tabular">{left}% left</span>
            </span>
          )}
          {fraction !== null && (
            <span className="flex items-center gap-1">
              <ContextGauge fraction={fraction} />
              <span className="font-medium text-caption text-fg-muted tabular">
                {percentOf(usage!.used, usage!.size)}
              </span>
            </span>
          )}
          {low && <span className="size-1.5 rounded-full bg-warning" />}
        </button>
      </HoverCard.Trigger>
      <HoverCard.Portal>
        <HoverCard.Content
          side="top"
          align="end"
          sideOffset={8}
          collisionPadding={surfaceCollisionPadding}
          style={{ maxHeight: "min(var(--radix-popper-available-height, 70vh), calc(100dvh - 140px))" }}
          // The one tip surface (ui/primitives.tsx `tipSurface`); this card has a Compact button in
          // it, so it keeps the pointer instead of letting it through.
          className={cx(tipSurface, "z-40 flex w-[320px] flex-col overflow-hidden p-3")}
          {...tip.contentProps}
        >
          <UsageCard
            plan={plan}
            usage={usage}
            before={before}
            autoCompactAt={autoCompactAt}
            openSettings={
              usageOpen
                ? undefined
                : () => {
                    tip.dismiss();
                    openSettings("usage");
                  }
            }
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

export function ContextInspector({
  usage,
  before,
  autoCompactAt,
  onCompact,
}: {
  usage: SessionUsage;
  before?: SessionUsage | null;
  /** The app's own auto-compact threshold, when rule 1 applies to this harness. */
  autoCompactAt?: number;
  onCompact?: () => void;
}) {
  const [view, setView] = useState<"before" | "after">("after");
  const snapshot = view === "before" && before ? before : usage;
  const detail = snapshot.context;
  const { used, size } = snapshot;
  const fraction = Math.min(1, used / size);
  const compactAt = [detail?.compactAt, autoCompactAt]
    .filter((value): value is number => value !== undefined && value > 0 && value < size)
    .sort((a, b) => a - b)[0];
  const compactedAfter = !!before && !!before.context && !!detail;
  const comparable =
    !!before &&
    (compactedAfter
      ? (detail?.compactions ?? 0) > (before.context?.compactions ?? 0) || usage.used < before.used
      : usage.used < before.used);
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-baseline justify-between gap-3 px-0.5">
        <span className="font-semibold text-body">Context</span>
        <span className="text-callout text-fg-muted tabular">
          {snapshot.estimated && "≈"}
          {formatTokens(used)} / {formatTokens(size)}
        </span>
      </div>
      {comparable && (
        <Segments
          fill
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
          aria-valuemax={size}
          aria-valuenow={Math.min(used, size)}
          className="flex h-[6px] overflow-hidden rounded-full bg-fill-strong"
        >
          <span
            className="h-full rounded-full"
            style={{ width: `${fraction * 100}%`, background: contextTint(fraction) }}
          />
        </div>
        {compactAt !== undefined && (
          <span
            aria-hidden
            className="absolute inset-y-0 w-px bg-fg-muted"
            style={{ left: `${(compactAt / size) * 100}%` }}
          />
        )}
      </div>
      {(compactAt !== undefined || onCompact) && (
        <div className="flex items-center justify-between gap-2">
          {compactAt !== undefined && (
            <span className="text-caption text-fg-muted">Auto-compacts at {formatTokens(compactAt)}</span>
          )}
          {onCompact && (
            <Pill onClick={onCompact} className="ml-auto">
              Compact now
            </Pill>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * The meter's card: every plan window with its reset, then the context window, so one hover answers
 * both questions, and a way on to the full Usage page.
 */
function UsageCard({
  plan,
  usage,
  before,
  autoCompactAt,
  openSettings: onOpenSettings,
  onCompact,
}: {
  plan?: PlanUsage;
  usage?: SessionUsage;
  before?: SessionUsage | null;
  autoCompactAt?: number;
  openSettings?: () => void;
  onCompact?: () => void;
}) {
  const windows = plan
    ? [...plan.windows].sort((a, b) => (a.minutes ?? Infinity) - (b.minutes ?? Infinity))
    : [];
  return (
    <div className="flex min-h-0 flex-col gap-3 overflow-y-auto">
      {windows.map((w) => (
        <PlanWindow key={windowTitle(w)} window={w} compact />
      ))}
      {plan?.stale && <span className="text-caption text-warning">Quota could not refresh</span>}
      {usage?.size ? (
        <div className={cx(windows.length > 0 && "border-border border-t pt-3")}>
          <ContextInspector
            usage={usage}
            before={before}
            autoCompactAt={autoCompactAt}
            onCompact={onCompact}
          />
        </div>
      ) : null}
      {onOpenSettings && (
        <button
          type="button"
          onClick={onOpenSettings}
          className="cursor-pointer rounded-md px-2 py-1 text-left text-caption text-fg-muted hover:bg-fill hover:text-fg"
        >
          Usage settings
        </button>
      )}
    </div>
  );
}

/** One window: its name and reset, a bar of what is left (bright: still left at the reset at this rate), the numbers. */
export function PlanWindow({
  window: w,
  title,
  compact,
}: {
  window: UsageWindow;
  title?: string;
  compact?: boolean;
}) {
  const left = leftPercent(w);
  const reset = resetLabel(w);
  const pace = planPace(w);
  return (
    <div className="flex flex-col gap-1.5 px-0.5">
      <div className="flex items-baseline justify-between gap-3 text-callout">
        <span className="font-medium">{title ?? windowTitle(w)}</span>
        {compact ? (
          <span className="text-caption tabular">{left}% left</span>
        ) : (
          reset && <span className="text-caption text-fg-muted">{reset}</span>
        )}
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
        {pace ? (
          <>
            <span
              data-pace-spend
              className={cx(
                "absolute inset-y-0 left-0 rounded-full",
                pace.kind === "runsOut" ? "bg-warning/45" : cx(planTint(left), "opacity-35"),
              )}
              style={{ width: `${left}%` }}
            />
            {pace.kind === "atReset" && (
              <span
                data-pace-end
                className={cx("absolute inset-y-0 left-0 rounded-full", planTint(left))}
                style={{ width: `max(3px, ${100 - pace.percent}%)` }}
              />
            )}
          </>
        ) : (
          <>
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
          </>
        )}
      </div>
      {compact ? (
        (reset || pace?.kind === "runsOut") && (
          <div className="flex flex-wrap items-baseline justify-between gap-x-2 text-caption text-fg-muted">
            {reset && <span>{reset}</span>}
            {pace?.kind === "runsOut" && <span className="text-warning">May run out early</span>}
          </div>
        )
      ) : (
        <div className="flex items-baseline justify-between gap-3">
          <span className={cx("font-medium text-caption tabular", left < 20 ? "text-warning" : "text-fg")}>
            {left}% left
          </span>
          {pace && (
            <span
              data-pace={pace.kind}
              className={cx("text-caption", pace.kind === "runsOut" ? "text-warning" : "text-fg-muted")}
            >
              {paceLabel(pace)}
            </span>
          )}
        </div>
      )}
    </div>
  );
}
