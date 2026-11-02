// The "Changes" chip under a finished turn: what that turn changed, from its
// checkpoint (`_rustcode/checkpoints/diff`), and Restore, which puts the
// files back to before the turn (and the conversation too, when the
// harness can rewind).

import { FileDiff, History } from "lucide-react";
import { Popover } from "radix-ui";
import { useState } from "react";
import type { Checkpoint, PatchFile } from "../acp/v2";
import { parsePatch } from "../state/diff";
import { askConfirm, canRewind, restoreCheckpoint, setView, turnDiff, useApp } from "../state/store";
import { cx, Pill, Spinner } from "./primitives";
import { DiffCount } from "./Rows";

function Patch({ file }: { file: PatchFile }) {
  const lines = parsePatch(file.patch);
  return (
    <div className="selectable overflow-x-auto rounded-lg bg-fill-3 py-1 font-mono text-caption leading-[16px]">
      <div className="min-w-fit">
        {lines.map((l, i) => (
          <div
            key={i}
            className={cx(
              "flex pr-3",
              l.kind === "add" && "bg-[var(--rc-diff-add)]",
              l.kind === "del" && "bg-[var(--rc-diff-del)]",
              l.kind === "hunk" && "text-fg-3",
            )}
          >
            {l.kind === "hunk" ? (
              <span className="px-2.5">{l.text}</span>
            ) : (
              <>
                <span className="w-[5ch] shrink-0 pr-1.5 text-right text-fg-3 tabular">
                  {(l.kind === "del" ? l.old : l.new) ?? ""}
                </span>
                <span className={cx("w-3 shrink-0", l.kind === "add" ? "text-success" : "text-danger")}>
                  {l.kind === "add" ? "+" : l.kind === "del" ? "−" : " "}
                </span>
                <span className="whitespace-pre">{l.text || " "}</span>
              </>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

export function TurnChanges({ threadId, checkpoint }: { threadId: string; checkpoint: Checkpoint }) {
  const [files, setFiles] = useState<PatchFile[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const thread = useApp((s) => s.threads[threadId]);
  const running = thread?.running ?? false;
  const rewind = thread ? canRewind(thread) : false;
  const load = () => {
    if (files) return;
    turnDiff(threadId, checkpoint.turn).then(setFiles, (e) =>
      setError(e instanceof Error ? e.message : String(e)),
    );
  };
  const restore = () =>
    askConfirm({
      title: `Restore the files to before turn ${checkpoint.turn}?`,
      message: rewind
        ? "The files go back to how they were before this turn. You can take the conversation back too; later messages and their replies are then removed."
        : "The files go back to how they were before this turn. This harness cannot rewind, so the conversation stays as it is.",
      action: "Files Only",
      destructive: true,
      run: () => void restoreCheckpoint(threadId, checkpoint.turn, false),
      alt: rewind
        ? {
            action: "Files and Conversation",
            run: () => void restoreCheckpoint(threadId, checkpoint.turn, true),
          }
        : undefined,
    });
  if (!checkpoint.filesChanged) return null;
  return (
    <Popover.Root onOpenChange={(open) => open && load()}>
      <Popover.Trigger asChild>
        <button
          type="button"
          className="flex h-[20px] items-center gap-1 rounded-full bg-fill-3 px-2 font-medium text-caption text-fg-2 hover:bg-fill-2 hover:text-fg data-[state=open]:bg-fill-1"
        >
          <FileDiff size={11} />
          Changes
          <DiffCount added={checkpoint.insertions} removed={checkpoint.deletions} />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          side="top"
          align="start"
          sideOffset={6}
          className="surface z-40 flex max-h-[min(520px,70vh)] w-[min(640px,80vw)] flex-col gap-2 rounded-[18px] border border-hairline p-3"
        >
          <div className="flex items-center gap-2 px-1">
            <span className="font-semibold text-body">Turn {checkpoint.turn}</span>
            <span className="text-callout text-fg-2">
              {checkpoint.filesChanged} file{checkpoint.filesChanged === 1 ? "" : "s"}
            </span>
            <span className="flex-1" />
            <Pill onClick={() => setView({ kind: "review", threadId })}>Review…</Pill>
            <Pill disabled={running} onClick={restore} title={running ? "Stop the turn first" : undefined}>
              <History size={12} /> Restore…
            </Pill>
          </div>
          <div className="flex min-h-0 flex-col gap-3 overflow-y-auto">
            {error && <p className="px-1 text-callout text-fg-2">{error}</p>}
            {!files && !error && (
              <div className="flex items-center gap-2 px-1 text-callout text-fg-2">
                <Spinner /> Reading the turn's changes…
              </div>
            )}
            {files?.map((f) => (
              <div key={f.path}>
                <div className="mb-1 flex items-center gap-1.5 text-caption">
                  <span className="font-bold font-mono text-[9px] text-fg-2 uppercase">
                    {f.status.slice(0, 1)}
                  </span>
                  <span className="truncate font-mono">{f.path}</span>
                  <DiffCount added={f.insertions} removed={f.deletions} />
                </div>
                <Patch file={f} />
              </div>
            ))}
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
