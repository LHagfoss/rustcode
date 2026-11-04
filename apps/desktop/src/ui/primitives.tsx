// Small building blocks shared by every view, matching the SwiftUI app's
// ChromeButton, ToolbarCluster, PillButtonStyle and mini spinner.

import { Check, ChevronDown, CircleAlert, TriangleAlert } from "lucide-react";
import { Select as RadixSelect, Tooltip as RadixTooltip } from "radix-ui";
import {
  type ButtonHTMLAttributes,
  forwardRef,
  type ReactNode,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { errorText, toast } from "../state/store";

export function cx(...parts: (string | false | null | undefined)[]): string {
  return parts.filter(Boolean).join(" ");
}

/** The running indicator: a 12px arc trimmed to 70%, one turn per second. */
// The ring of a 3×3 grid, clockwise from the top-left; the centre (4) stays still.
const ORB_RING = [0, 1, 2, 5, 8, 7, 6, 3];

/** An agent working: a small ring of dots with a travelling light, quieter than a spinner. */
export function Orb({ className }: { className?: string }) {
  return (
    <span className={cx("rc-orb text-accent", className)} aria-hidden>
      {Array.from({ length: 9 }, (_, cell) => {
        const at = ORB_RING.indexOf(cell);
        return at < 0 ? (
          <i key={cell} data-centre="" />
        ) : (
          <i
            key={cell}
            style={{ animationDelay: `${-((ORB_RING.length - at) / ORB_RING.length) * 1100}ms` }}
          />
        );
      })}
    </span>
  );
}

export function Spinner({ size = 12, className }: { size?: number; className?: string }) {
  const r = size / 2 - 1;
  const c = 2 * Math.PI * r;
  return (
    <svg
      width={size}
      height={size}
      viewBox={`0 0 ${size} ${size}`}
      className={cx("shrink-0 text-fg-muted", className)}
      style={{ animation: "rc-spin 1s linear infinite" }}
      aria-hidden
    >
      <circle
        cx={size / 2}
        cy={size / 2}
        r={r}
        fill="none"
        stroke="currentColor"
        strokeWidth={1.75}
        strokeLinecap="round"
        strokeDasharray={`${c * 0.7} ${c}`}
      />
    </svg>
  );
}

/**
 * A placeholder block where content will appear (docs/design.md, Loading):
 * `fill-subtle`, swept by a soft highlight that stops under reduced motion.
 * Size and radius come from the caller's classes.
 */
export function Skeleton({ className, style }: { className?: string; style?: React.CSSProperties }) {
  return <span aria-hidden className={cx("rc-skeleton block", className)} style={style} />;
}

export function Tooltip({
  label,
  children,
  side = "top",
}: {
  label?: ReactNode;
  children: ReactNode;
  side?: "top" | "bottom" | "left" | "right";
}) {
  if (!label) return <>{children}</>;
  return (
    // Delays come from the one Tooltip.Provider in App.tsx: 400 ms for the first, then
    // none while the pointer moves between triggers. The tip never takes the pointer.
    <RadixTooltip.Root>
      <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
      <RadixTooltip.Portal>
        <RadixTooltip.Content
          side={side}
          sideOffset={8}
          collisionPadding={10}
          className="elevated pointer-events-none z-50 max-w-80 rounded-lg px-2.5 py-1 text-callout text-fg"
        >
          {label}
        </RadixTooltip.Content>
      </RadixTooltip.Portal>
    </RadixTooltip.Root>
  );
}

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & { label: string; tooltip?: ReactNode };

/** A 30×28 icon button that answers the pointer with a quiet fill. */
export const ChromeButton = forwardRef<HTMLButtonElement, ButtonProps>(function ChromeButton(
  { label, tooltip, className, children, ...rest },
  ref,
) {
  return (
    <Tooltip label={tooltip ?? label}>
      <button
        ref={ref}
        type="button"
        aria-label={label}
        className={cx(
          "inline-flex h-7 w-[30px] items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong disabled:opacity-40 disabled:hover:bg-transparent",
          className,
        )}
        {...rest}
      >
        {children}
      </button>
    </Tooltip>
  );
});

/** A capsule that groups icon buttons, as the toolbar does. */
export function Cluster({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div className={cx("inline-flex h-7 items-center rounded-full bg-fill-subtle px-[3px]", className)}>
      {children}
    </div>
  );
}

export const ClusterButton = forwardRef<HTMLButtonElement, ButtonProps & { on?: boolean }>(
  function ClusterButton({ label, tooltip, on, className, children, ...rest }, ref) {
    return (
      <Tooltip label={tooltip ?? label}>
        <button
          ref={ref}
          type="button"
          aria-label={label}
          aria-pressed={on}
          className={cx(
            "inline-flex h-6 w-8 items-center justify-center rounded-full text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong disabled:opacity-40",
            on && "bg-fill-strong text-fg",
            className,
          )}
          {...rest}
        >
          {children}
        </button>
      </Tooltip>
    );
  },
);

/**
 * The labelled button (docs/design.md › Component recipes): a capsule, 24, 26 or 30 high.
 * Secondary by default; `prominent` for the one primary action, `danger` for a destructive one,
 * `quiet` for an action that rests without a fill (a section header's "Add").
 */
export const Pill = forwardRef<
  HTMLButtonElement,
  ButtonHTMLAttributes<HTMLButtonElement> & {
    prominent?: boolean;
    danger?: boolean;
    quiet?: boolean;
    size?: "sm" | "md" | "lg";
  }
>(function Pill({ prominent, danger, quiet, size = "md", className, children, ...rest }, ref) {
  return (
    <button
      ref={ref}
      type="button"
      className={cx(
        "inline-flex shrink-0 items-center justify-center gap-1.5 rounded-full font-medium text-callout whitespace-nowrap disabled:opacity-40",
        size === "lg" ? "h-[30px] px-3.5" : size === "sm" ? "h-6 px-2.5" : "h-[26px] px-3",
        prominent
          ? "bg-fg text-bg enabled:hover:bg-fg/90 enabled:active:bg-fg/80"
          : danger
            ? "bg-danger/12 text-danger enabled:hover:bg-danger/18 enabled:active:bg-danger/24"
            : quiet
              ? "text-fg-muted enabled:hover:bg-fill enabled:hover:text-fg enabled:active:bg-fill-strong"
              : "bg-fill text-fg enabled:hover:bg-fill-strong enabled:active:bg-fill-strong",
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
});

/** A menu's surface and rows, for `DropdownMenu` and `ContextMenu` alike. */
export const menuSurface = "elevated z-50 min-w-[200px] p-1.5 rc-in";
export const menuItem =
  "flex w-full cursor-pointer items-center gap-2 rounded-lg px-3 py-[7px] text-left text-body outline-none data-[disabled]:cursor-default data-[disabled]:opacity-40 data-[highlighted]:bg-fill";
export const menuItemDanger = "text-danger data-[highlighted]:bg-danger/12";
export const menuSeparator = "mx-1.5 my-1 h-px shrink-0 bg-border";
export const menuLabel = "px-3 pt-1.5 pb-1 text-caption text-fg-faint";

/** A popover's surface: width and anything else its content needs goes after it. */
export const popoverSurface = "elevated z-50 p-3 rc-in";

/** A small text field, per the Input recipe. */
export const inputClass =
  "h-8 rounded-md bg-fill-subtle px-3 text-callout text-fg placeholder:text-fg-faint hover:bg-fill focus:bg-fill";

export interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

// Radix keeps the empty string for "nothing selected", so an option for "" travels under this.
const EMPTY_OPTION = "__empty__";

/** One choice from a list: every picker that is not a `Segments` or the composer's own. */
export function Select({
  label,
  value,
  options,
  onChange,
  disabled,
  size = "md",
  className,
}: {
  label: string;
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  disabled?: boolean;
  size?: "sm" | "md";
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
          "inline-flex max-w-[260px] min-w-0 items-center gap-1.5 rounded-full bg-fill font-medium text-callout text-fg outline-none enabled:hover:bg-fill-strong disabled:opacity-40 data-[state=open]:bg-fill-strong",
          size === "sm" ? "h-6 pr-2 pl-2.5" : "h-[26px] pr-2.5 pl-3",
          className,
        )}
      >
        <span className="min-w-0 truncate">
          <RadixSelect.Value />
        </span>
        <RadixSelect.Icon className="shrink-0 text-fg-faint">
          <ChevronDown size={11} strokeWidth={2.5} />
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

export function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="rounded-xs bg-surface-3 px-[5px] py-px font-mono font-semibold text-caption text-fg-muted">
      {children}
    </kbd>
  );
}

/** A switch styled like the macOS one, small. */
export function Switch({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cx(
        "relative inline-flex h-[18px] w-8 shrink-0 items-center rounded-full disabled:opacity-40",
        checked ? "bg-accent" : "bg-fill-strong",
      )}
    >
      <span
        className={cx(
          "absolute top-[2px] size-[14px] rounded-full bg-accent-fg shadow-1 transition-transform duration-150 ease-snappy",
          checked ? "translate-x-[16px]" : "translate-x-[2px]",
        )}
      />
    </button>
  );
}

/** Segmented control: a track with a sliding thumb. */
export function Segments<T extends string>({
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
  // The selected segment's pill slides to the new one instead of jumping.
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
      className="relative inline-flex rounded-full bg-fill-subtle p-[2px]"
    >
      {thumb && (
        <span
          aria-hidden
          className="absolute top-0 left-0 h-5 rounded-full bg-surface-3 shadow-1 transition-[transform,width] duration-200 ease-snappy"
          style={{ transform: `translate(${thumb.left}px, 2px)`, width: thumb.width }}
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
            "relative h-5 rounded-full px-2.5 font-medium text-caption disabled:opacity-40",
            o.value === value
              ? cx("text-fg", !thumb && "bg-surface-3 shadow-1")
              : "text-fg-muted hover:text-fg",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** Copies to the clipboard and says so in a toast; a failure says that instead. */
export function copyText(text: string): Promise<void> {
  return navigator.clipboard.writeText(text).then(
    () => void toast("Copied", "success"),
    (error) => void toast(`Could not copy: ${errorText(error)}`, "error"),
  );
}

/**
 * The error card (docs/design.md): a neutral surface, the icon in the tone's
 * colour, the text muted, actions at the right. No coloured border or tint.
 */
export function Callout({
  tone = "danger",
  children,
  action,
  floating,
  className,
}: {
  tone?: "danger" | "warning";
  children: ReactNode;
  action?: ReactNode;
  /** Over the transcript (the composer stack): lifted with the first shadow. */
  floating?: boolean;
  className?: string;
}) {
  const Icon = tone === "danger" ? CircleAlert : TriangleAlert;
  return (
    <div
      role="alert"
      className={cx(
        "flex items-start gap-2.5 rounded-lg bg-surface px-3.5 py-3",
        floating && "shadow-1 rc-in",
        className,
      )}
    >
      <Icon size={14} className={cx("mt-px shrink-0", tone === "danger" ? "text-danger" : "text-warning")} />
      <div className="selectable line-clamp-6 min-w-0 flex-1 text-callout text-fg-muted">{children}</div>
      {action && <div className="-my-1 flex shrink-0 items-center gap-1.5">{action}</div>}
    </div>
  );
}
