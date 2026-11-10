// The overview's Delegates section: the threads this one handed work to, at every
// depth, as the tree the details panel draws (ui/DelegateTree.tsx). It sits right
// after Properties and is open by default: a thread that delegated is defined by
// what its branch is doing, not by the rare facts behind Details.

import { DelegatedThreads } from "./DelegateTree";
import { OverviewSection } from "./OverviewSection";

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
  return (
    <OverviewSection id="delegates" title="Delegates" count={count} folded={folded} onToggle={onToggle}>
      <p className="px-2 pt-0.5 pb-1 text-caption text-fg-faint">
        New threads it handed a task to, and the threads those started.
      </p>
      <DelegatedThreads threadId={threadId} />
      <div className="h-1" />
    </OverviewSection>
  );
}
