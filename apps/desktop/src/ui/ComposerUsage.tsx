// The composer's meters, after UsageCapsule.swift and PlanUsageMeter.swift:
// a small ring filling with the context window in use, and the plan's
// shortest window (5-hour) as a short bar with the percentage left. They
// read the store themselves: the composer does not re-render for usage.

import { useMemo } from "react";
import { leftPercent, type PlanUsage, shownWindow, windowDetail, windowTitle } from "../acp/rustcode";
import { refreshPlanUsage, useApp } from "../state/store";
import { formatTokens } from "../state/thread";
import { cx, Tooltip } from "./primitives";

/** Brand while there is room; amber, then red, as the window fills up. */
function contextTint(fraction: number) {
  return fraction < 0.8 ? "var(--rc-brand)" : fraction < 0.95 ? "var(--rc-attention)" : "var(--rc-red)";
}

/** Brand while there is room; amber under a fifth, red under a twentieth. */
function planTint(left: number) {
  return left < 5 ? "bg-danger" : left < 20 ? "bg-warning" : "bg-brand";
}

export function ContextRing({ threadId }: { threadId: string }) {
  const usage = useApp((s) => s.threads[threadId]?.usage);
  if (!usage?.size) return null;
  const fraction = Math.min(1, usage.used / usage.size);
  const left = Math.round((1 - fraction) * 100);
  const r = 6.5;
  const c = 2 * Math.PI * r;
  const detail = [
    `${formatTokens(usage.used)} / ${formatTokens(usage.size)} tokens · ${left}% left${usage.estimated ? " (estimated)" : ""}`,
    usage.inputTokens !== undefined
      ? `${formatTokens(usage.inputTokens)} in · ${formatTokens(usage.outputTokens ?? 0)} out${usage.cachedTokens ? ` · ${formatTokens(usage.cachedTokens)} cached` : ""}`
      : undefined,
  ]
    .filter(Boolean)
    .join("\n");
  return (
    <Tooltip label={<span className="whitespace-pre-line">{detail}</span>} side="top">
      <span
        role="meter"
        aria-label="Context used"
        aria-valuemin={0}
        aria-valuemax={usage.size}
        aria-valuenow={usage.used}
        aria-valuetext={`${left} percent left`}
        className="flex size-[30px] items-center justify-center rounded-full hover:bg-fill-2"
      >
        <svg width="17" height="17" viewBox="0 0 17 17" aria-hidden="true" className="-rotate-90">
          <circle cx="8.5" cy="8.5" r={r} fill="none" stroke="var(--rc-fg-4)" strokeWidth="2" />
          <circle
            cx="8.5"
            cy="8.5"
            r={r}
            fill="none"
            stroke={contextTint(fraction)}
            strokeWidth="2"
            strokeLinecap="round"
            strokeDasharray={c}
            strokeDashoffset={c * (1 - Math.max(fraction, 0.04))}
            className="transition-[stroke-dashoffset] duration-300 ease-snappy"
          />
        </svg>
      </span>
    </Tooltip>
  );
}

/**
 * The plan's usage. A new thread shows what the last thread learnt, so the
 * limits are known before the first message; hovering asks for fresh numbers.
 */
export function PlanMeter({ threadId }: { threadId?: string }) {
  const own = useApp((s) => (threadId ? s.planUsage[threadId] : undefined));
  const all = useApp((s) => s.planUsage);
  const plan: PlanUsage | null | undefined = useMemo(
    () => own ?? Object.values(all).find((p): p is PlanUsage => !!p?.windows.length),
    [own, all],
  );
  const shown = plan ? shownWindow(plan) : undefined;
  if (!plan || !shown) return null;
  const left = leftPercent(shown);
  const low = plan.windows.find((w) => w !== shown && leftPercent(w) < 20);
  return (
    <Tooltip
      side="top"
      label={
        <span className="flex min-w-[200px] flex-col gap-1.5 py-0.5">
          <span className="font-semibold text-caption text-fg-2">
            {plan.source === "codex" ? "Plan usage (from the Codex CLI)" : "Plan usage"}
          </span>
          {plan.windows.map((w) => (
            <span key={windowTitle(w)} className="flex flex-col gap-1">
              <span className="flex justify-between gap-4">
                <span>{windowTitle(w)}</span>
                <span className="text-fg-2 tabular">{windowDetail(w)}</span>
              </span>
              <span className="h-[4px] overflow-hidden rounded-full bg-fg-4">
                <span
                  className={cx("block h-full rounded-full", planTint(leftPercent(w)))}
                  style={{ width: `${leftPercent(w)}%` }}
                />
              </span>
            </span>
          ))}
        </span>
      }
    >
      <span
        role="meter"
        aria-label={`Plan usage left, ${windowTitle(shown)}`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={left}
        onPointerEnter={() => threadId && void refreshPlanUsage(threadId)}
        className="flex h-[30px] items-center gap-1.5 rounded-full px-2 hover:bg-fill-2"
      >
        <span className="h-[5px] w-4 overflow-hidden rounded-full bg-fg-4">
          <span
            className={cx(
              "block h-full rounded-full transition-[width] duration-300 ease-snappy",
              planTint(left),
            )}
            style={{ width: `max(3px, ${left}%)` }}
          />
        </span>
        <span className="font-medium text-caption text-fg-2 tabular">{left}%</span>
        {low && (
          <span
            className="size-1.5 rounded-full bg-warning"
            title={`${windowTitle(low)}: ${leftPercent(low)}% left`}
          />
        )}
      </span>
    </Tooltip>
  );
}
