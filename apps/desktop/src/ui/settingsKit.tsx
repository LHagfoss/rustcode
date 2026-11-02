// The pieces every Settings page is built from (docs/design.md, Card and
// Row recipes): a titled group card, a row, the loading state, and engine
// data loaded once the engine is ready.

import { type ReactNode, useCallback, useEffect, useState } from "react";
import type { Json } from "../acp/jsonrpc";
import { request, useApp } from "../state/store";
import { cx, Spinner } from "./primitives";

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
    <section className="mb-6">
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
    <div className={cx("flex min-h-11 items-center gap-4 px-3.5 py-2.5", className)}>
      <div className="min-w-0 flex-1">
        <div className="text-body">{title}</div>
        {subtitle && <div className="line-clamp-3 text-caption text-fg-muted">{subtitle}</div>}
      </div>
      {children}
    </div>
  );
}

export function Loading({ error }: { error: string | null }) {
  return error ? (
    <p className="text-callout text-warning">{error}</p>
  ) : (
    <div className="flex items-center gap-2 text-callout text-fg-muted">
      <Spinner /> Loading…
    </div>
  );
}

/** A small text field, per the Input recipe. */
export const inputClass =
  "h-8 rounded-md bg-fill-subtle px-3 text-callout text-fg placeholder:text-fg-faint hover:bg-fill focus:bg-fill";

export function useEngineData<T>(method: string, parse: (v: Json) => T) {
  const status = useApp((s) => s.status);
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(async () => {
    try {
      setData(parse(await request(method)));
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [method, parse]);
  useEffect(() => {
    if (status === "ready") void load();
  }, [status, load]);
  return { data, error, setData, reload: load };
}
