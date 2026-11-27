/**
 * The shape and fills every sidebar row shares: the nav rows (New thread, Dashboard, Jobs), the
 * project headers and the thread rows, so hovering or opening any of them reads the same.
 */
export const SIDEBAR_ROW_RADIUS = "rounded-lg";

/** A row's fill on hover and while pressed, for a row that draws its own background. */
export const SIDEBAR_ROW_HOVER = "hover:bg-fill active:bg-fill-strong";

/** The same hover fill, for a fill layer inside a `group` row. */
export const SIDEBAR_ROW_GROUP_HOVER =
  "group-hover:bg-fill group-hover:glass-filter group-active:bg-fill-strong";

/** The fill of the row that is open: the current view or the selected thread. */
export const SIDEBAR_ROW_CURRENT = "bg-fill-strong glass-filter";

/** A row picked up to drag: the selected fill over a blur of the rows it passes, so they never show through. */
export const SIDEBAR_ROW_LIFTED = "shadow-1";
