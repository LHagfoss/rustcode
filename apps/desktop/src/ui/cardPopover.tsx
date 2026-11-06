// The menus and panels the composer hangs off itself: the model picker, the +, folder, permissions
// and usage menus. Each one is anchored to the control that opened it, 8px above that control's own
// top edge (`CARD_GAP`), so a panel reads as coming out of the button pressed and not out of the
// card it happens to sit in. The side is pinned to `top`; only the alignment follows the control —
// `start` on the card's left half, `end` on its right — so a panel opens inward and stays inside the
// window. Collisions never move it down under its control; Radix still measures the room above as
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
  type Ref,
  useCallback,
  useContext,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { cx, menuItem, surfaceCollisionPadding, Tooltip } from "./primitives";

/** Anything floating-ui can position against: here, the control that opens a panel. */
interface Measurable {
  getBoundingClientRect(): DOMRect;
}

/** The gap between a control and its panel, and between the card and what hangs off it
 * (docs/design.md › Composer). */
const CARD_GAP = 8;

/** The composer card a control sits in: the one surface its panels relate to. */
function findCard(from?: Element | null): HTMLElement | null {
  return from?.closest<HTMLElement>(".rc-composer") ?? document.querySelector<HTMLElement>(".rc-composer");
}

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
  align,
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
  /** The pill that opens it: the panel hangs off this control's own box. */
  trigger: ReactElement<CardTriggerProps & Record<string, unknown>>;
  tooltip?: ReactNode;
  children: ReactNode;
  /** Left unset, the panel opens on the side of its control: inward from the window's edge. */
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
   * Off by default: a card menu stays above its control and shrinks to the room it has, rather than
   * flipping under the control to a place the user did not point at.
   */
  avoidCollisions?: boolean;
}) {
  const [uncontrolled, setUncontrolled] = useState(false);
  const open = controlledOpen ?? uncontrolled;
  const setOpen = (next: boolean) => {
    if (controlledOpen === undefined) setUncontrolled(next);
    onOpenChange?.(next);
  };
  // The control this panel hangs off, and the card it sits in: the pill gives the position, the
  // card which side of itself the panel should open on.
  const card = useRef<HTMLElement | null>(null);
  const pill = useRef<HTMLElement | null>(null);
  const [aligned, setAligned] = useState<"start" | "end">("start");
  const anchor = useRef<Measurable>({
    getBoundingClientRect: () => pill.current?.getBoundingClientRect() ?? new DOMRect(),
  });
  // The trigger's own ref, kept: a panel the app opens itself (`/model`) has no press to measure
  // from, so the node has to be known from the moment it mounts.
  const ownRef = (trigger.props as { ref?: Ref<HTMLElement> }).ref;
  const attachPill = useCallback(
    (node: HTMLElement | null) => {
      pill.current = node;
      card.current = node ? findCard(node) : null;
      if (typeof ownRef === "function") ownRef(node);
      else if (ownRef) (ownRef as { current: HTMLElement | null }).current = node;
    },
    [ownRef],
  );
  // Which way the panel grows: away from the window's edge, from the card's own middle.
  useLayoutEffect(() => {
    if (!open) return;
    const button = pill.current;
    const box = card.current ?? findCard(button);
    if (!button || !box) return;
    const at = button.getBoundingClientRect();
    const inCard = box.getBoundingClientRect();
    setAligned(at.left + at.width / 2 > inCard.left + inCard.width / 2 ? "end" : "start");
  }, [open]);

  const element = isValidElement(trigger)
    ? cloneElement(trigger, {
        ref: attachPill,
        onClick: (event: ReactMouseEvent<HTMLElement>) => {
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
          align={align ?? aligned}
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
