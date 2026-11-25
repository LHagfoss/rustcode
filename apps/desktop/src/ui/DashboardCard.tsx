/**
 * The pieces every dashboard card is built from: the card itself (a small icon and label, then
 * its content, sized by the grid), its quiet empty line, the count at the end of its label row,
 * and the rows of a list card, which show as many as the card's height holds.
 */

import type { ReactNode } from "react";
import type { LucideIcon } from "./icons";
import { cx, Tooltip } from "./primitives";
import {
  useCardHeight,
  useWidgetEditClass,
  useWidgetId,
  useWidgetPlacement,
  WidgetChrome,
} from "./WidgetGrid";

/** A list row's height, one line or two, and what a card spends on its padding, label row and the gap under it. */
export const ROW_HEIGHT = 24;
export const TWO_LINE_ROW_HEIGHT = 40;
export const CARD_CHROME = 52;
/** From this card height a list whose rows have more to say gives each row a second line. */
const TWO_LINE_FROM = 176;

/** Whether the card is tall enough for its rows' second line. */
export function useTwoLines(): boolean {
  return useCardHeight() >= TWO_LINE_FROM;
}

/** How many rows a list card shows before its label says how many are left: as many as its height holds. */
export function useShownRows(rowHeight = ROW_HEIGHT): number {
  return Math.max(1, Math.floor((useCardHeight() - CARD_CHROME + 4) / rowHeight));
}

/**
 * One card: a small icon and label, then whatever the card has to say, on the surface the
 * Statistics page's tiles use. Its height is the grid's rows; the dashboard's entrance moves what
 * is inside it on the first visit of a session (index.css, `rc-overview-in`).
 */
export function Card({
  label,
  icon: Icon,
  aside,
  children,
}: {
  label: string;
  icon: LucideIcon;
  /** Something small at the end of the card's label row. */
  aside?: ReactNode;
  children: ReactNode;
}) {
  const placement = useWidgetPlacement();
  const editClass = useWidgetEditClass();
  return (
    <section
      aria-label={label}
      data-widget={useWidgetId()}
      style={{ height: useCardHeight(), ...placement }}
      className={cx(
        "rc-overview-card flex min-w-0 flex-col overflow-hidden rounded-xl bg-surface p-3 ring-inset",
        editClass ?? "ring-1 ring-border",
      )}
    >
      <div className="rc-overview-body flex min-h-0 flex-1 flex-col gap-2">
        <div className="flex h-5 shrink-0 items-center justify-between gap-2">
          <h3 className="flex min-w-0 items-center gap-1.5 text-caption font-medium text-fg-muted">
            <Icon size={12} className="shrink-0" />
            <span className="truncate">{label}</span>
          </h3>
          <WidgetChrome fallback={aside} />
        </div>
        {children}
      </div>
    </section>
  );
}

/** A card's one quiet line when it has nothing else to draw. */
export function Nothing({ children }: { children: ReactNode }) {
  return (
    <div className="flex flex-1 items-center">
      <p className="text-caption text-fg-muted">{children}</p>
    </div>
  );
}

/**
 * A card's empty state when the emptiness is the news (nothing waiting, nothing running): a mark,
 * a short line and, when there is one, a quiet line under it that leads somewhere.
 */
export function Calm({
  icon: Icon,
  children,
  detail,
}: {
  icon: LucideIcon;
  children: ReactNode;
  detail?: ReactNode;
}) {
  return (
    <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-1.5 text-center">
      <span className="flex size-7 items-center justify-center rounded-full bg-fill-subtle text-fg-muted">
        <Icon size={14} />
      </span>
      <p className="text-callout text-fg">{children}</p>
      {detail && <div className="max-w-full text-caption text-fg-muted">{detail}</div>}
    </div>
  );
}

/** A small count at the end of a label row, or a link where it opens what it counts. */
export function LabelNote({
  children,
  onClick,
  title,
}: {
  children: ReactNode;
  onClick?: () => void;
  title: string;
}) {
  const className = "shrink-0 rounded-sm text-caption text-fg-muted tabular";
  return (
    <Tooltip label={title}>
      {onClick ? (
        <button type="button" onClick={onClick} className={cx(className, "hover:text-fg")}>
          {children}
        </button>
      ) : (
        <span className={className}>{children}</span>
      )}
    </Tooltip>
  );
}

/** A row in a card's list: a control, so the card is walkable with the keyboard. */
export function CardRow({
  onClick,
  title,
  mark,
  detail,
  children,
}: {
  onClick: () => void;
  title: string;
  /** What leads the row, a dot or an icon: centred on both lines when there are two. */
  mark?: ReactNode;
  /** A quiet second line under the row, for a card tall enough to show it. */
  detail?: ReactNode;
  children: ReactNode;
}) {
  const className = "-mx-1 min-w-0 shrink-0 rounded-sm px-1 text-left text-callout hover:bg-fill-subtle";
  return (
    <Tooltip label={title}>
      {detail === undefined ? (
        <button type="button" onClick={onClick} className={cx(className, "flex h-6 items-center gap-2")}>
          {mark}
          {children}
        </button>
      ) : (
        <button type="button" onClick={onClick} className={cx(className, "flex h-10 items-center gap-2")}>
          {mark}
          <span className="flex min-w-0 flex-1 flex-col">
            <span className="flex min-w-0 items-center gap-2">{children}</span>
            <span className="truncate text-caption text-fg-muted">{detail}</span>
          </span>
        </button>
      )}
    </Tooltip>
  );
}

export function CardList({ children }: { children: ReactNode }) {
  return <div className="-mt-1 flex min-h-0 flex-1 flex-col">{children}</div>;
}
