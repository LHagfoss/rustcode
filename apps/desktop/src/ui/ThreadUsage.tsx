import { useEffect, useState } from "react";
import { leftPercent, type PlanUsage, resetLabel, type UsageWindow, windowTitle } from "../acp/rustcode";
import type { SessionUsage } from "../acp/types";
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
import { cx, Pill, Segments } from "./primitives";

/** Brand while there is room; amber, then red, as the window fills up. */
function contextTint(fraction: number) {
  return fraction < 0.8 ? "var(--rc-accent)" : fraction < 0.95 ? "var(--rc-warning)" : "var(--rc-danger)";
}

function planTint(left: number) {
  return left < 5 ? "bg-danger" : left < 20 ? "bg-warning" : "bg-accent";
}

const percentOf = (part: number, whole: number) =>
  whole > 0 ? `${part / whole < 0.01 && part > 0 ? "<1" : Math.round((part / whole) * 100)}%` : "";

/** Quota and context for the thread shown in Activity. */
export function ThreadUsage({ threadId }: { threadId: string }) {
  const own = useApp((s) => s.planUsage[threadId]);
  const [snapshot, setSnapshot] = useState<SessionUsage>();
  const before = snapshot ?? autoCompactBeforeUsage(threadId);
  const autoCompactAt = useApp((s) => autoCompactThresholdFor(s, threadId));
  const cached = useApp((s) => {
    if (isRemoteThread(threadId)) return undefined;
    const key = planKeyFor(s, threadId);
    return key ? s.planCache[key] : onlyCachedPlan(s.planCache);
  });
  const usage = useApp((s) => s.threads[threadId]?.usage);
  const running = useApp((s) => isRunning(s.threads[threadId]));
  const { plan, cached: fromCache } = shownPlan(own, cached);

  useEffect(() => {
    void refreshPlanUsage(threadId);
  }, [threadId]);
  useEffect(() => {
    if (fromCache) void refreshPlanCache();
  }, [fromCache]);

  if (!plan && !usage?.size) return null;
  return (
    <UsageCard
      plan={plan}
      usage={usage}
      before={before}
      autoCompactAt={autoCompactAt}
      openSettings={plan ? () => openSettings("usage") : undefined}
      onCompact={
        running || !usage?.size
          ? undefined
          : () => {
              setSnapshot(usage);
              void submit("/compact");
            }
      }
    />
  );
}

function ContextInspector({
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
        <span className="font-semibold text-body">
          Context <span className="font-medium text-fg-muted tabular">{percentOf(used, size)}</span>
        </span>
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
    <div className="flex flex-col gap-3 px-2 pt-2">
      {!!usage?.size && (
        <ContextInspector usage={usage} before={before} autoCompactAt={autoCompactAt} onCompact={onCompact} />
      )}
      {windows.length > 0 && (
        <div className="flex flex-col gap-2">
          <div className="flex items-center justify-between gap-2 px-0.5">
            <span className="font-semibold text-body">Plan usage</span>
            <button
              type="button"
              onClick={onOpenSettings}
              aria-label="Open Usage settings"
              className="cursor-pointer rounded-md px-2 py-1 text-caption text-fg-muted hover:bg-fill hover:text-fg"
            >
              Settings
            </button>
          </div>
          {windows.map((window) => (
            <PlanWindow key={windowTitle(window)} window={window} compact />
          ))}
          {plan?.stale && <span className="text-caption text-warning">Quota could not refresh</span>}
        </div>
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
