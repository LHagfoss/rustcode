// The menus and panels the composer hangs off itself: the model picker, the +, folder, permissions
// and usage menus. Each one is anchored to the control that opened it, 8px off that control's own
// edge (`CARD_GAP`), so a panel reads as coming out of the button pressed and not out of the card it
// happens to sit in. A panel opens `top` unless a menu asks for a side; only the alignment follows
// the control — `start` on the card's left half, `end` on its right — so a panel opens inward and
// stays inside the window. Collisions never move it down under its control; Radix still measures the
// room as `--radix-popper-available-height`, so a short window shrinks the panel instead of moving
// it. When the panel closes it hands focus back to its control, unless the user pressed elsewhere:
// that press already said where focus belongs (the field, another button).

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
  useEffect,
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

/** The room under the composer's strip a menu needs before it opens downward instead of up. */
const FOOTER_ROOM = 280;

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
  side,
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
  /**
   * Which side of the control the panel opens on. Set, it is pinned there; left unset, the composer
   * strip's menus open downward when the window has room under them and upward when it does not,
   * and every other panel opens `top`.
   */
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
  const [placed, setPlaced] = useState<"top" | "bottom">(side ?? "top");
  // Whether the press that closed the panel landed outside it: that press chose where focus goes,
  // and pulling it back to the pill would take the caret out of the field the user just clicked.
  const closedOutside = useRef(false);
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
    // A side that was asked for is where the panel stays.
    if (side) {
      setPlaced(side);
      return;
    }
    // The strip under the card opens downward when the window has room there, so its menus
    // never cover the field above; at the bottom of a thread there is none, and they open up.
    const inStrip = button.closest(".rc-composer-footer");
    setPlaced(inStrip && window.innerHeight - at.bottom >= FOOTER_ROOM ? "bottom" : "top");
  }, [open, side]);
  // Every panel opens fresh: the last one's outside press says nothing about this one.
  useEffect(() => {
    if (open) closedOutside.current = false;
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
          side={placed}
          align={align ?? aligned}
          sideOffset={CARD_GAP}
          collisionPadding={surfaceCollisionPadding}
          avoidCollisions={avoidCollisions}
          role={role}
          aria-label={label}
          onEscapeKeyDown={onEscapeKeyDown}
          onKeyDown={onKeyDown}
          onOpenAutoFocus={onOpenAutoFocus}
          onInteractOutside={(event) => {
            // Pressing the pill itself is not an outside press: Radix leaves the panel open for it
            // and the pill's own click closes it, focus landing where the press already went.
            const target = event.target as Node | null;
            if (target && pill.current?.contains(target)) return;
            closedOutside.current = true;
          }}
          onCloseAutoFocus={(event) => {
            // Never hand focus to the panel's own closing budget; this decides where it goes.
            event.preventDefault();
            if (closedOutside.current) {
              closedOutside.current = false;
              // The press that closed it usually focused what it landed on. A press that left
              // nothing focused (a platform that does not focus a clicked button) still needs the
              // pill, so the composer keeps a focus target.
              const active = document.activeElement;
              if (active && active !== document.body) return;
            }
            pill.current?.focus();
          }}
          style={style}
          className={cx(
            // No panel scrolls sideways: the drill motion slides a page a few px, and the card
            // must not grow a horizontal scrollbar for those 220ms.
            "elevated z-40 max-h-[var(--radix-popper-available-height,80vh)] max-w-[calc(100vw-16px)] overflow-x-hidden overflow-y-auto p-1.5 outline-none",
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
