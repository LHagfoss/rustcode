// The hidden title bar's row, shared by the sidebar and the chat header so
// their buttons line up with the traffic lights as the SwiftUI app's do
// (SidebarMetrics: a 28 pt bar under a 6 pt inset, the first cluster 80 pt
// in, just past the zoom button; `trafficLightPosition` in tauri.conf.json
// centres the lights on the same line). In full screen the lights are gone,
// so the row starts at the edge instead of leaving their room empty.

import { type ReactNode, useEffect, useState } from "react";
import { isTauri } from "../acp/transport";
import { cx } from "./primitives";

/** Room the traffic lights take, up to just past the zoom button. */
export const LIGHTS_INSET = 80;

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
  return useFullscreen() ? 10 : LIGHTS_INSET;
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
      className={cx("flex h-[34px] shrink-0 items-end gap-1", className)}
      style={lights ? { paddingLeft: inset } : undefined}
    >
      {children}
    </div>
  );
}
