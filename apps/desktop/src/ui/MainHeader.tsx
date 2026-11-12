// The main column's header, shared by the chat, Settings and the other
// views, so switching between them keeps the bar where it is: the sidebar's
// toggle (and Settings) when the sidebar is hidden (clear of the traffic lights), the
// view's title and its own controls. It floats in glass over its view (the
// view leaves HEADER_HEIGHT at its top), so content scrolls under it.
//
// It is the sidebar's title row's twin (ui/Titlebar.tsx): one height on one
// line, and on Windows it leaves the window's own controls their room at the
// window's right edge (ui/WindowControls.tsx).

import { Settings as Gear, PanelLeft } from "lucide-react";
import type { ReactNode } from "react";
import { appPlatform } from "../lib/platform";
import { useMachines } from "../state/machines";
import { openSettings, toggleSidebar, useApp, useShortcutHint } from "../state/store";
import { Cluster, ClusterButton, cx } from "./primitives";
import { TITLEBAR_HEIGHT, useLightsInset } from "./Titlebar";
import { onTitleRowDoubleClick, windowControlsInset } from "./WindowControls";

/** Settings, in the sidebar's toolbar and in the header's row when the sidebar is hidden. */
export function SettingsButton() {
  const open = useApp((s) => s.settings !== null);
  const key = useShortcutHint("settings");
  // A paired machine that is not connected has no row of its own in the list: the gear says so
  // with a dot, and Settings › Machines says which.
  const away = useMachines((s) => s.machines.filter((m) => m.state !== "connected").length);
  return (
    <ClusterButton
      label="Settings"
      tooltip={
        open
          ? "Close Settings (Esc)"
          : `Settings (${key})${away ? ` · ${away === 1 ? "a machine is" : `${away} machines are`} not connected` : ""}`
      }
      on={open}
      onClick={() => openSettings(open ? null : "general")}
    >
      <span className="relative flex">
        <Gear size={14} />
        {away > 0 && (
          <span
            aria-hidden
            data-testid="machines-away-dot"
            className="-top-0.5 -right-0.5 absolute size-1.5 rounded-full bg-warning"
          />
        )}
      </span>
    </ClusterButton>
  );
}

/** The header's height: a view under it starts its content this far down. */
export const HEADER_HEIGHT = TITLEBAR_HEIGHT;

export function MainHeader({
  title,
  right,
  className,
  lights,
}: {
  title: ReactNode;
  right?: ReactNode;
  className?: string;
  /** Leave the traffic lights' room even with the sidebar shown (Settings covers the sidebar). */
  lights?: boolean;
}) {
  const sidebarHidden = useApp((s) => s.sidebarHidden);
  const toggleKey = useShortcutHint("toggle-sidebar");
  const lightsInset = useLightsInset();
  const controlsInset = windowControlsInset(appPlatform());
  const underLights = lights || sidebarHidden;
  // A double-click on the title row maximizes on Windows, which has no native title bar to do
  // it; macOS gets it from the window's own title bar, and Linux has native decorations.
  const maximizes = appPlatform() === "windows";
  return (
    <header
      data-tauri-drag-region
      onDoubleClick={maximizes ? onTitleRowDoubleClick : undefined}
      className={cx(
        // Glass over the content, which scrolls under it (docs/design.md, Header).
        "rc-header-glass absolute inset-x-0 top-0 z-10 flex items-center gap-2 pr-2",
        !underLights && "pl-2",
        className,
      )}
      style={{
        height: HEADER_HEIGHT,
        ...(underLights ? { paddingLeft: lightsInset } : null),
        ...(controlsInset ? { paddingRight: controlsInset } : null),
      }}
    >
      {sidebarHidden && !lights && (
        <Cluster>
          <ClusterButton label="Show sidebar" tooltip={`Show sidebar (${toggleKey})`} onClick={toggleSidebar}>
            <PanelLeft size={14} />
          </ClusterButton>
          <SettingsButton />
        </Cluster>
      )}
      <div className="flex min-w-0 flex-1 items-center gap-1.5" data-tauri-drag-region>
        {title}
      </div>
      {right && (
        // Each group stands in its own track, like the sidebar's toolbar cluster: the icon buttons
        // keep their pill, and the groups (and the pills beside them) are 6px apart.
        <div className="flex shrink-0 items-center gap-1.5">{right}</div>
      )}
    </header>
  );
}
