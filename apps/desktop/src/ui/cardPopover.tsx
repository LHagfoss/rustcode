// The menus and panels the composer hangs off itself: the model pill's picker, the +, folder,
// permissions and meters menus. They are anchored to the composer card, not to the pill that
// opened them, so a panel always clears the card's own top edge with the 8px gap the design keeps
// between a surface and what it belongs to, and never lands 100px under the pill just because the
// window ran out of room above. Radix measures the room that is left and publishes it as
// `--radix-popper-available-height`, so a short window shrinks the panel instead of moving it.

import { Popover } from "radix-ui";
import {
  type ButtonHTMLAttributes,
  type CSSProperties,
  cloneElement,
  createContext,
  isValidElement,
  type ReactElement,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  useContext,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { cx, menuItem, surfaceCollisionPadding, Tooltip } from "./primitives";

/** Anything floating-ui can position against: here, the composer card's own box. */
interface Measurable {
  getBoundingClientRect(): DOMRect;
}

/** The gap between the composer card and anything hanging off it (docs/design.md › Composer). */
const CARD_GAP = 8;

export interface CardTriggerProps {
  onClick: (event: ReactMouseEvent<HTMLElement>) => void;
  "aria-haspopup": "menu" | "dialog";
  "aria-expanded": boolean;
  "data-state": "open" | "closed";
}

const CloseContext = createContext<() => void>(() => {});

/** Closes the panel a row sits in: what a row does after it has done its work. */
export function useCardPopoverClose() {
  return useContext(CloseContext);
}

/**
 * A panel anchored to the composer card. `trigger` is a plain element: it keeps its own look and
 * gets the click, ARIA and open state the panel needs put on it on the way through, and the
 * tooltip it would have had. When the panel closes, focus goes back to it.
 */
export function CardPopover({
  trigger,
  tooltip,
  children,
  align = "start",
  side = "top",
  className,
  style,
  label,
  role = "menu",
  open: controlledOpen,
  onOpenChange,
  onEscapeKeyDown,
  onKeyDown,
  onOpenAutoFocus,
  avoidCollisions = false,
}: {
  /** The pill that opens it. */
  trigger: ReactElement<CardTriggerProps & Record<string, unknown>>;
  tooltip?: ReactNode;
  children: ReactNode;
  align?: "start" | "center" | "end";
  side?: "top" | "bottom";
  className?: string;
  style?: CSSProperties;
  /** Names the panel for screen readers, where the trigger's own label is not enough. */
  label?: string;
  /** `menu` for a list of rows, `dialog` for a panel of controls. */
  role?: "menu" | "dialog";
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  onEscapeKeyDown?: (event: KeyboardEvent) => void;
  onKeyDown?: (event: ReactKeyboardEvent<HTMLDivElement>) => void;
  onOpenAutoFocus?: (event: Event) => void;
  /**
   * Off by default: a card menu stays above the card and shrinks to the room it has, rather than
   * flipping under the card to a place the user did not point at.
   */
  avoidCollisions?: boolean;
}) {
  const [uncontrolled, setUncontrolled] = useState(false);
  const open = controlledOpen ?? uncontrolled;
  const setOpen = (next: boolean) => {
    if (controlledOpen === undefined) setUncontrolled(next);
    onOpenChange?.(next);
  };
  // The card this pill sits in. Measured when the pill is pressed (and once on mount, for a panel
  // the app opens itself, such as `/model`), so it never disagrees with where the card is.
  const card = useRef<HTMLElement | null>(null);
  const pill = useRef<HTMLElement | null>(null);
  const findCard = (from?: Element | null) =>
    from?.closest<HTMLElement>(".rc-composer") ?? document.querySelector<HTMLElement>(".rc-composer");
  const anchor = useRef<Measurable>({
    getBoundingClientRect: () => card.current?.getBoundingClientRect() ?? new DOMRect(),
  });
  useLayoutEffect(() => {
    card.current ??= findCard(pill.current);
  });

  const element = isValidElement(trigger)
    ? cloneElement(trigger, {
        onClick: (event: ReactMouseEvent<HTMLElement>) => {
          pill.current = event.currentTarget;
          card.current = findCard(event.currentTarget);
          (trigger.props.onClick as CardTriggerProps["onClick"] | undefined)?.(event);
          setOpen(!open);
        },
        "aria-haspopup": role === "menu" ? "menu" : "dialog",
        "aria-expanded": open,
        "data-state": open ? "open" : "closed",
      } as CardTriggerProps & Record<string, unknown>)
    : trigger;

  return (
    <Popover.Root open={open} onOpenChange={setOpen} modal={false}>
      {tooltip ? (
        <Tooltip label={tooltip} side="bottom">
          {element}
        </Tooltip>
      ) : (
        element
      )}
      <Popover.Anchor virtualRef={anchor} />
      <Popover.Portal>
        <Popover.Content
          side={side}
          align={align}
          sideOffset={CARD_GAP}
          collisionPadding={surfaceCollisionPadding}
          avoidCollisions={avoidCollisions}
          role={role}
          aria-label={label}
          onEscapeKeyDown={onEscapeKeyDown}
          onKeyDown={onKeyDown}
          onOpenAutoFocus={onOpenAutoFocus}
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            pill.current?.focus();
          }}
          style={style}
          className={cx(
            "elevated z-40 max-h-[var(--radix-popper-available-height,80vh)] max-w-[calc(100vw-16px)] overflow-y-auto p-1.5 outline-none",
            className,
          )}
        >
          <CloseContext.Provider value={() => setOpen(false)}>{children}</CloseContext.Provider>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

/**
 * A row in a `CardPopover` menu: the shared menu row, closed by the panel after it runs.
 * `close={false}` keeps the panel open for a row that changes something in place (an effort step).
 */
export function CardMenuItem({
  onSelect,
  close = true,
  className,
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { onSelect: () => void; close?: boolean }) {
  const onClose = useCardPopoverClose();
  return (
    <button
      type="button"
      role="menuitem"
      data-menu-row=""
      onClick={() => {
        onSelect();
        if (close) onClose();
      }}
      className={cx(menuItem, className)}
      {...rest}
    >
      {children}
    </button>
  );
}

/**
 * Arrow keys, Home and End move focus down a panel's rows: the keyboard half of a menu that is a
 * popover rather than Radix's own menu. Enter and Space are the rows' own buttons.
 */
export function menuKeys(event: ReactKeyboardEvent<HTMLElement>) {
  const panel = event.currentTarget;
  const rows = [...panel.querySelectorAll<HTMLElement>("[data-menu-row], [role='option']")].filter(
    (row) => !row.hasAttribute("disabled") && !row.closest("[data-menu-static]"),
  );
  if (!rows.length) return;
  const current = document.activeElement;
  const at = current instanceof HTMLElement ? rows.indexOf(current) : -1;
  const focus = (index: number) => {
    const row = rows[(index + rows.length) % rows.length];
    if (!row) return;
    row.focus({ preventScroll: true });
    row.scrollIntoView({ block: "nearest" });
  };
  if (event.key === "ArrowDown") focus(at < 0 ? 0 : at + 1);
  else if (event.key === "ArrowUp") focus(at < 0 ? rows.length - 1 : at - 1);
  else if (event.key === "Home") focus(0);
  else if (event.key === "End") focus(rows.length - 1);
  else return;
  event.preventDefault();
}
