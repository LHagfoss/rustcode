// The window's own minimize, maximize/restore and close. Only Windows draws
// them: tauri.windows.conf.json turns the native decorations off there, so the
// shell's title row is the window's title bar (docs/desktop.md › Title bar).
// They sit at the window's top right, over the main column's top row, so they
// stay put with the sidebar hidden and while any view is open. macOS keeps its
// traffic lights and Linux its native title bar.

import { useEffect, useState } from "react";
import { isTauri } from "../acp/transport";
import { appPlatform, type Platform } from "../lib/platform";
import { cx } from "./primitives";
import { TITLEBAR_HEIGHT } from "./Titlebar";

/** The width the three buttons take; a header that reaches the window's edge leaves it clear. */
export const WINDOW_CONTROLS_WIDTH = 138;

/** Whether the page draws the window's controls, which only Windows does (no decorations there). */
export function drawsWindowControls(platform: Platform): boolean {
  return platform === "windows";
}

/** The room a top row leaves for the controls: none anywhere but Windows. */
export function windowControlsInset(platform: Platform): number {
  return drawsWindowControls(platform) ? WINDOW_CONTROLS_WIDTH : 0;
}

export type WindowAction = "minimize" | "toggle-maximize" | "close";

/**
 * Runs one of the window's actions through the shell. The mock window and
 * `dev:web` have no window of their own, so there they do nothing.
 */
export async function windowAction(action: WindowAction): Promise<void> {
  if (!isTauri()) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const win = getCurrentWindow();
  if (action === "minimize") await win.minimize();
  else if (action === "close") await win.close();
  else await win.toggleMaximize();
}

/**
 * The title row's double-click: Windows maximizes the window from it, as a native title bar
 * does, and a double-click that lands on one of the row's controls is that control's alone.
 */
export function onTitleRowDoubleClick(event: { target: EventTarget | null }): void {
  const hit = event.target as Element | null;
  if (hit?.closest("button, a, input, textarea, select, [role='button']")) return;
  void windowAction("toggle-maximize");
}

/** Whether the window is maximized now, kept current as it is resized or snapped. */
function useMaximized(): boolean {
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    if (!isTauri()) return;
    let live = true;
    let stop: (() => void) | undefined;
    void import("@tauri-apps/api/window").then(async ({ getCurrentWindow }) => {
      const win = getCurrentWindow();
      const check = () => {
        void win
          .isMaximized()
          .then((on) => {
            if (live) setMaximized(on);
          })
          .catch(() => undefined);
      };
      check();
      if (!live) return;
      stop = await win.onResized(() => void check());
    });
    return () => {
      live = false;
      stop?.();
    };
  }, []);
  return maximized;
}

/** Windows' own glyphs, at the size its title bar draws them: a hairline, a square, two squares, an X. */
function Glyph({ name }: { name: "minimize" | "maximize" | "restore" | "close" }) {
  return (
    <svg
      width={10}
      height={10}
      viewBox="0 0 10 10"
      fill="none"
      stroke="currentColor"
      strokeWidth={1}
      aria-hidden
      className="shrink-0"
    >
      {name === "minimize" && <path d="M0 5.5h10" />}
      {name === "maximize" && <rect x={0.5} y={0.5} width={9} height={9} />}
      {name === "restore" && (
        <>
          <rect x={0.5} y={2.5} width={7} height={7} />
          <path d="M2.5 2.5v-2h7v7h-2" />
        </>
      )}
      {name === "close" && <path d="M0.5 0.5l9 9M9.5 0.5l-9 9" />}
    </svg>
  );
}

function Control({
  label,
  glyph,
  close,
  onClick,
}: {
  label: string;
  glyph: "minimize" | "maximize" | "restore" | "close";
  /** The close button, which turns the danger red under the pointer, as Windows' does. */
  close?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      onClick={onClick}
      className={cx(
        "inline-flex h-full w-[46px] items-center justify-center text-fg-muted active:bg-fill-strong",
        close ? "hover:bg-danger hover:text-accent-fg" : "hover:bg-fill hover:text-fg",
      )}
    >
      <Glyph name={glyph} />
    </button>
  );
}

/** The window's three controls, at its top right; nothing at all on macOS and Linux. */
export function WindowControls({ className }: { className?: string }) {
  const platform = appPlatform();
  const maximized = useMaximized();
  if (!drawsWindowControls(platform)) return null;
  return (
    <div className={cx("flex shrink-0 items-stretch", className)} style={{ height: TITLEBAR_HEIGHT }}>
      <Control label="Minimize" glyph="minimize" onClick={() => void windowAction("minimize")} />
      <Control
        label={maximized ? "Restore" : "Maximize"}
        glyph={maximized ? "restore" : "maximize"}
        onClick={() => void windowAction("toggle-maximize")}
      />
      <Control label="Close" glyph="close" close onClick={() => void windowAction("close")} />
    </div>
  );
}
