// The pieces every Settings page is built from (docs/design.md › Settings
// section and row): a titled group card, a row, the loading state, the three
// controls a row holds, and engine data loaded once the engine is ready.
//
// A page is a column of sections. A section is its name in `fg`, then one
// card: `rounded-xl` on a step of the page's own fill (`bg-surface`) with a hairline edge, its rows
// divided by a hairline. A row is a title, a description, an optional
// status line and a control at the right; its own container query stacks the
// control under the text when the window is narrow, whatever the nav takes. A
// row whose value differs from its default shows a reset button before the
// control, on hover or keyboard focus.
//
// The controls are one height — 28px, a step of their own
// over the card — so a row of buttons, selects and choices reads as one
// family: fields and selects are rounded rectangles, buttons and choices pills.
//
// Groups and rows carry `data-setting` with their title, so the panel's search
// can scroll to the row it found (Settings.tsx) and light it once.
//
// A row whose value differs from its default registers that reset with the
// page's `RowResets` provider, so the panel's header can offer one "Restore
// defaults" for everything on the page that changed (it disappears again as
// the rows go back to their defaults).

import { Select as RadixSelect } from "radix-ui";
import {
  type ButtonHTMLAttributes,
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import type { Json, JsonObject } from "../acp/jsonrpc";
import { displayPath } from "../acp/types";
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
import { connectionFor, request, useApp } from "../state/store";
import { Check, ChevronDown, Info, RotateCcw } from "./icons";
import { cx, menuItem, Pill, rowInset, Segments, Skeleton, Switch, Tooltip } from "./primitives";

/**
 * A section's name, above its card: `text-title3` in `fg`, at the card's own inset. `minor` is a
 * card's name inside a section that already has a title (an account under its harness, a
 * source under Skills): a step down in size, weight and colour, so the two do not compete.
 */
export function SectionTitle({
  title,
  action,
  minor,
  className,
}: {
  title?: ReactNode;
  action?: ReactNode;
  minor?: boolean;
  className?: string;
}) {
  if (!title && !action) return null;
  const Heading = minor ? "h3" : "h2";
  return (
    <div
      className={cx("mb-2 flex flex-wrap items-center gap-2 px-4", minor ? "min-h-5" : "min-h-7", className)}
    >
      <Heading
        className={cx(
          "flex min-w-0 flex-1 items-center gap-2",
          minor ? "font-medium text-body text-fg-muted" : "font-semibold text-fg text-title3",
        )}
      >
        {title}
      </Heading>
      {action}
    </div>
  );
}

/** One section: a name and the card of rows under it. */
export function Group({
  title,
  children,
  footer,
  hint,
  action,
  minor,
  disabled,
  enabled,
  onEnabledChange,
  switchLabel,
}: {
  title?: ReactNode;
  /** A card inside a section that has its own title: its name reads a step quieter. */
  minor?: boolean;
  children: ReactNode;
  footer?: ReactNode;
  /** What the one-line footer leaves out: a tip behind an info mark after the title. */
  hint?: ReactNode;
  /** A control at the right of the title (a filter, "New…"). */
  action?: ReactNode;
  /** The group has no effect right now: its controls are disabled and it reads as off. */
  disabled?: boolean;
  /**
   * A feature the group turns on: a switch in the title row. Off, the group is that one row
   * and its card and footer fold away; on, its rows show.
   */
  enabled?: boolean;
  onEnabledChange?: (enabled: boolean) => void;
  /** The switch's accessible name; the title when it is text. */
  switchLabel?: string;
}) {
  const feature = onEnabledChange !== undefined;
  const folded = feature && !enabled;
  const toggle = feature ? (
    <Switch
      label={switchLabel ?? (typeof title === "string" ? title : "Turn this on")}
      checked={Boolean(enabled)}
      onChange={onEnabledChange}
    />
  ) : null;
  return (
    <section
      className="mb-6 scroll-mt-5"
      data-setting={typeof title === "string" ? title : undefined}
      aria-disabled={disabled || undefined}
    >
      <SectionTitle
        minor={minor}
        title={
          hint && title ? (
            <>
              {title}
              <HintTip label={typeof title === "string" ? title : "this section"}>{hint}</HintTip>
            </>
          ) : (
            title
          )
        }
        action={
          toggle ? (
            <>
              {!folded && action}
              {toggle}
            </>
          ) : (
            action
          )
        }
      />
      {!folded && (
        <fieldset
          disabled={disabled}
          className={cx(
            "rule-rows m-0 min-w-0 overflow-hidden rounded-xl border border-border bg-surface p-0",
            disabled && "opacity-60",
          )}
        >
          {children}
        </fieldset>
      )}
      {footer && !folded && <div className="mt-2 max-w-[70ch] px-4 text-caption text-fg-muted">{footer}</div>}
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

/** Optional page-level title metadata for sub-pages (e.g. one machine's name and back link). */
export interface SettingsPageMeta {
  title: ReactNode;
  badge?: ReactNode;
  back?: { label: string; onClick: () => void };
}

interface SettingsHeaderContextValue {
  slot: HTMLDivElement | null;
  setMeta: (meta: SettingsPageMeta | null) => void;
}

const SettingsHeaderContext = createContext<SettingsHeaderContextValue>({
  slot: null,
  setMeta: () => undefined,
});

/** Connects a Settings page to its floating header's right-side action slot and page title. */
export function SettingsHeaderProvider({
  slot,
  setMeta,
  children,
}: {
  slot: HTMLDivElement | null;
  setMeta: (meta: SettingsPageMeta | null) => void;
  children: ReactNode;
}) {
  const value = useMemo(() => ({ slot, setMeta }), [slot, setMeta]);
  return <SettingsHeaderContext value={value}>{children}</SettingsHeaderContext>;
}

/** Places a Settings page's primary button(s) in the floating Settings header, aligned with content. */
export function SettingsHeaderActions({ children }: { children: ReactNode }) {
  const { slot } = useContext(SettingsHeaderContext);
  if (!slot) return null;
  return createPortal(children, slot);
}

/** Lets a sub-page override the Settings large title and header title (cleared on unmount). */
export function useSettingsPageMeta(meta: SettingsPageMeta | null) {
  const { setMeta } = useContext(SettingsHeaderContext);
  useEffect(() => {
    setMeta(meta);
    return () => setMeta(null);
  }, [setMeta, meta]);
}

/**
 * What a row's one short line leaves out, behind a small info mark after its title: it opens on
 * hover or keyboard focus, so the row itself stays one line (docs/design.md › Settings section and row).
 */
export function HintTip({ label, children }: { label: string; children: ReactNode }) {
  return (
    <Tooltip label={children}>
      <button
        type="button"
        aria-label={`More about ${label}`}
        className="flex size-4 shrink-0 items-center justify-center rounded-full text-fg-faint hover:text-fg focus-visible:text-fg"
      >
        <Info size={12} />
      </button>
    </Tooltip>
  );
}

export function Row({
  title,
  subtitle,
  hint,
  status,
  badge,
  children,
  className,
  reset,
  stacked = false,
}: {
  title: ReactNode;
  /** One short line (about 90 characters): what the row does that its title does not say. */
  subtitle?: ReactNode;
  /** Anything longer a person may need: a tip behind an info mark after the title. */
  hint?: ReactNode;
  /** A live value shown under the description (a path, why a control is off). */
  status?: ReactNode;
  /**
   * A row's short state (a `Chip`), just before its controls on the same line, so a
   * state and the action that changes it read together. A stacked row keeps it at its
   * top right, level with the title, and its controls in their own line below.
   */
  badge?: ReactNode;
  children?: ReactNode;
  className?: string;
  /** Puts the row back to its default; pass it only while the value differs. */
  reset?: () => void;
  stacked?: boolean;
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
      {badge && stacked && <div className="absolute end-4 top-3 flex items-center gap-2">{badge}</div>}
      <div
        className={cx(
          "flex min-h-12 flex-col gap-3",
          !stacked && "@min-[34rem]:flex-row @min-[34rem]:items-center @min-[34rem]:gap-6",
          rowInset,
        )}
      >
        <div
          className={cx(
            "min-w-0 flex-1",
            badge && stacked ? "pr-24" : undefined,
            reset && (stacked ? "pr-7" : "@max-[34rem]:pr-7"),
          )}
        >
          <div className="flex min-h-5 items-center gap-1.5 font-medium text-body text-fg">
            {title}
            {hint && <HintTip label={typeof title === "string" ? title : "this setting"}>{hint}</HintTip>}
          </div>
          {subtitle && (
            <div className="max-w-[90ch] pt-1 text-balance text-callout text-fg-muted leading-normal">
              {subtitle}
            </div>
          )}
          {status && <div className="pt-0.5 text-caption text-fg-muted">{status}</div>}
        </div>
        {(children || reset || (badge && !stacked)) && (
          <div className="flex shrink-0 flex-wrap items-center gap-2">
            {badge && !stacked && <div className="flex items-center gap-2">{badge}</div>}
            {/* Stacked, the reset leaves the flow for the row's top right: the control then
                starts at the same left edge as every other stacked control. */}
            {reset && (
              <ResetButton
                title={title}
                onClick={reset}
                className={
                  stacked
                    ? "absolute top-2 right-2"
                    : "@max-[34rem]:absolute @max-[34rem]:top-2 @max-[34rem]:right-2"
                }
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
 * The field shape a row's control takes: 28px high, `rounded-lg` and a step of
 * its own over the card, so it reads as a control and not as a block.
 */
/** A switch setting's value as a row reads it. */
export const boolLabel = (value: Json | undefined) => (value ? "On" : "Off");

const fieldSurface = "h-7 bg-fill px-3 text-callout text-fg hover:bg-fill-strong focus:bg-fill-strong";

export const fieldClass = cx(fieldSurface, "rounded-lg placeholder:text-fg-faint");

/**
 * A labelled action in a row: the app's one labelled button (`Pill`), so Settings and the rest of
 * the app share its shape and fills. Secondary by default, `prominent` for the one primary action,
 * `danger` for a destructive one, `quiet` for an action that rests without a fill. Prefer a short text label.
 */
export function Action({
  size = "md",
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  prominent?: boolean;
  danger?: boolean;
  quiet?: boolean;
  size?: "sm" | "md";
}) {
  return <Pill size={size} {...rest} />;
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
          fieldSurface,
          "inline-flex max-w-[260px] min-w-0 items-center justify-between gap-2 rounded-full font-medium disabled:opacity-40",
          className,
        )}
      >
        <span className="min-w-0 truncate">
          <RadixSelect.Value />
        </span>
        <RadixSelect.Icon className="shrink-0 text-fg-muted">
          <ChevronDown size={12} strokeWidth={2.25} />
        </RadixSelect.Icon>
      </RadixSelect.Trigger>
      <RadixSelect.Portal>
        <RadixSelect.Content
          position="popper"
          align="end"
          sideOffset={6}
          collisionPadding={8}
          className="elevated rounded-2xl z-[60] max-h-[min(360px,var(--radix-select-content-available-height))] max-w-[380px] min-w-[var(--radix-select-trigger-width)] overflow-hidden p-2 rc-in"
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
 * Two to four exclusive choices: the segmented control (primitives.tsx › Segments) at a row's
 * height, the picked segment's pill sliding there rather than jumping.
 */
export function Choice<T extends string>(props: {
  value: T;
  options: { value: T; label: string; disabled?: boolean }[];
  onChange: (value: T) => void;
  label: string;
}) {
  return <Segments size="md" {...props} />;
}

/** A row's state at a glance: a short tinted chip (signed in, signed out, active). */
export function Chip({
  tone = "idle",
  title,
  children,
}: {
  tone?: "ok" | "warn" | "bad" | "idle";
  title?: string;
  children: ReactNode;
}) {
  return (
    <span
      title={title}
      className={cx(
        "inline-flex h-5 shrink-0 items-center gap-1 rounded-full px-2 font-medium text-caption",
        tone === "ok"
          ? "bg-success/12 text-success"
          : tone === "warn"
            ? "bg-warning/12 text-warning"
            : tone === "bad"
              ? "bg-danger/12 text-danger"
              : "bg-fill text-fg-muted",
      )}
    >
      {children}
    </span>
  );
}

/**
 * A sentence with its backticked commands and paths set in code type, so a title or a note never
 * shows the backticks themselves. Text without any comes back as the same string.
 */
export function withCode(text: string): ReactNode {
  if (!text.includes("`")) return text;
  // One span, so a flex title row keeps the sentence as a single item.
  return (
    <span>
      {text.split(/(`[^`]+`)/g).map((part, i) =>
        part.length > 2 && part.startsWith("`") && part.endsWith("`") ? (
          // biome-ignore lint/suspicious/noArrayIndexKey: the parts of one fixed sentence
          <code key={i} className="selectable rounded-xs bg-fill px-1 py-px font-mono">
            {part.slice(1, -1)}
          </code>
        ) : (
          part
        ),
      )}
    </span>
  );
}

/** One short literal the user may copy (a path, command, id or version), inline in the normal text around it. */
export function Literal({ children, title }: { children: ReactNode; title?: string }) {
  return (
    <span title={title} className="selectable font-mono">
      {children}
    </span>
  );
}

/** A folder or file as people read it: shortened (`~/code/app`) with the full path on hover. */
export function PathText({ path, className }: { path: string; className?: string }) {
  const short = displayPath(path);
  return (
    <span
      title={short === path ? undefined : path}
      className={cx("selectable min-w-0 truncate font-mono", className)}
    >
      {short}
    </span>
  );
}

/**
 * A failure as a plain sentence first, then the raw technical text folded under "Details" in
 * monospace, so a long error never fills the page with code.
 */
export function ErrorDetails({
  summary,
  detail,
  className,
}: {
  summary: string;
  detail?: string | null;
  className?: string;
}) {
  const raw = detail?.trim();
  return (
    <div className={cx("selectable text-callout", className)}>
      <p className="m-0">{summary}</p>
      {raw && raw !== summary && (
        <details className="pt-0.5 text-caption text-fg-muted">
          <summary className="w-fit cursor-pointer select-none hover:text-fg">Details</summary>
          <pre className="m-0 mt-1 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-md bg-code-bg p-2 font-mono text-caption">
            {raw}
          </pre>
        </details>
      )}
    </div>
  );
}

/**
 * A whole number chosen from a short range, in a row: the native slider (so
 * the arrow keys, Home and End work) with the value it is on beside it and the
 * ends of its range under them.
 *
 * With `onCommit` the thumb follows the drag locally and the write happens
 * once, when it is released, a key comes up or focus leaves: one request per
 * gesture instead of one per step.
 */
export function Slider({
  label,
  value,
  min,
  max,
  step = 1,
  onChange,
  onCommit,
  unit,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  /** A live value while the slider moves; with `onCommit`, a preview only. */
  onChange?: (value: number) => void;
  /** The value to save once the gesture ends. */
  onCommit?: (value: number) => void;
  /** What one step is of, for the caption (“threads”). */
  unit: string;
}) {
  const [draft, setDraft] = useState(value);
  const dragging = useRef(false);
  const lastSent = useRef<number | null>(null);
  useEffect(() => {
    lastSent.current = null;
    if (!dragging.current) setDraft(value);
  }, [value]);
  const commit = (next: number) => {
    if (!onCommit || next === value || next === lastSent.current) return;
    lastSent.current = next;
    onCommit(next);
  };
  const shown = onCommit ? draft : value;
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-3">
        <input
          type="range"
          aria-label={label}
          aria-valuetext={`${shown} ${unit}`}
          min={min}
          max={max}
          step={step}
          value={shown}
          onChange={(e) => {
            const next = Number(e.target.value);
            if (onCommit) setDraft(next);
            onChange?.(next);
          }}
          onPointerDown={() => {
            dragging.current = true;
            lastSent.current = null;
          }}
          onPointerUp={(e) => {
            dragging.current = false;
            commit(Number(e.currentTarget.value));
          }}
          onPointerCancel={() => {
            dragging.current = false;
            lastSent.current = null;
            setDraft(value);
          }}
          onKeyDown={() => {
            lastSent.current = null;
          }}
          onKeyUp={(e) => commit(Number(e.currentTarget.value))}
          onBlur={(e) => {
            dragging.current = false;
            commit(Number(e.currentTarget.value));
          }}
          className="rc-range h-5 w-36 cursor-pointer"
          style={{ "--rc-range-progress": `${((shown - min) / (max - min)) * 100}%` } as React.CSSProperties}
        />
        <span className="min-w-20 text-right font-medium text-callout text-fg tabular">
          {shown} {unit}
        </span>
      </div>
      {/* Under the rail, not under the row: `min` sits at the range's start and `max` at its end. */}
      <div className="flex w-36 justify-between text-caption text-fg-faint tabular">
        <span>{min}</span>
        <span>{max}</span>
      </div>
    </div>
  );
}

export function Loading({ error, onRetry }: { error: string | null; onRetry?: () => void }) {
  return error ? (
    <div className="flex items-center gap-3">
      <p className="text-callout text-warning">{error}</p>
      {onRetry && (
        <Action quiet size="sm" onClick={onRetry}>
          Retry
        </Action>
      )}
    </div>
  ) : (
    // Where the page's list will be: a group card of rows, title and subtitle each.
    <section className="mb-6" role="status" aria-label="Loading">
      <Skeleton className="mx-3 mb-3 h-3 w-24 sm:mx-4" />
      <div className="rule-rows overflow-hidden rounded-xl border border-border bg-surface">
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

/** One answer from several computers: this one's failure fails it, a paired machine's drops out. */
async function readFleet<T>(
  hosts: readonly string[],
  method: string,
  params: JsonObject,
  parse: (v: Json) => T,
  combine: (parts: { host: string; data: T }[]) => T,
): Promise<T> {
  const reads = await Promise.allSettled(
    hosts.map(async (host) => {
      if (!host) return { host, data: parse(await request(method, params)) };
      const conn = connectionFor(host);
      if (!conn) throw new Error("that machine is not connected");
      return { host, data: parse(await conn.request(method, params, 20_000)) };
    }),
  );
  const local = hosts.indexOf("");
  const failed = local >= 0 ? reads[local] : undefined;
  if (failed?.status === "rejected") throw failed.reason;
  const parts = reads.flatMap((read) => (read.status === "fulfilled" ? [read.value] : []));
  if (!parts.length) throw new Error("no machine answered");
  return combine(parts);
}

/**
 * A page's engine data: the last answer for this ask (`state/pageCache.ts`)
 * draws at once, and the page reads again on open, behind it. A read that
 * started before a newer one, or before an action wrote its own answer, is
 * dropped, so a slow response never puts back what the page just changed.
 * The cache is in memory only and is cleared when the engine goes away.
 */
export function useEngineData<T>(
  method: string,
  parse: (v: Json) => T,
  params: JsonObject = {},
  {
    skip = false,
    hosts,
    combine,
  }: {
    skip?: boolean;
    /**
     * The computers to ask (`""` is this one), each over its own connection,
     * with `combine` making one answer of theirs. A paired machine that does
     * not answer is left out; only this computer's failure is the page's.
     */
    hosts?: readonly string[];
    combine?: (parts: { host: string; data: T }[]) => T;
  } = {},
) {
  const status = useApp((s) => s.engine.phase);
  const fleet = hosts && combine && !(hosts.length === 1 && hosts[0] === "") ? hosts : undefined;
  const key = fleet ? `${pageKey(method, params)} @${fleet.join(",")}` : pageKey(method, params);
  const [data, setDataState] = useState<T | null>(() => cachedPage<T>(key)?.data ?? null);
  const [error, setError] = useState<string | null>(null);
  // The latest render's ask: a stable `load` reads it, so an inline params
  // object or parser does not restart the effect on every render.
  const ask = useRef({ method, params, parse, key, fleet, combine });
  ask.current = { method, params, parse, key, fleet, combine };
  const skipped = useRef(skip);
  skipped.current = skip;
  const setData = useCallback((value: T | null) => {
    const { key } = ask.current;
    if (value === null) forgetPage(key);
    else rememberPage(key, value);
    setDataState((shown) => (sameValue(shown, value) ? shown : value));
  }, []);
  const load = useCallback(async () => {
    if (skipped.current) return;
    const { method, params, parse, fleet, combine } = ask.current;
    const token = beginPageRead(key);
    try {
      const answer =
        fleet && combine
          ? await readFleet(fleet, method, params, parse, combine)
          : parse(await request(method, params));
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
    if (status === "ready" && !skip) void load();
  }, [status, skip, load]);
  return { data, error, setData, reload: load };
}
