// One section of the right panel's overview (ui/ThreadDetails.tsx): a header
// that folds it away — its name, its count, and the actions that belong to it —
// over a body that holds the rows. Sections are told apart by the room between
// them, not by a card or a line: the panel is one column, and the header alone
// says where one section ends (docs/design.md › Borders, Spacing).

import { ChevronRight } from "lucide-react";
import type { ComponentProps, ReactNode } from "react";
import type { OverviewSection as SectionId } from "./overview";
import { cx, Tooltip } from "./primitives";

/** The header's count, in the quiet size the section headings use: `Pull requests 3`. */
export function SectionCount({ count }: { count: number }) {
  return (
    <span className="shrink-0 text-caption text-fg-faint tabular" aria-hidden>
      {count}
    </span>
  );
}

/**
 * The one shape of a section header's act: a 24px icon button that answers the
 * pointer with a quiet fill. A button that opens a menu takes this class itself,
 * so the menu's trigger is the button and not a tip wrapped around one.
 */
export const sectionIconButton =
  "flex size-6 shrink-0 items-center justify-center rounded-md text-fg-muted enabled:hover:bg-fill enabled:hover:text-fg enabled:active:bg-fill-strong disabled:opacity-40";

/** A 24px act in a section's header: `+`, the grid toggle, the fold to the full log. */
export function SectionButton({
  label,
  children,
  ...rest
}: { label: string } & Omit<ComponentProps<"button">, "aria-label">) {
  return (
    <Tooltip label={label}>
      <button type="button" aria-label={label} className={sectionIconButton} {...rest}>
        {children}
      </button>
    </Tooltip>
  );
}

export function OverviewSection({
  id,
  title,
  count,
  actions,
  folded,
  onToggle,
  children,
}: {
  id: SectionId;
  title: string;
  /** What the section holds, when it is worth counting (a fold with nothing in it still says 0). */
  count?: number;
  /** Right-aligned acts of its own; they stay reachable while the section is folded. */
  actions?: ReactNode;
  folded: boolean;
  onToggle: () => void;
  children: ReactNode;
}) {
  const body = `overview-${id}-body`;
  return (
    <section aria-label={title} data-section={id}>
      <div className="flex h-8 items-center gap-1">
        <button
          type="button"
          onClick={onToggle}
          aria-expanded={!folded}
          aria-controls={body}
          className="flex h-8 min-w-0 flex-1 items-center gap-1.5 rounded-md px-2 text-left hover:bg-fill-subtle"
        >
          <ChevronRight
            size={11}
            strokeWidth={2.5}
            className={cx("shrink-0 text-fg-faint transition-transform duration-150", !folded && "rotate-90")}
          />
          <span className="truncate font-medium text-fg-muted text-sub">{title}</span>
          {count !== undefined && <SectionCount count={count} />}
        </button>
        {actions && <div className="flex shrink-0 items-center gap-0.5">{actions}</div>}
      </div>
      {!folded && (
        <div id={body} className="pt-1 pb-0.5">
          {children}
        </div>
      )}
    </section>
  );
}
