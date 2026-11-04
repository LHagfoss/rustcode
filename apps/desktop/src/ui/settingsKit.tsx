// The pieces every Settings page is built from (docs/design.md, Card and
// Row recipes): a titled group card, a row, the loading state, and engine
// data loaded once the engine is ready.
//
// A row is a title, a description, an optional status line and a control at
// the right; its own container query stacks the control under the text when
// the window is narrow, whatever the nav takes. A row whose value differs
// from its default shows a reset button before the control, on hover or
// keyboard focus.
//
// Groups and rows carry `data-setting` with their title, so the panel's search
// can scroll to the row it found (Settings.tsx) and light it once.

import { RotateCcw } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useState } from "react";
import type { Json } from "../acp/jsonrpc";
import { request, useApp } from "../state/store";
import { cx, Skeleton, Tooltip } from "./primitives";

export function Group({
  title,
  children,
  footer,
  action,
}: {
  title?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  /** A control at the right of the title (a filter, "New…"). */
  action?: ReactNode;
}) {
  return (
    <section className="mb-7" data-setting={typeof title === "string" ? title : undefined}>
      {(title || action) && (
        <div className="mb-2 flex min-h-6 items-center gap-2 px-1">
          <h3 className="flex-1 font-semibold text-callout text-fg-muted">{title}</h3>
          {action}
        </div>
      )}
      <div className="divide-y divide-border overflow-hidden rounded-xl bg-surface">{children}</div>
      {footer && <p className="mt-2 max-w-[70ch] px-1 text-caption text-fg-faint">{footer}</p>}
    </section>
  );
}

/** Puts a row back to its default; the row only offers it while the value differs. */
function ResetButton({
  title,
  onClick,
  className,
}: {
  title: ReactNode;
  onClick: () => void;
  className?: string;
}) {
  const label = typeof title === "string" ? `Reset ${title} to default` : "Reset to default";
  return (
    <Tooltip label="Reset to default">
      <button
        type="button"
        aria-label={label}
        onClick={onClick}
        className={cx(
          "flex size-6 shrink-0 items-center justify-center rounded-full text-fg-faint opacity-0 hover:bg-fill hover:text-fg focus-visible:opacity-100 group-hover/row:opacity-100",
          className,
        )}
      >
        <RotateCcw size={12} />
      </button>
    </Tooltip>
  );
}

export function Row({
  title,
  subtitle,
  status,
  children,
  className,
  reset,
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  /** A live value shown under the description (a path, why a control is off). */
  status?: ReactNode;
  children?: ReactNode;
  className?: string;
  /** Puts the row back to its default; pass it only while the value differs. */
  reset?: () => void;
}) {
  return (
    <div
      className={cx("@container group/row relative", className)}
      data-setting={typeof title === "string" ? title : undefined}
    >
      <div className="flex min-h-11 flex-col gap-2.5 px-3.5 py-2.5 @min-[34rem]:flex-row @min-[34rem]:items-center @min-[34rem]:gap-6">
        <div className={cx("min-w-0 flex-1", reset && "@max-[34rem]:pr-7")}>
          <div className="text-body">{title}</div>
          {subtitle && <div className="max-w-[70ch] text-caption text-fg-muted">{subtitle}</div>}
          {status && <div className="pt-0.5 text-caption text-fg-faint">{status}</div>}
        </div>
        {(children || reset) && (
          <div className="flex shrink-0 items-center gap-2">
            {/* Stacked, the reset leaves the flow for the row's top right: the control then
                starts at the same left edge as every other stacked control. */}
            {reset && (
              <ResetButton
                title={title}
                onClick={reset}
                className="@max-[34rem]:absolute @max-[34rem]:top-2 @max-[34rem]:right-2.5"
              />
            )}
            {children}
          </div>
        )}
      </div>
    </div>
  );
}

export function Loading({ error }: { error: string | null }) {
  return error ? (
    <p className="text-callout text-warning">{error}</p>
  ) : (
    // Where the page's list will be: a group card of rows, title and subtitle each.
    <section className="mb-7" role="status" aria-label="Loading">
      <Skeleton className="mx-1 mb-3 h-2.5 w-24" />
      <div className="divide-y divide-border overflow-hidden rounded-xl bg-surface">
        {[46, 62, 38, 54].map((w, i) => (
          <div key={i} className="flex min-h-11 items-center gap-4 px-3.5 py-2.5">
            <div className="flex flex-1 flex-col gap-1.5">
              <Skeleton className="h-3" style={{ width: `${w}%` }} />
              <Skeleton className="h-2" style={{ width: `${w + 14}%` }} />
            </div>
            <Skeleton className="h-5 w-9 rounded-full" />
          </div>
        ))}
      </div>
    </section>
  );
}

/** The last answer per method: a page opens with it at once and refreshes behind it. */
const cache = new Map<string, unknown>();

export function useEngineData<T>(method: string, parse: (v: Json) => T) {
  const status = useApp((s) => s.status);
  const [data, setDataState] = useState<T | null>(() => (cache.get(method) as T | undefined) ?? null);
  const [error, setError] = useState<string | null>(null);
  const setData = useCallback(
    (value: T | null) => {
      if (value !== null) cache.set(method, value);
      setDataState(value);
    },
    [method],
  );
  const load = useCallback(async () => {
    try {
      setData(parse(await request(method)));
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [method, parse, setData]);
  useEffect(() => {
    if (status === "ready") void load();
  }, [status, load]);
  return { data, error, setData, reload: load };
}
