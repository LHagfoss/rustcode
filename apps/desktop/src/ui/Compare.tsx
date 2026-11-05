// Fan-out compare: one prompt run on several harnesses, each in its own
// worktree thread, side by side: harness, status, files changed and the
// diff stat of its branch, its answer, and Open / Review. "Keep this" puts
// the chosen thread up for review and marks the others done.

import { Check, ExternalLink, FileDiff, GitBranch } from "lucide-react";
import { useEffect, useState } from "react";
import { gitChanges } from "../lib/native";
import { keepFanOut, stateOf } from "../state/host";
import { accountLabelOf } from "../state/logins";
import { LINK_LABEL, type Link } from "../state/orchestration";
import { selectThread, setView, statusOf, useApp, worktreeOf } from "../state/store";
import { ViewHeader } from "./Board";
import { Markdown } from "./Markdown";
import { Pill, Spinner } from "./primitives";
import { DiffCount } from "./Rows";
import { StatusChip } from "./Status";

function useBranchStat(threadId: string, running: boolean) {
  const wt = useApp((s) => worktreeOf(s, threadId));
  const cwd = useApp((s) => s.threads[threadId]?.cwd);
  const [stat, setStat] = useState<{ files: number; added: number; removed: number } | null>(null);
  useEffect(() => {
    if (running) return;
    const folder = wt?.path ?? cwd;
    if (!folder) return;
    let live = true;
    void gitChanges(folder, wt?.base)
      .then((c) => {
        if (!live || !c) return;
        setStat({
          files: c.files.length,
          added: c.files.reduce((n, f) => n + f.added, 0),
          removed: c.files.reduce((n, f) => n + f.removed, 0),
        });
      })
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [wt?.path, wt?.base, cwd, running]);
  return stat;
}

function CompareCard({ link, fanOut, kept }: { link: Link; fanOut: string; kept?: string }) {
  const state = useApp((s) => stateOf(s, link));
  const status = useApp((s) => statusOf(s, link.target));
  const harness = useApp(
    (s) => s.harnesses?.harnesses.find((h) => h.id === link.harness)?.name ?? link.harness ?? "RustCode",
  );
  // Two logins of one harness read apart here too, not only on the approval card.
  const account = useApp((s) =>
    accountLabelOf(
      s.harnesses?.harnesses.find((h) => h.id === link.harness),
      link.account,
    ),
  );
  const wt = useApp((s) => worktreeOf(s, link.target));
  const running = state === "working" || state === "queued";
  const stat = useBranchStat(link.target, running);
  const isKept = kept === link.target;
  return (
    <article
      className={`flex min-w-[260px] flex-1 flex-col self-start gap-2.5 rounded-xl border bg-fill-subtle p-3.5 transition-[border-color,box-shadow] duration-150 rc-in ${
        isKept ? "border-accent shadow-[0_0_0_1px_var(--rc-accent)]" : "border-border"
      }`}
    >
      <header className="flex items-center gap-2">
        <span className="font-semibold text-body">{harness}</span>
        {account && <span className="truncate text-caption text-fg-muted">{account}</span>}
        <StatusChip status={status} />
        <span className="flex-1" />
        <span className="flex items-center gap-1 text-caption text-fg-muted">
          {running && <Spinner size={10} />}
          {LINK_LABEL[state]}
        </span>
      </header>
      {wt && (
        <span className="flex items-center gap-1 truncate font-mono text-caption text-fg-muted">
          <GitBranch size={11} className="shrink-0" />
          {wt.branch}
        </span>
      )}
      <div className="flex items-center gap-2 text-callout">
        <FileDiff size={13} className="text-fg-muted" />
        {stat ? (
          <>
            <span>
              {stat.files} file{stat.files === 1 ? "" : "s"} changed
            </span>
            <DiffCount added={stat.added} removed={stat.removed} />
          </>
        ) : link.filesChanged !== undefined ? (
          <span>
            {link.filesChanged} file{link.filesChanged === 1 ? "" : "s"} changed
          </span>
        ) : (
          <span className="text-fg-faint">{running ? "Working…" : "No changes read yet"}</span>
        )}
      </div>
      <div className="max-h-[320px] min-h-[80px] flex-1 overflow-y-auto rounded-lg bg-fill-subtle px-3 py-2">
        {link.answer ? (
          <Markdown text={link.answer} className="text-callout" />
        ) : (
          <span className="text-callout text-fg-faint">
            {running ? "The answer shows when the turn ends." : "No answer."}
          </span>
        )}
      </div>
      <footer className="flex flex-wrap gap-1.5">
        <Pill onClick={() => selectThread(link.target)}>
          <ExternalLink size={12} /> Open
        </Pill>
        <Pill onClick={() => setView({ kind: "review", threadId: link.target })}>
          <FileDiff size={12} /> Review
        </Pill>
        <span className="flex-1" />
        <Pill
          prominent={!isKept}
          disabled={running || isKept}
          onClick={() => void keepFanOut(fanOut, link.target)}
        >
          <Check size={12} /> {isKept ? "Kept" : "Keep this"}
        </Pill>
      </footer>
    </article>
  );
}

export function Compare({ fanOut }: { fanOut: string }) {
  const group = useApp((s) => s.links.fanOuts.find((f) => f.id === fanOut));
  const links = useApp((s) => s.links.links);
  if (!group) {
    return (
      <main className="flex min-w-0 flex-1 flex-col bg-bg">
        <ViewHeader title="Compare" />
        <p className="px-6 text-callout text-fg-muted">This fan-out is no longer kept.</p>
      </main>
    );
  }
  const shown = group.links.flatMap((id) => links.find((l) => l.id === id) ?? []);
  return (
    <main className="flex min-w-0 flex-1 flex-col bg-bg" aria-label="Compare">
      <ViewHeader title={`Compare · ${group.title}`}>
        <Pill onClick={() => selectThread(group.orchestrator)}>Back to the thread</Pill>
      </ViewHeader>
      <p className="selectable mx-4 mb-3 line-clamp-2 text-callout text-fg-muted">{group.prompt}</p>
      <div className="flex min-h-0 flex-1 gap-3 overflow-x-auto px-4 pb-4">
        {shown.map((l) => (
          <CompareCard key={l.id} link={l} fanOut={group.id} kept={group.kept} />
        ))}
      </div>
    </main>
  );
}
