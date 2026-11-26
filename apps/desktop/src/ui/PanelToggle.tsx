import { useSyncExternalStore } from "react";
import { appPlatform } from "../lib/platform";
import { toggleRightPanel, useApp, useShortcutHint } from "../state/store";
import { PANEL_SHEET_AT } from "../state/viewActions";
import { PanelRight } from "./icons";
import { cx, Tooltip } from "./primitives";
import { windowControlsInset } from "./WindowControls";

/** What the toggle says, wherever it stands. */
export const PANEL_TOGGLE_LABEL = "Thread info";

function subscribe(onChange: () => void) {
  window.addEventListener("resize", onChange);
  return () => window.removeEventListener("resize", onChange);
}

/**
 * Whether the open card starts at the window's top and carries the toggle: a docked card does, a
 * sheet (a narrow window) does not, nor does a window that draws its own controls at the top right
 * (Windows), where the card starts under the header's row.
 */
export function useCardOverHeader(): boolean {
  return useSyncExternalStore(
    subscribe,
    () => window.innerWidth > PANEL_SHEET_AT && windowControlsInset(appPlatform()) === 0,
  );
}

/**
 * The side panel's toggle, "Thread info": the header carries it while the panel is closed, and the
 * panel's card carries it while open, at the card's top right (ui/RightPanel.tsx). Its
 * `data-panel-toggle` takes the keyboard back when a panel closes.
 */
export function PanelToggle({ className, iconOnly }: { className?: string; iconOnly?: boolean }) {
  const open = useApp((s) => !!s.panel);
  const key = useShortcutHint("toggle-panel");
  return (
    <Tooltip label={open ? `Hide thread info (${key})` : `Show thread info (${key})`} side="bottom">
      <button
        type="button"
        aria-label={PANEL_TOGGLE_LABEL}
        aria-pressed={open}
        data-panel-toggle
        onClick={toggleRightPanel}
        className={cx(
          "inline-flex h-7 shrink-0 items-center gap-1.5 text-callout text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong",
          iconOnly ? "w-7 justify-center rounded-md" : "rounded-full pr-3 pl-2",
          open && !iconOnly && "bg-fill text-fg",
          className,
        )}
      >
        <PanelRight size={16} />
        {!iconOnly && PANEL_TOGGLE_LABEL}
      </button>
    </Tooltip>
  );
}
