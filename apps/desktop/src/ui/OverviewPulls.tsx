// The overview's Pull requests section: the pulls this thread's branches have —
// its own, then the ones its delegate threads opened — each with its state, what
// it changes and a pill saying where it stands (ui/ThreadDetails.tsx draws the
// sections, state/pullQueries.ts holds the reads). The first three rows show;
// a thread with more work on the go lists the rest behind "Show N more".

import {
  GitMerge,
  GitPullRequest,
  GitPullRequestClosed,
  GitPullRequestDraft,
  MoreHorizontal,
  Plus,
} from "lucide-react";
import { DropdownMenu } from "radix-ui";
import { useMemo, useState } from "react";
import { type PullRequest, pullErrorTitle } from "../acp/rustcode";
import { openExternal } from "../lib/native";
import { PULL_LIMIT, readPullList, usePullList } from "../state/pullQueries";
import { pullState } from "../state/pulls";
import { setDraft, showPanel, startNewThread } from "../state/store";
import { relativeAge } from "../state/thread";
import { OverviewSection, SectionButton, sectionIconButton } from "./OverviewSection";
import { type PullPill, pullPill, pullsForBranches } from "./overview";
import { cx, MenuContent, menuItem, menuSeparator, Pill } from "./primitives";
import { DiffCount } from "./Rows";

/** How many pulls the section shows before it asks: the rest are behind one press. */
export const PULL_ROWS = 3;

const STATE_MARK = {
  open: GitPullRequest,
  draft: GitPullRequestDraft,
  merged: GitMerge,
  closed: GitPullRequestClosed,
} as const;

const STATE_ICON_TONE = {
  open: "text-success",
  draft: "text-fg-muted",
  merged: "text-accent",
  closed: "text-danger",
} as const;

/** A pull's tallies in the app's tone: open is success, a draft is quiet, the rest speak for themselves. */
const PILL_TONE: Record<PullPill["tone"], string> = {
  accent: "bg-accent/16 text-accent",
  success: "bg-success/12 text-success",
  warning: "bg-warning/14 text-warning",
  muted: "bg-fill text-fg-muted",
  danger: "bg-danger/12 text-danger",
};

function StateMark({ pull }: { pull: PullRequest }) {
  const state = pullState(pull);
  const Mark = STATE_MARK[state];
  return <Mark size={14} strokeWidth={2} className={cx("mt-0.5 shrink-0", STATE_ICON_TONE[state])} />;
}

/** A pull's own line: its title, then what it is by number, size or age, and its state on the right. */
function PullRow({ pull, onOpen }: { pull: PullRequest; onOpen: () => void }) {
  const pill = pullPill(pull);
  const sized = pull.additions + pull.deletions > 0;
  return (
    <button
      type="button"
      onClick={onOpen}
      className="flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left hover:bg-fill-subtle"
    >
      <StateMark pull={pull} />
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="truncate font-medium text-body">{pull.title}</span>
        <span className="flex items-center gap-1.5 text-caption text-fg-muted">
          <span className="shrink-0 tabular">#{pull.number}</span>
          <span className="shrink-0 text-fg-faint">·</span>
          {sized ? (
            <DiffCount added={pull.additions} removed={pull.deletions} />
          ) : (
            <span className="truncate">{relativeAge(pull.updatedAt)}</span>
          )}
        </span>
      </span>
      <span
        className={cx(
          "mt-0.5 flex h-[18px] shrink-0 items-center rounded-full px-1.5 font-medium text-caption",
          PILL_TONE[pill.tone],
        )}
      >
        {pill.label}
      </span>
    </button>
  );
}

export function PullRequestsSection({
  cwd,
  branches,
  branch,
  folded,
  onToggle,
}: {
  /** The project the reads go to: the worktree's repository, else the thread's folder. */
  cwd: string | undefined;
  /** The branches whose pulls belong here: the thread's own first. */
  branches: string[];
  /** The thread's branch, for the ask that opens a pull. */
  branch?: string;
  folded: boolean;
  onToggle: () => void;
}) {
  // The same read the Pull requests view makes, so opening both costs one process.
  const { data: pulls, error } = usePullList(cwd, "all", { limit: PULL_LIMIT });
  const rows = useMemo(() => pullsForBranches(pulls, branches), [pulls, branches]);
  const [all, setAll] = useState(false);
  const shown = all ? rows : rows.slice(0, PULL_ROWS);
  const hidden = rows.length - shown.length;

  const askForPull = () => {
    if (!cwd) return;
    startNewThread(cwd);
    setDraft(
      "new",
      `Open a pull request${branch ? ` from ${branch}` : ""}: write up what changed, push the branch and open the PR.`,
    );
  };

  const actions = (
    <>
      <SectionButton label="Open a pull request" onClick={askForPull} disabled={!cwd}>
        <Plus size={13} />
      </SectionButton>
      <DropdownMenu.Root modal={false}>
        <DropdownMenu.Trigger asChild>
          <button type="button" aria-label="More pull request actions" className={sectionIconButton}>
            <MoreHorizontal size={13} />
          </button>
        </DropdownMenu.Trigger>
        <MenuContent align="end">
          <DropdownMenu.Item className={menuItem} onSelect={() => showPanel("pulls")}>
            All pull requests
          </DropdownMenu.Item>
          <DropdownMenu.Separator className={menuSeparator} />
          <DropdownMenu.Item
            className={menuItem}
            disabled={!cwd}
            onSelect={() => cwd && void readPullList(cwd, "all", { limit: PULL_LIMIT, force: true })}
          >
            Reload
          </DropdownMenu.Item>
        </MenuContent>
      </DropdownMenu.Root>
    </>
  );

  return (
    <OverviewSection
      id="pulls"
      title="Pull requests"
      count={rows.length}
      actions={actions}
      folded={folded}
      onToggle={onToggle}
    >
      {shown.map((pull) => (
        <PullRow
          key={pull.number}
          pull={pull}
          onOpen={() => (pull.url ? openExternal(pull.url) : showPanel("pulls"))}
        />
      ))}
      {hidden > 0 && (
        <Pill quiet size="sm" className="mt-0.5 ml-2" onClick={() => setAll(true)}>
          Show {hidden} more
        </Pill>
      )}
      {rows.length === 0 && (
        <p className="px-2 py-1 text-callout text-fg-muted">
          {error
            ? pullErrorTitle(error.code)
            : branches.length
              ? "No pull request for this branch yet."
              : "This thread has no branch yet."}
        </p>
      )}
    </OverviewSection>
  );
}
