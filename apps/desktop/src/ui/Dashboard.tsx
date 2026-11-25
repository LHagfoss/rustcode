/**
 * Dashboard: the project at a glance, reached from the sidebar's Dashboard row. A bento grid of cards
 * (activity, recent threads, what needs you, what is running, spend and pull requests) that fills
 * the page; Customize moves, resizes, adds and removes cards (WidgetGrid).
 */

import { useEffect } from "react";
import { projectName } from "../acp/types";
import { useApp } from "../state/store";
import { ViewHeader } from "./MainHeader";
import { NewThreadWidgets } from "./NewThreadWidgets";
import { CustomizeOverview, setWidgetsEditing } from "./WidgetGrid";

export function Dashboard() {
  const projectCwd = useApp((s) => s.projectCwd);
  useEffect(() => () => setWidgetsEditing(false), []);
  return (
    <div className="flex min-h-0 flex-1 flex-col bg-bg">
      <ViewHeader />
      <div className="flex min-h-0 flex-1 flex-col px-6 pt-2 pb-6">
        <div className="mx-auto flex min-h-0 w-full max-w-[1200px] flex-1 flex-col gap-3">
          <div className="flex shrink-0 flex-wrap items-end justify-between gap-x-4 gap-y-1">
            <div className="flex min-w-0 items-baseline gap-2">
              <h1 className="font-semibold text-title tracking-tight text-fg">Dashboard</h1>
              {projectCwd && (
                <span className="truncate text-callout text-fg-muted">{projectName(projectCwd)}</span>
              )}
            </div>
            <CustomizeOverview glass={false} />
          </div>
          <div className="rc-overview-in min-h-0 w-full flex-1 overflow-y-auto">
            <NewThreadWidgets project={projectCwd} docked />
          </div>
        </div>
      </div>
    </div>
  );
}
