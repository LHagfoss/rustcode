import { useCallback, useLayoutEffect, useState } from "react";
import { observeSize } from "../lib/resize";

/**
 * The gap a composer popover needs to clear the card above its trigger. The trigger sits in the
 * card's control row, so a popover opened `side="top"` with a fixed offset lands on the card's
 * own glass (and the card's edge and controls show through). Measure from the trigger's top to
 * the card's top, add the 8px gap the design keeps between a surface and its trigger, and keep
 * it current as the card grows (attachments, a taller field).
 *
 * The trigger is a callback ref rather than an object ref: some triggers render only once their
 * data is in (the model pill shows a skeleton first), so the offset is measured when the button
 * actually mounts, not on the hook's first render.
 */
export function useComposerPopoverOffset() {
  const [trigger, setTrigger] = useState<HTMLButtonElement | null>(null);
  const [offset, setOffset] = useState(8);
  const triggerRef = useCallback((el: HTMLButtonElement | null) => setTrigger(el), []);

  useLayoutEffect(() => {
    if (!trigger) return;
    const card = trigger.closest<HTMLElement>(".rc-composer");
    if (!card) return;
    const measure = () => {
      const above = trigger.getBoundingClientRect().top - card.getBoundingClientRect().top;
      setOffset(Math.max(8, Math.round(above) + 8));
    };
    measure();
    return observeSize(card, measure);
  }, [trigger]);

  return { triggerRef, sideOffset: offset };
}

/**
 * The collision padding every composer popover and menu shares: clear of the header (42px plus a
 * 16px gap) at the top, and a 12px gutter from the other viewport edges, so a tall panel flips or
 * slides rather than running off the screen.
 */
export const composerPopoverCollisionPadding = { top: 58, left: 12, right: 12, bottom: 12 };
