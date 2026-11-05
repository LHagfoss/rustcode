// The main column's header, shared by the chat, Settings and the other
// views, so switching between them keeps the bar where it is: the sidebar's
// buttons when the sidebar is hidden (clear of the traffic lights), the
// view's title and its own controls. It floats in glass over its view (the
// view leaves HEADER_HEIGHT at its top), so content scrolls under it.

import { Settings as Gear, PanelLeft, SquarePen } from "lucide-react";
import type { ReactNode } from "react";
import { openSettings, startNewThread, toggleSidebar, useApp, useShortcutHint } from "../state/store";
import { Cluster, ClusterButton, cx } from "./primitives";
import { useLightsInset } from "./Titlebar";

/** Settings, in the sidebar's toolbar and in the header's row when the sidebar is hidden. */
export function SettingsButton() {
  const open = useApp((s) => s.settings !== null);
  const key = useShortcutHint("settings");
  return (
    <ClusterButton
      label="Settings"
      tooltip={open ? "Close Settings (Esc)" : `Settings (${key})`}
      on={open}
      onClick={() => openSettings(open ? null : "general")}
    >
      <Gear size={14} />
    </ClusterButton>
  );
}

/** The header's height: a view under it starts its content this far down. */
export const HEADER_HEIGHT = 42;

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
  const newKey = useShortcutHint("new-thread");
  const lightsInset = useLightsInset();
  const underLights = lights || sidebarHidden;
  return (
    <header
      data-tauri-drag-region
      className={cx(
        // Glass over the content, which scrolls under it (docs/design.md, Header).
        "rc-header-glass absolute inset-x-0 top-0 z-10 flex h-[42px] items-center gap-2.5 pt-1.5 pb-2 pr-2.5",
        !underLights && "pl-2.5",
        className,
      )}
      style={underLights ? { paddingLeft: lightsInset } : undefined}
    >
      {sidebarHidden && !lights && (
        <Cluster>
          <ClusterButton label="Show sidebar" tooltip={`Show sidebar (${toggleKey})`} onClick={toggleSidebar}>
            <PanelLeft size={14} />
          </ClusterButton>
          <ClusterButton
            label="New thread"
            tooltip={`New thread (${newKey})`}
            onClick={() => startNewThread()}
          >
            <SquarePen size={14} />
          </ClusterButton>
          <SettingsButton />
        </Cluster>
      )}
      <div className="flex min-w-0 flex-1 items-center gap-1.5" data-tauri-drag-region>
        {title}
      </div>
      {right && (
        // Each control stands on its own, without the track a Cluster draws: the header already
        // has one shape in it (the title's project mark), and a pill beside it reads as a second.
        <div className="flex shrink-0 items-center gap-0.5 [&>div]:h-7 [&>div]:gap-0 [&>div]:rounded-none [&>div]:bg-transparent [&>div]:px-0">
          {right}
        </div>
      )}
    </header>
  );
}
