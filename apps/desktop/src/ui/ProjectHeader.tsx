// Shared controls for project folders and sidebar row actions.
import type { HiddenKind } from "../state/sidebarRows";
import { ChevronDown, Folder, FolderOpen, SquarePen } from "./icons";
import { cx, Tooltip } from "./primitives";
import { RollingNumber } from "./RollingNumber";
import { SIDEBAR_ROW_HOVER, SIDEBAR_ROW_RADIUS } from "./sidebarRowStyle";

/** A small icon button in a row's hover slot: 24px, so three of them take the 80px slot. */
export function RowAction({
  label,
  hint,
  onClick,
  children,
}: {
  label: string;
  /** The key that does the same thing, shown on the tooltip. */
  hint?: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <Tooltip label={hint ? `${label} (${hint})` : label}>
      <button
        type="button"
        aria-label={label}
        onClick={(e) => {
          e.stopPropagation();
          onClick();
        }}
        className="flex size-6 items-center justify-center rounded-sm text-fg-muted hover:bg-fill hover:text-fg"
      >
        {children}
      </button>
    </Tooltip>
  );
}

/** The dot a folded group shows for what it hides. */
export function HiddenDot({ kind }: { kind?: HiddenKind }) {
  if (!kind) return null;
  return (
    <span
      role="img"
      aria-label={
        kind === "needs"
          ? "A thread here needs you"
          : kind === "working"
            ? "A thread here is working"
            : "A thread here finished"
      }
      className={cx(
        "size-1.5 shrink-0 rounded-full",
        kind === "needs" ? "bg-warning" : kind === "working" ? "bg-accent animate-pulse" : "bg-success",
      )}
    />
  );
}

/**
 * A project's folder: the folder and its name, a dot for
 * what it hides and its count only while folded, the chevron at the end, and
 * a new-thread button on hover. It reads as its section's own labels do: the same 32px, the same
 * quiet 14px type, so a list of projects is one quiet column of headings.
 */
export function ProjectHeader({
  label,
  count,
  open,
  hidden,
  onToggle,
  onNew,
}: {
  label: string;
  count: number;
  open: boolean;
  hidden?: HiddenKind;
  onToggle: () => void;
  onNew?: () => void;
}) {
  return (
    <div className="group/p relative flex h-[34px] items-center">
      <button
        type="button"
        onClick={onToggle}
        data-project-handle
        aria-expanded={open}
        className={cx(
          "flex h-full min-w-0 flex-1 items-center gap-2 px-2 text-left font-medium text-body text-fg/90",
          SIDEBAR_ROW_RADIUS,
          SIDEBAR_ROW_HOVER,
          "hover:glass-filter active:glass-filter",
        )}
      >
        {open ? (
          <FolderOpen size={14} className="shrink-0 text-fg-muted" />
        ) : (
          <Folder size={14} className="shrink-0 text-fg-muted" />
        )}
        <span className="min-w-0 flex-1 truncate">{label}</span>
        {!open && <HiddenDot kind={hidden} />}
        {!open && <RollingNumber value={count} className="font-normal text-fg-faint text-sub" />}
        <ChevronDown
          size={12}
          className={cx(
            "shrink-0 text-fg-faint transition-transform duration-200 ease-snappy",
            !open && "-rotate-90",
            onNew && "group-hover/p:invisible group-has-[:focus-visible]/p:invisible",
          )}
        />
      </button>
      {onNew && (
        <span className="absolute right-1.5 hidden group-hover/p:flex group-has-[:focus-visible]/p:flex">
          <RowAction label={`New thread in ${label}`} onClick={onNew}>
            <SquarePen size={12} />
          </RowAction>
        </span>
      )}
    </div>
  );
}
