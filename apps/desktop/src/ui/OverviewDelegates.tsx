import { useState } from "react";
import { readStorage, writeStorage } from "../lib/storage";
import { DelegateLanes } from "./DelegateLanes";
import { DelegatedThreads } from "./DelegateTree";
import { OverviewSection } from "./OverviewSection";
import { Segments } from "./primitives";

type DelegatesView = "tree" | "lanes";
const VIEW_KEY = "rustcode.delegatesView";

export function DelegatesSection({
  threadId,
  count,
  folded,
  onToggle,
}: {
  threadId: string;
  /** Every thread in the branch: the ones it started, and the ones those started. */
  count: number;
  folded: boolean;
  onToggle: () => void;
}) {
  // Tree (who started whom) or lanes (what ran on which agent, in order); the choice is kept.
  const [view, setView] = useState<DelegatesView>(() =>
    readStorage(VIEW_KEY) === "lanes" ? "lanes" : "tree",
  );
  const choose = (next: DelegatesView) => {
    setView(next);
    writeStorage(VIEW_KEY, next);
  };
  return (
    <OverviewSection
      id="delegates"
      title="Delegates"
      count={count}
      folded={folded}
      onToggle={onToggle}
      actions={
        <Segments
          label="Delegates view"
          value={view}
          onChange={choose}
          options={[
            { value: "tree", label: "Tree" },
            { value: "lanes", label: "Lanes" },
          ]}
        />
      }
    >
      <p className="px-2 pt-0.5 pb-1 text-caption text-fg-faint">
        {view === "lanes"
          ? "Each agent and model in its own lane, in the order the work was handed out."
          : "New threads it handed a task to, and the threads those started."}
      </p>
      {view === "lanes" ? <DelegateLanes threadId={threadId} /> : <DelegatedThreads threadId={threadId} />}
      <div className="h-1" />
    </OverviewSection>
  );
}
