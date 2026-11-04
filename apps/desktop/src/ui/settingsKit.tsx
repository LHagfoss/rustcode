// The pieces every Settings page is built from (docs/design.md, Card and
// Row recipes): a titled group card, a row, the loading state, and engine
// data loaded once the engine is ready.
//
// Groups and rows carry `data-setting` with their title, so the panel's search
// can scroll to the row it found (Settings.tsx) and light it once.

import { type ReactNode, useCallback, useEffect, useState } from "react";
import type { Json } from "../acp/jsonrpc";
import { request, useApp } from "../state/store";
import { cx, Skeleton } from "./primitives";

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
    <section className="mb-6" data-setting={typeof title === "string" ? title : undefined}>
      {(title || action) && (
        <div className="mb-2 flex min-h-6 items-center gap-2 px-1">
          <h3 className="flex-1 font-semibold text-callout text-fg-muted">{title}</h3>
          {action}
        </div>
      )}
      <div className="divide-y divide-border overflow-hidden rounded-lg bg-surface">{children}</div>
      {footer && <p className="mt-2 px-1 text-caption text-fg-faint">{footer}</p>}
    </section>
  );
}

export function Row({
  title,
  subtitle,
  children,
  className,
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  children?: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cx("flex min-h-11 items-center gap-4 px-3.5 py-2.5", className)}
      data-setting={typeof title === "string" ? title : undefined}
    >
      <div className="min-w-0 flex-1">
        <div className="text-body">{title}</div>
        {subtitle && <div className="text-caption text-fg-muted">{subtitle}</div>}
      </div>
      {children}
    </div>
  );
}

export function Loading({ error }: { error: string | null }) {
  return error ? (
    <p className="text-callout text-warning">{error}</p>
  ) : (
    // Where the page's list will be: a group card of rows, title and subtitle each.
    <section className="mb-6" role="status" aria-label="Loading">
      <Skeleton className="mx-1 mb-3 h-2.5 w-24" />
      <div className="divide-y divide-border overflow-hidden rounded-lg bg-surface">
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
