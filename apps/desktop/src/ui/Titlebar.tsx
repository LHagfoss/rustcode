// The window's title row, shared by the sidebar (its toggle) and the main
// column's header, so the two line up on one line through the traffic lights.
// It is the only row the window has (docs/design.md › Title row). `trafficLightPosition` in
// tauri.conf.json puts the lights' centre on this row's centre. In full screen
// the lights are gone, so the row starts at the edge instead of leaving their
// room empty, and on Windows the shell draws its own controls (ui/WindowControls.tsx).

import { type ReactNode, useEffect, useState } from "react";
import { isTauri } from "../acp/transport";
import { appPlatform } from "../lib/platform";
import { cx } from "./primitives";

/**
 * Room the traffic lights take, plus the SwiftUI app's gap after them. With
 * `trafficLightPosition` x 18 the zoom button ends at x 78 (measured in the
 * window); the SwiftUI app's lights end near x 66 with its first cluster at
 * 80 (SidebarMetrics.barLeading), a 14 pt gap, so the row starts at 92.
 */
export const LIGHTS_INSET = 92;

/**
 * The title row's height, on every platform: the window's drag strip, the
 * traffic lights and the main column's header all sit on this one line.
 * `trafficLightPosition.y` is 22 so the lights' centre (tao sizes their bar to
 * the close button's height plus y and centres them in it) lands on 18, where
 * they sat before the row grew to hold a brand.
 */
export const TITLEBAR_HEIGHT = 36;

let fullscreen = false;
const listeners = new Set<(v: boolean) => void>();
let watching = false;

function watch() {
  if (watching || !isTauri()) return;
  watching = true;
  void import("@tauri-apps/api/window").then(async ({ getCurrentWindow }) => {
    const win = getCurrentWindow();
    const check = async () => {
      const next = await win.isFullscreen().catch(() => false);
      if (next === fullscreen) return;
      fullscreen = next;
      for (const l of listeners) l(next);
    };
    await check();
    await win.onResized(() => void check());
  });
}

/** Whether the window is in full screen, where the traffic lights hide. */
export function useFullscreen(): boolean {
  const [value, setValue] = useState(fullscreen);
  useEffect(() => {
    watch();
    listeners.add(setValue);
    setValue(fullscreen);
    return () => {
      listeners.delete(setValue);
    };
  }, []);
  return value;
}

/** The leading room a title-bar row leaves for the traffic lights. */
export function useLightsInset(): number {
  const fullscreen = useFullscreen();
  return appPlatform() !== "mac" || fullscreen ? 10 : LIGHTS_INSET;
}

export function TitlebarRow({
  children,
  className,
  lights = true,
}: {
  children: ReactNode;
  className?: string;
  /** Whether the row starts under the traffic lights. */
  lights?: boolean;
}) {
  const inset = useLightsInset();
  return (
    <div
      data-tauri-drag-region
      className={cx("flex shrink-0 items-center", className)}
      style={{ height: TITLEBAR_HEIGHT, ...(lights ? { paddingLeft: inset } : null) }}
    >
      {children}
    </div>
  );
}
