// The pieces every Settings page is built from (docs/design.md › Settings
// section and row): a titled group card, a row, the loading state, the three
// controls a row holds, and engine data loaded once the engine is ready.
//
// A page is a column of sections. A section is its name in `fg`, then one
// card: `rounded-xl`, a 1px `border-strong` edge and a soft step over the page,
// its rows divided by a hairline. A row is a title, a description, an optional
// status line and a control at the right; its own container query stacks the
// control under the text when the window is narrow, whatever the nav takes. A
// row whose value differs from its default shows a reset button before the
// control, on hover or keyboard focus.
//
// The controls are one shape — 32px high, `rounded-lg`, a thin border — so a
// row of buttons, selects and choices reads as one family, not as pills.
//
// Groups and rows carry `data-setting` with their title, so the panel's search
// can scroll to the row it found (Settings.tsx) and light it once.
//
// A row whose value differs from its default registers that reset with the
// page's `RowResets` provider, so the panel's header can offer one "Restore
// defaults" for everything on the page that changed (it disappears again as
// the rows go back to their defaults).

import { Check, ChevronDown, RotateCcw } from "lucide-react";
import { Select as RadixSelect } from "radix-ui";
import {
  type ButtonHTMLAttributes,
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { Json, JsonObject } from "../acp/jsonrpc";
import {
  beginPageRead,
  cachedPage,
  commitPage,
  forgetPage,
  isLatestPageRead,
  pageKey,
  rememberPage,
  sameValue,
} from "../state/pageCache";
import { request, useApp } from "../state/store";
import { cx, menuItem, Skeleton, Tooltip } from "./primitives";

/** A section's name, above its card: `text-base` in `fg`, at the card's own inset. */
export function SectionTitle({
  title,
  action,
  className,
}: {
  title?: ReactNode;
  action?: ReactNode;
  className?: string;
}) {
  if (!title && !action) return null;
  return (
    <div className={cx("mb-2.5 flex min-h-7 items-center gap-2 px-3 sm:px-4", className)}>
      <h2 className="flex min-w-0 flex-1 items-center gap-2 font-semibold text-fg text-title3">{title}</h2>
      {action}
    </div>
  );
}

/** One section: a name and the card of rows under it. */
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
    <section className="mb-8" data-setting={typeof title === "string" ? title : undefined}>
      <SectionTitle title={title} action={action} />
      <div className="divide-y divide-border overflow-hidden rounded-xl border border-border-strong bg-surface-2/40">
        {children}
      </div>
      {footer && <p className="mt-2 max-w-[70ch] px-3 text-caption text-fg-faint sm:px-4">{footer}</p>}
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
          "flex size-6 shrink-0 items-center justify-center rounded-sm text-fg-faint opacity-0 hover:bg-fill hover:text-fg focus-visible:opacity-100 group-hover/row:opacity-100",
          className,
        )}
      >
        <RotateCcw size={12} />
      </button>
    </Tooltip>
  );
}

/** The provider's registry: stable for the life of the page, so a row registers once. */
type RegisterReset = (run: () => void) => () => void;
interface RowResetsState {
  /** Rows on the page that can go back to their default, right now. */
  count: number;
  restore: () => void;
}

const RegisterResetContext = createContext<RegisterReset | null>(null);
const RowResetsContext = createContext<RowResetsState>({ count: 0, restore: () => undefined });

let nextResetId = 0;

/** Collector for the resets of the rows on the page it wraps (Settings.tsx). */
export function RowResets({ children }: { children: ReactNode }) {
  const runs = useRef(new Map<number, () => void>());
  const [version, bump] = useState(0);
  const register = useCallback<RegisterReset>((run) => {
    nextResetId += 1;
    const id = nextResetId;
    runs.current.set(id, run);
    bump((n) => n + 1);
    return () => {
      runs.current.delete(id);
      bump((n) => n + 1);
    };
  }, []);
  const restore = useCallback(() => {
    for (const run of [...runs.current.values()]) run();
  }, []);
  // biome-ignore lint/correctness/useExhaustiveDependencies: `version` is what makes the count fresh
  const state = useMemo<RowResetsState>(() => ({ count: runs.current.size, restore }), [version, restore]);
  return (
    <RegisterResetContext value={register}>
      <RowResetsContext value={state}>{children}</RowResetsContext>
    </RegisterResetContext>
  );
}

/** The page's resettable rows, for the panel header's "Restore defaults". */
export function useRowResets(): RowResetsState {
  return useContext(RowResetsContext);
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
  // Register with the page so its header can restore every changed row at once. The
  // run goes through a ref: the registry is keyed on presence, not on the closure.
  const register = useContext(RegisterResetContext);
  const latest = useRef(reset);
  latest.current = reset;
  const resettable = reset !== undefined;
  useEffect(() => {
    if (!register || !resettable) return;
    return register(() => latest.current?.());
  }, [register, resettable]);
  return (
    <div
      className={cx("@container group/row relative", className)}
      data-setting={typeof title === "string" ? title : undefined}
    >
      <div className="flex min-h-12 flex-col gap-3 px-4 py-3.5 @min-[34rem]:flex-row @min-[34rem]:items-center @min-[34rem]:gap-8">
        <div className={cx("min-w-0 flex-1", reset && "@max-[34rem]:pr-7")}>
          <div className="flex min-h-5 items-center gap-1.5 font-medium text-body text-fg">{title}</div>
          {subtitle && (
            <div className="max-w-[70ch] pt-1 text-balance text-callout text-fg-muted leading-normal">
              {subtitle}
            </div>
          )}
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

// ---------------------------------------------------------------- controls

/**
 * The field shape a row's control takes: 32px high, `rounded-lg`, a thin edge
 * and a soft step over the card, so it reads as a control and not as a block.
 */
export const fieldClass =
  "h-8 rounded-lg border border-border-strong bg-surface-2/60 px-3 text-callout text-fg placeholder:text-fg-faint hover:bg-surface-2 focus:bg-surface-2";

/**
 * A labelled action in a row: the bordered default, `prominent` for the one
 * primary action, `danger` for a destructive one, `quiet` for one that rests
 * without an edge (a section header's "Add").
 */
export function Action({
  prominent,
  danger,
  quiet,
  size = "md",
  className,
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  prominent?: boolean;
  danger?: boolean;
  quiet?: boolean;
  size?: "sm" | "md";
}) {
  return (
    <button
      type="button"
      className={cx(
        "inline-flex shrink-0 items-center justify-center gap-1.5 rounded-lg font-medium whitespace-nowrap text-callout disabled:opacity-40",
        size === "sm" ? "h-6 px-2" : "h-8 px-3",
        prominent
          ? "bg-fg text-bg enabled:hover:bg-fg/90 enabled:active:bg-fg/80"
          : danger
            ? "border border-danger/30 text-danger enabled:hover:bg-danger/12"
            : quiet
              ? "text-fg-muted enabled:hover:bg-fill enabled:hover:text-fg"
              : "border border-border-strong bg-surface-2/60 text-fg enabled:hover:bg-surface-2 enabled:active:bg-surface-3",
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
}

/** A menu's options arrive as `{ value, label }`; `""` is a real choice here. */
export interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

// Radix keeps the empty string for "nothing selected", so an option for "" travels under this.
const EMPTY_OPTION = "__empty__";

/** One choice from a list, in the field shape every Settings row shares. */
export function Select({
  label,
  value,
  options,
  onChange,
  disabled,
  className,
}: {
  label: string;
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <RadixSelect.Root
      value={value || EMPTY_OPTION}
      disabled={disabled}
      onValueChange={(next) => onChange(next === EMPTY_OPTION ? "" : next)}
    >
      <RadixSelect.Trigger
        aria-label={label}
        className={cx(
          fieldClass,
          "inline-flex max-w-[260px] min-w-0 items-center gap-1.5 font-medium outline-none enabled:hover:bg-surface-2 disabled:opacity-40",
          className,
        )}
      >
        <span className="min-w-0 truncate">
          <RadixSelect.Value />
        </span>
        <RadixSelect.Icon className="shrink-0 text-fg-faint">
          <ChevronDown size={12} strokeWidth={2} />
        </RadixSelect.Icon>
      </RadixSelect.Trigger>
      <RadixSelect.Portal>
        <RadixSelect.Content
          position="popper"
          align="end"
          sideOffset={6}
          collisionPadding={8}
          className="elevated z-[60] max-h-[min(360px,var(--radix-select-content-available-height))] max-w-[380px] min-w-[var(--radix-select-trigger-width)] overflow-hidden p-1.5 rc-in"
        >
          <RadixSelect.Viewport>
            {options.map((o) => (
              <RadixSelect.Item
                key={o.value}
                value={o.value || EMPTY_OPTION}
                disabled={o.disabled}
                className={menuItem}
              >
                <RadixSelect.ItemText>{o.label}</RadixSelect.ItemText>
                <RadixSelect.ItemIndicator className="ml-auto shrink-0 text-fg-muted">
                  <Check size={13} />
                </RadixSelect.ItemIndicator>
              </RadixSelect.Item>
            ))}
          </RadixSelect.Viewport>
        </RadixSelect.Content>
      </RadixSelect.Portal>
    </RadixSelect.Root>
  );
}

/**
 * Two to four exclusive choices: a track with a thin edge and the picked
 * segment on a step of its own, sliding there rather than jumping.
 */
export function Choice<T extends string>({
  value,
  options,
  onChange,
  label,
}: {
  value: T;
  options: { value: T; label: string; disabled?: boolean }[];
  onChange: (value: T) => void;
  label: string;
}) {
  const group = useRef<HTMLDivElement>(null);
  const [thumb, setThumb] = useState<{ left: number; width: number } | null>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: re-measures when the choice or labels change
  useLayoutEffect(() => {
    const el = group.current?.querySelector<HTMLElement>('[aria-checked="true"]');
    setThumb(el ? { left: el.offsetLeft, width: el.offsetWidth } : null);
  }, [value, options.map((o) => o.label).join()]);
  return (
    <div
      ref={group}
      role="radiogroup"
      aria-label={label}
      className="relative inline-flex shrink-0 rounded-lg border border-border-strong p-1"
    >
      {thumb && (
        <span
          aria-hidden
          className="absolute top-0 left-0 h-6 rounded-sm bg-surface-2 shadow-1 transition-[transform,width] duration-200 ease-snappy"
          style={{ transform: `translate(${thumb.left}px, 4px)`, width: thumb.width }}
        />
      )}
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={o.value === value}
          disabled={o.disabled}
          onClick={() => onChange(o.value)}
          className={cx(
            "relative h-6 rounded-sm px-2.5 font-medium text-caption whitespace-nowrap disabled:opacity-40",
            o.value === value
              ? cx("text-fg", !thumb && "bg-surface-2 shadow-1")
              : "text-fg-muted hover:text-fg",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Loading({ error }: { error: string | null }) {
  return error ? (
    <p className="text-callout text-warning">{error}</p>
  ) : (
    // Where the page's list will be: a group card of rows, title and subtitle each.
    <section className="mb-8" role="status" aria-label="Loading">
      <Skeleton className="mx-3 mb-3 h-3 w-24 sm:mx-4" />
      <div className="divide-y divide-border overflow-hidden rounded-xl border border-border-strong bg-surface-2/40">
        {[46, 62, 38, 54].map((w, i) => (
          <div key={i} className="flex min-h-12 items-center gap-4 px-4 py-3">
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

/**
 * A page's engine data: the last answer for this ask (`state/pageCache.ts`)
 * draws at once, and the page reads again on open, behind it. A read that
 * started before a newer one, or before an action wrote its own answer, is
 * dropped, so a slow response never puts back what the page just changed.
 * The cache is in memory only and is cleared when the engine goes away.
 */
export function useEngineData<T>(method: string, parse: (v: Json) => T, params: JsonObject = {}) {
  const status = useApp((s) => s.status);
  const key = pageKey(method, params);
  const [data, setDataState] = useState<T | null>(() => cachedPage<T>(key)?.data ?? null);
  const [error, setError] = useState<string | null>(null);
  // The latest render's ask: a stable `load` reads it, so an inline params
  // object or parser does not restart the effect on every render.
  const ask = useRef({ method, params, parse, key });
  ask.current = { method, params, parse, key };
  const setData = useCallback((value: T | null) => {
    const { key } = ask.current;
    if (value === null) forgetPage(key);
    else rememberPage(key, value);
    setDataState((shown) => (sameValue(shown, value) ? shown : value));
  }, []);
  const load = useCallback(async () => {
    const { method, params, parse } = ask.current;
    const token = beginPageRead(key);
    try {
      const answer = parse(await request(method, params));
      const stored = commitPage(key, token, answer);
      if (!stored) return;
      setDataState((shown) => (sameValue(shown, stored.data) ? shown : stored.data));
      setError(null);
    } catch (e) {
      if (!isLatestPageRead(key, token)) return;
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [key]);
  // A key change (or an engine going away) shows that ask's last answer, never
  // the one the page had for another engine.
  useEffect(() => {
    const shown = status === "ready" ? (cachedPage<T>(key)?.data ?? null) : null;
    setDataState((current) => (sameValue(current, shown) ? current : shown));
    setError(null);
  }, [status, key]);
  useEffect(() => {
    if (status === "ready") void load();
  }, [status, load]);
  return { data, error, setData, reload: load };
}
