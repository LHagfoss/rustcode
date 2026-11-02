// The main column's header, shared by the chat, Settings and the other
// views, so switching between them keeps the bar where it is: the sidebar's
// buttons when the sidebar is hidden (clear of the traffic lights), the
// view's title and its own controls.

import { Settings as Gear, PanelLeft, SquarePen } from "lucide-react";
import type { ReactNode } from "react";
import { openSettings, startNewThread, toggleSidebar, useApp } from "../state/store";
import { Cluster, ClusterButton, cx } from "./primitives";
import { useLightsInset } from "./Titlebar";

/** Settings, in the sidebar's top row (and the header's when the sidebar is hidden). */
export function SettingsButton() {
  const open = useApp((s) => s.settings !== null);
  return (
    <ClusterButton
      label="Settings"
      tooltip={open ? "Close Settings (Esc)" : "Settings (⌘,)"}
      on={open}
      onClick={() => openSettings(open ? null : "general")}
    >
      <Gear size={14} />
    </ClusterButton>
  );
}

export function MainHeader({
  title,
  right,
  className,
}: {
  title: ReactNode;
  right?: ReactNode;
  className?: string;
}) {
  const sidebarHidden = useApp((s) => s.sidebarHidden);
  const lightsInset = useLightsInset();
  return (
    <header
      data-tauri-drag-region
      className={cx(
        "relative z-10 flex h-[42px] shrink-0 items-center gap-2.5 pt-1.5 pb-2 pr-2.5",
        !sidebarHidden && "pl-2.5",
        className,
      )}
      style={sidebarHidden ? { paddingLeft: lightsInset } : undefined}
    >
      {sidebarHidden && (
        <Cluster>
          <ClusterButton label="Show sidebar" tooltip="Show sidebar (⌘B)" onClick={toggleSidebar}>
            <PanelLeft size={14} />
          </ClusterButton>
          <ClusterButton label="New thread" tooltip="New thread (⌘N)" onClick={() => startNewThread()}>
            <SquarePen size={14} />
          </ClusterButton>
          <SettingsButton />
        </Cluster>
      )}
      <div className="flex min-w-0 flex-1 items-center gap-1.5" data-tauri-drag-region>
        {title}
      </div>
      {right}
    </header>
  );
}
