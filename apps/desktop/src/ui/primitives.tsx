// Small building blocks shared by every view, matching the SwiftUI app's
// ChromeButton, ToolbarCluster, PillButtonStyle and mini spinner.

import { CircleAlert, TriangleAlert } from "lucide-react";
import { Tooltip as RadixTooltip } from "radix-ui";
import { type ButtonHTMLAttributes, forwardRef, type ReactNode } from "react";

export function cx(...parts: (string | false | null | undefined)[]): string {
  return parts.filter(Boolean).join(" ");
}

/** The running indicator: a 12px arc trimmed to 70%, one turn per second. */
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

/** The SwiftUI app's PillButtonStyle: callout medium, height 26, capsule. */
export const Pill = forwardRef<
  HTMLButtonElement,
  ButtonHTMLAttributes<HTMLButtonElement> & { prominent?: boolean; danger?: boolean; size?: "md" | "lg" }
>(function Pill({ prominent, danger, size = "md", className, children, ...rest }, ref) {
  return (
    <button
      ref={ref}
      type="button"
      className={cx(
        "inline-flex shrink-0 items-center justify-center gap-1.5 rounded-full px-3 font-medium text-callout whitespace-nowrap disabled:opacity-40",
        size === "lg" ? "h-[30px] px-3.5" : "h-[26px]",
        prominent
          ? "bg-fg text-bg enabled:hover:bg-fg/90 enabled:active:bg-fg/80"
          : danger
            ? "bg-danger/12 text-danger enabled:hover:bg-danger/18 enabled:active:bg-danger/24"
            : "bg-fill text-fg enabled:hover:bg-fill-strong enabled:active:bg-fill-strong",
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
});

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
  return (
    <div role="radiogroup" aria-label={label} className="inline-flex rounded-full bg-fill-subtle p-[2px]">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={o.value === value}
          disabled={o.disabled}
          onClick={() => onChange(o.value)}
          className={cx(
            "h-5 rounded-full px-2.5 font-medium text-caption disabled:opacity-40",
            o.value === value ? "bg-surface-3 text-fg shadow-1" : "text-fg-muted hover:bg-fill hover:text-fg",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function copyText(text: string): Promise<void> {
  return navigator.clipboard.writeText(text);
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
