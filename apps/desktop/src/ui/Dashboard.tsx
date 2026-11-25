/**
 * Dashboard: the project at a glance, reached from the sidebar's Dashboard row. Four figures with
 * their trends lead, then a grid of cards (activity, threads, what needs you and what is running,
 * models, harnesses, rhythm, pull requests, today) that fills the page and drops to two columns
 * or one in a narrow window. Customize moves, resizes, adds and removes cards (WidgetGrid).
 */

import { useEffect } from "react";
import { projectName } from "../acp/types";
import { useApp } from "../state/store";
import { DashboardBody } from "./DashboardWidgets";
import { ViewHeader } from "./MainHeader";
import { CustomizeOverview, setWidgetsEditing } from "./WidgetGrid";

export function Dashboard() {
  const projectCwd = useApp((s) => s.projectCwd);
  useEffect(() => () => setWidgetsEditing(false), []);
  return (
    <div className="flex min-h-0 flex-1 flex-col bg-bg">
      <ViewHeader />
      <div className="flex min-h-0 flex-1 flex-col px-6 pt-2 pb-6">
        <div className="mx-auto flex min-h-0 w-full max-w-[1400px] flex-1 flex-col gap-3">
          <div className="flex shrink-0 flex-wrap items-end justify-between gap-x-4 gap-y-1">
            <div className="flex min-w-0 items-baseline gap-2">
              <h1 className="font-semibold text-title tracking-tight text-fg">Dashboard</h1>
              {projectCwd && (
                <span className="truncate text-callout text-fg-muted">{projectName(projectCwd)}</span>
              )}
            </div>
            <CustomizeOverview glass={false} />
          </div>
          <DashboardBody project={projectCwd} />
        </div>
      </div>
    </div>
  );
}
