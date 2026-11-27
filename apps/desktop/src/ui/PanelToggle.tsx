import { useSyncExternalStore } from "react";
import { appPlatform } from "../lib/platform";
import { toggleRightPanel, useApp, useShortcutHint } from "../state/store";
import { PANEL_SHEET_AT } from "../state/viewActions";
import { PanelRight } from "./icons";
import { Tooltip } from "./primitives";
import { windowControlsInset } from "./WindowControls";

/** What the toggle says, wherever it stands. */
export const PANEL_TOGGLE_LABEL = "Thread info";

function subscribe(onChange: () => void) {
  window.addEventListener("resize", onChange);
  return () => window.removeEventListener("resize", onChange);
}

/**
 * Whether the open card starts at the window's top: a docked card does, a
 * sheet (a narrow window) does not, nor does a window that draws its own controls at the top right
 * (Windows), where the card starts under the header's row.
 */
export function useCardOverHeader(): boolean {
  return useSyncExternalStore(
    subscribe,
    () => window.innerWidth > PANEL_SHEET_AT && windowControlsInset(appPlatform()) === 0,
  );
}

export function PanelToggle() {
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
        className="inline-flex size-7 shrink-0 items-center justify-center rounded-md text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong"
      >
        <PanelRight size={16} />
      </button>
    </Tooltip>
  );
}
