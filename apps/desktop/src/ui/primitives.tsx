// Small building blocks shared by every view, matching the SwiftUI app's
// ChromeButton, ToolbarCluster, PillButtonStyle and mini spinner.

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
      className={cx("shrink-0 text-fg-2", className)}
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
  side = "bottom",
}: {
  label?: ReactNode;
  children: ReactNode;
  side?: "top" | "bottom" | "left" | "right";
}) {
  if (!label) return <>{children}</>;
  return (
    <RadixTooltip.Root delayDuration={600}>
      <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
      <RadixTooltip.Portal>
        <RadixTooltip.Content
          side={side}
          sideOffset={6}
          className="z-50 max-w-80 rounded-md surface px-2 py-1 text-callout text-fg shadow-float rc-in"
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
          "inline-flex h-7 w-[30px] items-center justify-center rounded-[7px] text-fg-2 transition-colors duration-100 hover:bg-fill-2 hover:text-fg disabled:opacity-40 disabled:hover:bg-transparent",
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
    <div className={cx("inline-flex h-7 items-center rounded-full bg-fill-3 px-[3px]", className)}>
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
            "inline-flex h-6 w-8 items-center justify-center rounded-full text-fg-2 transition-colors duration-100 hover:bg-fill-1 disabled:opacity-40",
            on && "bg-fill-1 text-brand",
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
        "inline-flex shrink-0 items-center justify-center gap-1.5 rounded-full px-3 font-medium text-callout whitespace-nowrap transition-[opacity,background-color] duration-100 active:opacity-70 disabled:opacity-45 disabled:active:opacity-45",
        size === "lg" ? "h-[30px] px-3.5" : "h-[26px]",
        prominent
          ? "bg-brand-fill text-white hover:brightness-110"
          : danger
            ? "bg-danger/12 text-danger hover:bg-danger/18"
            : "bg-fill-2 text-fg hover:bg-fill-1",
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
    <kbd className="rounded-[4px] bg-fill-2 px-[5px] py-px font-mono font-semibold text-caption text-fg-2">
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
        "relative inline-flex h-[18px] w-8 shrink-0 items-center rounded-full transition-colors duration-150 disabled:opacity-40",
        checked ? "bg-brand-fill" : "bg-fill-1",
      )}
    >
      <span
        className={cx(
          "absolute top-[2px] size-[14px] rounded-full bg-white shadow-[0_1px_2px_rgb(0_0_0/0.3)] transition-transform duration-150 ease-snappy",
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
    <div role="radiogroup" aria-label={label} className="inline-flex rounded-full bg-fill-3 p-[2px]">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={o.value === value}
          disabled={o.disabled}
          onClick={() => onChange(o.value)}
          className={cx(
            "h-5 rounded-full px-2.5 font-medium text-caption transition-colors duration-150 disabled:opacity-40",
            o.value === value ? "bg-fill-1 text-fg" : "text-fg-2 hover:text-fg",
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
