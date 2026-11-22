// Small building blocks shared by every view, matching the SwiftUI app's
// ChromeButton, ToolbarCluster, PillButtonStyle and mini spinner.

import {
  ContextMenu as RadixContextMenu,
  DropdownMenu as RadixDropdownMenu,
  Select as RadixSelect,
  Tooltip as RadixTooltip,
} from "radix-ui";
import {
  type ButtonHTMLAttributes,
  type ComponentProps,
  type ComponentType,
  forwardRef,
  type KeyboardEvent,
  type ReactNode,
  type Ref,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { useHoverTip } from "../lib/hover";
import { useIntentRest } from "../lib/intent";
import { quickActionTarget } from "../lib/menuNavigation";
import { errorText, toast } from "../state/store";
import { Check, ChevronDown, CircleAlert, Search, TriangleAlert, X } from "./icons";

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

/**
 * A placeholder block where content will appear (docs/design.md, Loading):
 * `fill-subtle`, swept by a soft highlight that stops under reduced motion.
 * Size and radius come from the caller's classes.
 */
export function Skeleton({ className, style }: { className?: string; style?: React.CSSProperties }) {
  return <span aria-hidden className={cx("rc-skeleton block", className)} style={style} />;
}

/**
 * The surface a tip sits on, tooltip or hover card alike (docs/design.md › Tooltips and hover
 * cards): the `elevated` glass and its hairline, the `xl` radius every popover and menu takes, and
 * the tooltip's own text size. A tip adds its inset and width — `px-2 py-1` for one line,
 * `p-3` for a hover card — and `pointer-events-none` unless it has controls in it.
 */
export const tipSurface = "elevated rounded-xl text-callout text-fg";

export function Tooltip({
  label,
  children,
  side = "top",
}: {
  label?: ReactNode;
  children: ReactNode;
  side?: "top" | "bottom" | "left" | "right";
}) {
  // The app's own timing (lib/hover.ts) rather than the provider's: it is the same rest for every
  // tip, hands the place to a neighbour at once, and a press or a scroll puts the tip away.
  const tip = useHoverTip();
  if (!label) return <>{children}</>;
  return (
    <RadixTooltip.Root open={tip.open} disableHoverableContent>
      <RadixTooltip.Trigger asChild {...tip.triggerProps}>
        {children}
      </RadixTooltip.Trigger>
      <RadixTooltip.Portal>
        <RadixTooltip.Content
          side={side}
          sideOffset={8}
          collisionPadding={surfaceCollisionPadding}
          className={cx(tipSurface, "pointer-events-none z-50 max-w-80 px-2 py-1")}
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
        size === "lg" ? "h-[30px] px-4" : size === "sm" ? "h-6 px-2" : "h-[26px] px-3",
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

/**
 * The spacing scale (docs/design.md › Spacing), as the insets the surfaces share: a card's own
 * padding, a list row inside one, and a menu's padding around its items. Text starts on the same
 * 16px line in all three (`rowInset` is the card's inline inset plus its own), so a card, a settings
 * row and a menu row line up; components name the inset rather than repeat the numbers.
 */
export const cardInset = "p-4";
export const rowInset = "px-4 py-3";
export const menuInset = "p-2";

/**
 * A menu's surface and rows, for `DropdownMenu` and `ContextMenu` alike. The surface never runs
 * past the window (16px at every edge) and takes at most the room it has, scrolling instead of
 * landing half off-screen: Radix publishes that room as `--radix-popper-available-height`.
 * Opening and closing motion comes from the one rule for portal surfaces in `index.css`
 * (`rc-pop` from the transform origin), so the surface itself carries no animation class.
 */
export const menuSurface = `elevated rounded-2xl z-50 max-h-[var(--radix-popper-available-height,80vh)] min-w-[200px] max-w-[calc(100vw-16px)] overflow-y-auto ${menuInset}`;
export const menuItem =
  "flex min-h-7 w-full cursor-pointer items-center gap-2 rounded-xl px-2 py-1 text-left text-body outline-none data-[disabled]:cursor-default data-[disabled]:opacity-40 data-[highlighted]:bg-fill";
export const menuItemDanger = "text-danger data-[highlighted]:bg-danger/12";
export const menuSeparator = "mx-2 my-1 h-px shrink-0 bg-separator";
export const menuLabel = "px-2 pt-1.5 pb-1 text-caption text-fg-faint";

export interface MenuQuickAction {
  label: string;
  /** The full action, when the square's short label needs context. */
  description?: string;
  icon: ReactNode;
  onSelect: () => void;
  disabled?: boolean;
  checked?: boolean;
}

/** Works with either Radix menu's Item or a CardMenuItem, retaining its activation and close behavior. */
export function MenuQuickActions({
  Item,
  actions,
}: {
  Item: ComponentType<{
    children: ReactNode;
    className: string;
    onSelect: () => void;
    disabled?: boolean;
    role?: "menuitem" | "menuitemcheckbox";
    "aria-label"?: string;
    "aria-checked"?: boolean;
  }>;
  actions: MenuQuickAction[];
}) {
  if (!actions.length) return null;
  return (
    <div
      role="group"
      aria-label="Quick actions"
      data-menu-quick-actions=""
      className="mb-1 grid shrink-0 gap-1"
      style={{ gridTemplateColumns: `repeat(${actions.length}, minmax(0, 1fr))` }}
    >
      {actions.map((action) => (
        <Tooltip key={action.label} label={action.description ?? action.label}>
          <Item
            role={action.checked === undefined ? "menuitem" : "menuitemcheckbox"}
            aria-checked={action.checked}
            aria-label={action.description ?? action.label}
            disabled={action.disabled}
            onSelect={action.onSelect}
            className={cx(
              "flex aspect-square min-w-0 cursor-pointer flex-col items-center justify-center gap-1 rounded-xl p-1 text-caption text-fg outline-none hover:bg-fill focus-visible:bg-fill active:bg-fill-strong data-[highlighted]:bg-fill data-[disabled]:cursor-default data-[disabled]:opacity-40 disabled:cursor-default disabled:opacity-40 disabled:hover:bg-fill-subtle [&_svg]:size-4 [&_svg]:shrink-0",
              action.checked ? "bg-fill-strong" : "bg-fill-subtle",
            )}
          >
            <span aria-hidden>{action.icon}</span>
            <span className="w-full truncate text-center">{action.label}</span>
          </Item>
        </Tooltip>
      ))}
    </div>
  );
}

/** Leave submenu keys, typeahead, Home/End, activation and dismissal to the menu implementation. */
export function menuQuickActionKeys(event: KeyboardEvent<HTMLElement>): boolean {
  if (event.defaultPrevented) return false;
  const panel = event.currentTarget;
  const row = panel.querySelector<HTMLElement>("[data-menu-quick-actions]");
  if (!row) return false;
  const items = [...panel.querySelectorAll<HTMLElement>('[role^="menuitem"]')].filter(
    (item) =>
      !item.hasAttribute("disabled") &&
      !item.hasAttribute("data-disabled") &&
      item.closest('[role="menu"]') === panel,
  );
  const quick = items.filter((item) => row.contains(item));
  const target = quickActionTarget(
    event.key,
    items.indexOf(document.activeElement as HTMLElement),
    quick.length,
    items.length,
  );
  if (target === null) return false;
  event.preventDefault();
  event.stopPropagation();
  items[target]?.focus();
  return true;
}

/**
 * A row's trailing shortcut (⌘K, ⏎), pushed to the far edge of its menu row. One spelling of it
 * everywhere, so the alignment matches from menu to menu.
 */
export function MenuShortcut({ children }: { children: ReactNode }) {
  return <kbd className="ms-auto shrink-0 pl-3 font-sans text-caption text-fg-faint">{children}</kbd>;
}

/** The `collisionPadding` every floating surface shares: 8px clear of the window's edges. */
export const surfaceCollisionPadding = 8;

/**
 * A menu's content with the shared defaults: 8px from the window's edges, one surface, and its own
 * portal. Call sites pass alignment and anything the content needs on top.
 */
export function MenuContent({
  align = "start",
  sideOffset = 6,
  className,
  children,
  onKeyDownCapture,
  ...rest
}: ComponentProps<typeof RadixDropdownMenu.Content>) {
  return (
    <RadixDropdownMenu.Portal>
      <RadixDropdownMenu.Content
        align={align}
        sideOffset={sideOffset}
        collisionPadding={surfaceCollisionPadding}
        className={cx(menuSurface, className)}
        {...rest}
        onKeyDownCapture={(event) => {
          onKeyDownCapture?.(event);
          menuQuickActionKeys(event);
        }}
      >
        {children}
      </RadixDropdownMenu.Content>
    </RadixDropdownMenu.Portal>
  );
}

/** `MenuContent` for a right-click menu, with the same margin from the window's edges. */
export function ContextMenuSurface({
  className,
  children,
  onKeyDownCapture,
  ...rest
}: ComponentProps<typeof RadixContextMenu.Content>) {
  return (
    <RadixContextMenu.Portal>
      <RadixContextMenu.Content
        collisionPadding={surfaceCollisionPadding}
        className={cx(menuSurface, className)}
        {...rest}
        onKeyDownCapture={(event) => {
          onKeyDownCapture?.(event);
          menuQuickActionKeys(event);
        }}
      >
        {children}
      </RadixContextMenu.Content>
    </RadixContextMenu.Portal>
  );
}

/**
 * The search field every searchable popup opens with (the model browser, the project menu, the
 * sidebar's filter): one height, one icon slot, one placeholder colour.
 */
export function MenuSearchField({
  value,
  onChange,
  placeholder,
  label,
  inputRef,
  onKeyDown,
  onBlur,
  busy,
  controls,
  activeDescendant,
  onClear,
  size = "md",
  className,
}: {
  value: string;
  onChange: (value: string) => void;
  placeholder: string;
  label: string;
  inputRef?: Ref<HTMLInputElement>;
  onKeyDown?: (event: KeyboardEvent<HTMLInputElement>) => void;
  onBlur?: () => void;
  busy?: boolean;
  controls?: string;
  activeDescendant?: string;
  /** A trailing clear button, for a field that stays on screen after it is filled. */
  onClear?: () => void;
  /** `sm` for a field in a toolbar row (the sidebar), `md` in a panel. */
  size?: "sm" | "md";
  className?: string;
}) {
  return (
    <div
      className={cx(
        "flex min-w-0 items-center gap-1.5 rounded-md bg-fill-subtle px-2 focus-within:bg-fill",
        size === "sm" ? "h-7" : "h-8",
        className,
      )}
    >
      {busy ? (
        <Spinner size={13} className="shrink-0" />
      ) : (
        <Search size={size === "sm" ? 12 : 13} className="shrink-0 text-fg-faint" />
      )}
      <input
        ref={inputRef}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
        onBlur={onBlur}
        placeholder={placeholder}
        aria-label={label}
        aria-controls={controls}
        aria-activedescendant={activeDescendant}
        {...noTextHelpers}
        className={cx(
          "min-w-0 flex-1 bg-transparent text-fg outline-none placeholder:text-fg-faint",
          size === "sm" ? "text-callout" : "text-body",
        )}
      />
      {onClear && value && (
        <button
          type="button"
          aria-label={`Clear ${label.toLowerCase()}`}
          onClick={onClear}
          className="shrink-0 text-fg-faint hover:text-fg-muted"
        >
          <X size={12} />
        </button>
      )}
    </div>
  );
}

/**
 * What every field the user types in turns off: the system's text helpers. Autocorrect, automatic
 * capitalisation and completions have no place in a prompt, a path, a branch name or a search
 * term — they rewrite what the user meant — so each `<input>` and `<textarea>` the app draws
 * spreads this (docs/desktop.md › Native text fields, for the layers below it).
 */
export const noTextHelpers = {
  spellCheck: false,
  autoCorrect: "off",
  autoCapitalize: "off",
  autoComplete: "off",
} as const;

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
          size === "sm" ? "h-6 pr-2 pl-2" : "h-[26px] pr-2 pl-3",
          className,
        )}
      >
        <span className="min-w-0 truncate">
          <RadixSelect.Value />
        </span>
        <RadixSelect.Icon className="shrink-0 text-fg-faint">
          <ChevronDown size={12} strokeWidth={2.5} />
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

export function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="rounded-xs bg-surface-3 px-[5px] py-px font-mono font-semibold text-caption text-fg-muted">
      {children}
    </kbd>
  );
}

/**
 * A switch: a pill knob as tall as its track (docs/design.md › Active controls), so the
 * button's own box is the track's 20 px and the row it sits in does not grow.
 */
/** How far the switch's knob travels, in px: the track's width less the knob's and both insets. */
const SWITCH_TRAVEL = 16;

/**
 * A switch. A press toggles it; the knob can also be dragged across and let go on either side.
 * Held down, the knob stretches toward the side it would move to, so the press reads before the
 * release. Keyboard Space and Enter toggle it as a button does.
 */
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
  const [pressed, setPressed] = useState(false);
  const [dragOffset, setDragOffset] = useState<number | null>(null);
  const gesture = useRef<{ x: number; moved: boolean } | null>(null);
  const end = (commit: boolean, x?: number) => {
    const g = gesture.current;
    gesture.current = null;
    setPressed(false);
    setDragOffset(null);
    if (!commit || !g) return;
    const next = g.moved && x !== undefined ? knobAt(x, g.x, checked) > SWITCH_TRAVEL / 2 : !checked;
    if (next !== checked) onChange(next);
  };
  const offset = dragOffset ?? (checked ? SWITCH_TRAVEL : 0);
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={(e) => {
        if (e.detail === 0) onChange(!checked);
      }}
      onPointerDown={(e) => {
        if (e.button !== 0 || disabled) return;
        e.currentTarget.setPointerCapture(e.pointerId);
        gesture.current = { x: e.clientX, moved: false };
        setPressed(true);
      }}
      onPointerMove={(e) => {
        const g = gesture.current;
        if (!g) return;
        if (!g.moved && Math.abs(e.clientX - g.x) < 3) return;
        g.moved = true;
        setDragOffset(knobAt(e.clientX, g.x, checked));
      }}
      onPointerUp={(e) => end(true, e.clientX)}
      onPointerCancel={() => end(false)}
      onLostPointerCapture={() => gesture.current && end(false)}
      className={cx(
        "relative inline-flex h-5 w-11 shrink-0 touch-none items-center rounded-full transition-[background-color] duration-200 ease-snappy disabled:opacity-40",
        (dragOffset === null ? checked : dragOffset > SWITCH_TRAVEL / 2) ? "bg-accent" : "bg-fill-strong",
      )}
    >
      <span
        className={cx(
          "absolute top-0.5 left-0.5 h-4 rounded-full bg-knob shadow-knob ease-snappy",
          pressed ? "w-7" : "w-6",
          dragOffset === null
            ? "transition-[transform,width] duration-200"
            : "transition-[width] duration-150",
        )}
        style={{ transform: `translateX(${pressed ? offset - (4 * offset) / SWITCH_TRAVEL : offset}px)` }}
      />
    </button>
  );
}

/** The knob's offset for a pointer at `x` that went down at `start`, clamped to the track. */
function knobAt(x: number, start: number, checked: boolean): number {
  return Math.min(SWITCH_TRAVEL, Math.max(0, (checked ? SWITCH_TRAVEL : 0) + x - start));
}

/**
 * Segmented control: a track with a sliding thumb. The track fits its segments (`w-fit`), so a flex
 * column that stretches its children never leaves the buttons at one end of a full-width track;
 * `fill` is for a track that spans its card, where the segments share the width equally.
 * `tabs` makes it a tab strip for the views of a panel: tab roles, panel-sized labels, and a quiet
 * thumb on the soft fill instead of the white knob, so a secondary surface does not outshine the chat.
 * On a track (not a tab strip) a press swells the thumb, which follows the pointer across the
 * segments and picks the one it is over on release; a click from the keyboard picks at once.
 */
export function Segments<T extends string>({
  value,
  options,
  onChange,
  label,
  onIntent,
  fill,
  tabs,
  size = "sm",
}: {
  value: T;
  options: { value: T; label: string; disabled?: boolean }[];
  onChange: (value: T) => void;
  label: string;
  /** A rest of the pointer or focus on an option: the screen may read ahead for that choice. */
  onIntent?: (value: T) => void;
  /** Span the container, the segments sharing the width equally (a card's own toggle). */
  fill?: boolean;
  /** A tab strip: the id of the panel the tabs control. */
  tabs?: { controls: string };
  /** `md` is a settings row's control (24px segments), `sm` a toolbar's (20px). */
  size?: "sm" | "md";
}) {
  const height = size === "md" ? "h-6" : "h-5";
  // The selected segment's pill slides to the new one instead of jumping.
  const group = useRef<HTMLDivElement>(null);
  const [thumb, setThumb] = useState<{ left: number; width: number } | null>(null);
  const intent = useIntentRest((next: T) => onIntent?.(next));
  const [held, setHeld] = useState<T | null>(null);
  const pressing = useRef(false);
  const shown = held ?? value;
  // biome-ignore lint/correctness/useExhaustiveDependencies: re-measures when the choice or labels change
  useLayoutEffect(() => {
    const el = group.current?.querySelector<HTMLElement>("[data-on]");
    setThumb(el ? { left: el.offsetLeft, width: el.offsetWidth } : null);
  }, [shown, options.map((o) => o.label).join()]);
  const optionAt = (x: number): T | undefined => {
    const buttons = group.current?.querySelectorAll<HTMLButtonElement>("button[data-value]") ?? [];
    let best: { value: T; distance: number } | undefined;
    for (const button of buttons) {
      if (button.disabled) continue;
      const r = button.getBoundingClientRect();
      const distance = x < r.left ? r.left - x : x > r.right ? x - r.right : 0;
      if (!best || distance < best.distance) best = { value: button.dataset.value as T, distance };
    }
    return best?.value;
  };
  const release = (commit: boolean) => {
    if (!pressing.current) return;
    pressing.current = false;
    if (commit && held !== null && held !== value) onChange(held);
    setHeld(null);
  };
  // The pick's label sits on the white pill, so it takes the pill's dark fg. Before the first
  // measure the button draws the pill itself; a measured pill with no width (a track measured while
  // hidden) draws none, so its label keeps the track's own fg instead of dark on dark.
  const selected = tabs
    ? cx("text-fg", !thumb && "bg-fill")
    : thumb
      ? thumb.width > 0
        ? "text-knob-fg"
        : "text-fg"
      : "bg-knob text-knob-fg shadow-knob";
  return (
    <div
      ref={group}
      {...{ role: tabs ? "tablist" : "radiogroup", "aria-label": label }}
      {...(!tabs && {
        onPointerDown: (e: React.PointerEvent<HTMLDivElement>) => {
          const at = e.button === 0 ? optionAt(e.clientX) : undefined;
          if (at === undefined) return;
          e.currentTarget.setPointerCapture(e.pointerId);
          pressing.current = true;
          setHeld(at);
        },
        onPointerMove: (e: React.PointerEvent<HTMLDivElement>) => {
          if (!pressing.current) return;
          const at = optionAt(e.clientX);
          if (at !== undefined && at !== held) setHeld(at);
        },
        onPointerUp: () => release(true),
        onPointerCancel: () => release(false),
        onLostPointerCapture: () => release(false),
      })}
      className={cx(
        "relative",
        tabs ? "gap-1" : "touch-none rounded-full bg-fill p-[3px]",
        fill ? "flex w-full" : "inline-flex w-fit shrink-0",
      )}
    >
      {thumb && (
        <span
          aria-hidden
          className={cx(
            "absolute top-0 left-0 transition-[translate,width,scale] duration-200 ease-snappy",
            tabs ? "h-7 rounded-lg bg-fill" : cx(height, "rounded-full bg-knob shadow-knob"),
            held !== null && "scale-[1.04]",
          )}
          style={{ translate: `${thumb.left}px ${tabs ? 0 : 3}px`, width: thumb.width }}
        />
      )}
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          data-value={o.value}
          data-on={o.value === shown || undefined}
          {...(tabs
            ? { role: "tab", "aria-selected": o.value === value, "aria-controls": tabs.controls }
            : { role: "radio", "aria-checked": o.value === value })}
          disabled={o.disabled}
          onClick={(e) => {
            if (tabs || e.detail === 0) onChange(o.value);
          }}
          onPointerEnter={() => intent.start(o.value)}
          onPointerLeave={intent.cancel}
          onFocus={() => intent.start(o.value)}
          onBlur={intent.cancel}
          className={cx(
            "relative whitespace-nowrap font-medium disabled:opacity-40",
            tabs ? "h-7 rounded-lg px-3 text-callout" : cx(height, "rounded-full px-2 text-caption"),
            fill && "min-w-0 flex-1",
            o.value === shown ? selected : "text-fg-muted hover:text-fg",
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
        "flex items-start gap-2 rounded-lg bg-surface",
        rowInset,
        floating && "shadow-1 rc-in",
        className,
      )}
    >
      <Icon size={14} className={cx("mt-px shrink-0", tone === "danger" ? "text-danger" : "text-warning")} />
      <div className="selectable line-clamp-6 min-w-0 flex-1 text-callout text-fg-muted">{children}</div>
      {/* Centred on the message, however many lines it takes; the buttons take the step below the
          callout's `rounded-lg` (docs/design.md › Radius), not pills against its corners. */}
      {action && (
        <div className="-my-1 flex shrink-0 items-center gap-1.5 self-center [&>button]:rounded-md">
          {action}
        </div>
      )}
    </div>
  );
}
