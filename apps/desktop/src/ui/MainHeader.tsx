// The main column's header, shared by the chat, Settings and the other
// views, so switching between them keeps the bar where it is: the sidebar's
// toggle (and Settings) when the sidebar is hidden (clear of the traffic lights), the
// view's title and its own controls. It floats in glass over its view (the
// view leaves HEADER_HEIGHT at its top), so content scrolls under it.
//
// It is the sidebar's title row's twin (ui/Titlebar.tsx): one height on one
// line, and on Windows it leaves the window's own controls their room at the
// window's right edge (ui/WindowControls.tsx).

import type { ReactNode } from "react";
import { appPlatform } from "../lib/platform";
import { useSettingsAttention } from "../state/settingsAttention";
import {
  openSettings,
  type SettingsSection,
  setSwitcher,
  toggleSidebar,
  useApp,
  useShortcutHint,
} from "../state/store";
import { Settings as Gear, PanelLeft, Search } from "./icons";
import { Cluster, ClusterButton, cx } from "./primitives";
import { TITLEBAR_HEIGHT, useLightsInset } from "./Titlebar";
import { onTitleRowDoubleClick, windowControlsInset } from "./WindowControls";

/** Settings, in the sidebar's toolbar and in the header's row when the sidebar is hidden. */
export function SettingsButton() {
  const open = useApp((s) => s.settings !== null);
  const key = useShortcutHint("settings");
  // A paired machine that is not connected, or a warning from the RustCode server: the gear says
  // so with a dot and opens the page that says which and offers the fix (state/settingsAttention.ts).
  const attention = useSettingsAttention();
  const first = Object.keys(attention)[0] as SettingsSection | undefined;
  const reasons = Object.values(attention).join(" · ");
  return (
    <ClusterButton
      label="Settings"
      tooltip={open ? "Close Settings (Esc)" : `Settings (${key})${reasons ? ` · ${reasons}` : ""}`}
      on={open}
      onClick={() => openSettings(open ? null : (first ?? "general"))}
    >
      <span className="relative flex">
        <Gear size={14} />
        {first && (
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

export function SidebarCluster() {
  const toggleKey = useShortcutHint("toggle-sidebar");
  return (
    <Cluster>
      <ClusterButton label="Show sidebar" tooltip={`Show sidebar (${toggleKey})`} onClick={toggleSidebar}>
        <PanelLeft size={14} />
      </ClusterButton>
      <ClusterButton label="Search threads" tooltip="Search threads (⌘K)" onClick={() => setSwitcher(true)}>
        <Search size={14} />
      </ClusterButton>
      <SettingsButton />
    </Cluster>
  );
}

/** A header for views that replace the chat: clear of the traffic lights and the window's controls. */
export function ViewHeader({ title, children }: { title: ReactNode; children?: ReactNode }) {
  const sidebarHidden = useApp((s) => s.sidebarHidden);
  const lightsInset = useLightsInset();
  const controlsInset = windowControlsInset(appPlatform());
  const maximizes = appPlatform() === "windows";
  return (
    <header
      data-tauri-drag-region
      onDoubleClick={maximizes ? onTitleRowDoubleClick : undefined}
      className={cx("flex shrink-0 items-center gap-2 pr-2", !sidebarHidden && "pl-4")}
      style={{
        height: HEADER_HEIGHT,
        ...(sidebarHidden ? { paddingLeft: lightsInset } : null),
        ...(controlsInset ? { paddingRight: controlsInset } : null),
      }}
    >
      {sidebarHidden && <SidebarCluster />}
      <h1 className="min-w-0 flex-1 truncate font-semibold text-body" data-tauri-drag-region>
        {title}
      </h1>
      {children}
    </header>
  );
}

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
        paddingRight: controlsInset || "calc(0.5rem + var(--rc-panel-room, 0px))",
      }}
    >
      {sidebarHidden && !lights && <SidebarCluster />}
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
