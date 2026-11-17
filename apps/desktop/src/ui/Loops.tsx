// Loops: recurring agent runs that check one thing on a schedule and report only when something
// needs the user. Reached from the sidebar's Loops row; a placeholder until the feature lands.
// The scheduler it will drive already exists (docs/daemon.md).

import { ViewHeader } from "./Board";
import { Repeat } from "./icons";

export function Loops() {
  return (
    <div className="flex min-h-0 flex-1 flex-col bg-bg">
      <ViewHeader title="Loops" />
      <div className="flex flex-1 items-center justify-center p-8">
        <div className="flex max-w-sm flex-col items-center gap-3 text-center">
          <span className="inline-flex size-10 items-center justify-center rounded-full bg-fill text-fg-muted">
            <Repeat size={18} />
          </span>
          <h2 className="font-semibold text-title3 text-fg">Coming soon</h2>
          <p className="text-callout text-fg-muted">
            Loops run an agent on a schedule, like “every 4 hours, check CI on main”, each in a fresh thread.
            When all is fine the thread archives itself; when something needs you, it stays open for review.
          </p>
        </div>
      </div>
    </div>
  );
}
