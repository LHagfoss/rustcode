import { type ReactNode, useEffect, useState } from "react";
import { cx } from "./primitives";

/**
 * Rows that unfold with a height transition and fold back before they unmount. Folded rows are
 * not rendered at all, so a long list costs nothing until opened. Opening waits two frames, so
 * the folded state is painted before the unfold starts; the clip lifts, and a closed fold
 * unmounts, when its height transition ends. An open fold is not clipped, so a dragged row can
 * travel past its last row.
 */
/** When a fold settles even if no `transitionend` arrives (a hidden parent, a toggle mid-frame). */
const FOLD_SETTLE_MS = 400;

export function Fold({
  open,
  children,
  className,
}: {
  open: boolean;
  children: ReactNode;
  className?: string;
}) {
  const [mounted, setMounted] = useState(open);
  const [shown, setShown] = useState(open);
  const [still, setStill] = useState(open);
  useEffect(() => {
    if (open) {
      setMounted(true);
      let frame = requestAnimationFrame(() => {
        frame = requestAnimationFrame(() => setShown(true));
      });
      return () => cancelAnimationFrame(frame);
    }
    setStill(false);
    setShown(false);
    const settle = setTimeout(() => setMounted(false), FOLD_SETTLE_MS);
    return () => clearTimeout(settle);
  }, [open]);
  useEffect(() => {
    if (!open || !shown) return;
    const settle = setTimeout(() => setStill(true), FOLD_SETTLE_MS);
    return () => clearTimeout(settle);
  }, [open, shown]);
  if (!mounted) return null;
  const clipped = !still || !shown;
  return (
    <div
      data-fold={shown ? "open" : "closed"}
      className={cx(
        "grid grid-cols-[minmax(0,1fr)] transition-[grid-template-rows,opacity] duration-200 ease-snappy",
        clipped && "overflow-hidden",
      )}
      style={{ gridTemplateRows: shown ? "1fr" : "0fr", opacity: shown ? 1 : 0 }}
      onTransitionEnd={(e) => {
        if (e.target !== e.currentTarget || e.propertyName !== "grid-template-rows") return;
        if (open && shown) setStill(true);
        else setMounted(false);
      }}
    >
      <div className={cx("min-h-0 min-w-0", clipped && "overflow-hidden", className)}>{children}</div>
    </div>
  );
}
