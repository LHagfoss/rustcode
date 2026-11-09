// The overview's first section: what this thread is and where it works
// (ui/ThreadDetails.tsx draws every section). A label/value list with a mark
// for each fact — the status it is in, the agent and model it runs on, the
// project and branch it works in, the effort and access it was given — and the
// rows that do something are the ones with an act behind them: the project
// opens the file manager, the branch copies itself.

import { Copy, Folder, Gauge, GitBranch, ShieldCheck, UserRound } from "lucide-react";
import { type ReactNode, useMemo } from "react";
import { type ConfigOption, effortLabel, modelName, projectName } from "../acp/types";
import { revealPath } from "../lib/native";
import { selectOption } from "../state/harness";
import { modeLabel } from "../state/permissions";
import { type AppState, permissionModeOf, statusOf, useApp } from "../state/store";
import type { ThreadState } from "../state/thread";
import { HarnessIcon } from "./HarnessIcon";
import { OverviewSection } from "./OverviewSection";
import { copyText, cx } from "./primitives";
import { StatusChip, StatusIcon } from "./Status";

/** One fact of the thread: a mark, the word for it, and what it is. */
function Property({
  icon,
  label,
  value,
  muted,
  title,
  onClick,
  trailing,
}: {
  icon?: ReactNode;
  label: string;
  value: ReactNode;
  /** A value the thread inherited rather than chose: it reads quieter than a chosen one. */
  muted?: boolean;
  title?: string;
  onClick?: () => void;
  trailing?: ReactNode;
}) {
  const body = (
    <>
      <span className="flex size-4 shrink-0 items-center justify-center text-fg-muted">{icon}</span>
      <span className="shrink-0 text-fg-muted">{label}</span>
      <span className={cx("min-w-0 flex-1 truncate text-right", muted ? "text-fg-faint" : "text-fg")}>
        {value}
      </span>
      {trailing}
    </>
  );
  if (!onClick) {
    return (
      <div title={title} className="flex h-8 w-full items-center gap-2 px-2 text-body">
        {body}
      </div>
    );
  }
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      className="flex h-8 w-full items-center gap-2 rounded-md px-2 text-body hover:bg-fill-subtle"
    >
      {body}
    </button>
  );
}

/** A fact under the fact it belongs to: `Based on` under the branch. */
function SubFact({ label, value, title }: { label: string; value: ReactNode; title?: string }) {
  return (
    <div title={title} className="flex h-8 w-full items-center gap-2 pr-2 pl-8 text-body">
      <span className="shrink-0 text-fg-muted">{label}</span>
      <span className="min-w-0 flex-1 truncate text-right font-mono text-caption text-fg-faint">{value}</span>
    </div>
  );
}

/** The model and effort the thread reported, named the way the composer names them. */
function choiceOf(options: ConfigOption[], id: string) {
  const option = selectOption(options, id);
  const value = option?.current;
  const name = option?.choices.find((choice) => choice.value === value)?.name;
  return { value, name: name && name !== value ? name : undefined };
}

/** The named account a thread runs on, in the words Settings › Harnesses gives it. */
function accountName(s: AppState, thread: ThreadState): string | undefined {
  const id = thread.harness?.account;
  if (!id) return undefined;
  const accounts = s.harnesses?.harnesses.find((h) => h.id === thread.harness?.id)?.accounts ?? [];
  return accounts.find((a) => a.id === id)?.name ?? id;
}

export function PropertiesSection({
  thread,
  branch,
  project,
  folded,
  onToggle,
}: {
  thread: ThreadState;
  /** The branch it works on, from its worktree or the folder's own checkout. */
  branch?: string;
  /** The project the thread belongs to: its worktree's repository, else its folder. */
  project: string;
  folded: boolean;
  onToggle: () => void;
}) {
  const status = useApp((s) => statusOf(s, thread.id));
  const harnessName = useApp((s) =>
    thread.harness
      ? (s.harnesses?.harnesses.find((h) => h.id === thread.harness?.id)?.name ?? thread.harness.id)
      : undefined,
  );
  const account = useApp((s) => accountName(s, thread));
  const mode = useApp((s) => permissionModeOf(s, thread.id));
  // A mode the user never chose is the app's default: the row says where it came from.
  const inheritedAccess = useApp((s) => s.permissionModes[thread.id] === undefined);
  const picked = useMemo(() => choiceOf(thread.configOptions, "model"), [thread.configOptions]);
  const pickedEffort = useMemo(() => choiceOf(thread.configOptions, "effort"), [thread.configOptions]);
  const model = picked.name ?? modelName(picked.value);
  const effort = pickedEffort.name ?? (pickedEffort.value ? effortLabel(pickedEffort.value) : undefined);
  const base = thread.worktree?.base;

  return (
    <OverviewSection id="properties" title="Properties" folded={folded} onToggle={onToggle}>
      <Property
        icon={<StatusIcon status={status} size={14} />}
        label="Status"
        value={
          status === "idle" ? (
            <span className="text-fg-faint">Idle</span>
          ) : (
            <span className="inline-flex items-center">
              <StatusChip status={status} />
            </span>
          )
        }
      />
      {thread.harness && (
        <Property
          icon={<HarnessIcon id={thread.harness.id} name={harnessName} size={14} />}
          label="Agent"
          title={[harnessName, model].filter(Boolean).join(" · ") || undefined}
          value={
            <span className="flex items-center justify-end gap-1.5">
              {model && <span className="truncate">{model}</span>}
              {model && harnessName && <span className="text-fg-faint">·</span>}
              {harnessName && <span className="truncate text-fg-muted">{harnessName}</span>}
            </span>
          }
        />
      )}
      {account && <Property icon={<UserRound size={14} />} label="Account" value={account} />}
      <Property
        icon={<Folder size={14} />}
        label="Project"
        value={projectName(project)}
        title={`Show ${thread.cwd} in the file manager`}
        onClick={() => revealPath(thread.cwd)}
      />
      {branch && (
        <Property
          icon={<GitBranch size={14} />}
          label="Branch"
          value={<span className="font-mono text-callout">{branch}</span>}
          title="Copy the branch name"
          onClick={() => void copyText(branch)}
          trailing={<Copy size={12} className="shrink-0 text-fg-faint" />}
        />
      )}
      {base && <SubFact label="Based on" value={base} title={thread.worktree?.baseRef ?? base} />}
      <Property
        icon={<Gauge size={14} />}
        label="Effort"
        // A thread that sets none runs on the harness's own default, which reads quieter.
        muted={!effort}
        value={effort ?? "Default"}
        title={effort ? undefined : "This thread sets no effort: its harness chooses."}
      />
      <Property
        icon={<ShieldCheck size={14} />}
        label="Access"
        title={
          inheritedAccess
            ? "This thread keeps the default access from Settings › General."
            : "Chosen for this thread, under its composer."
        }
        muted={inheritedAccess}
        value={inheritedAccess ? `${modeLabel(mode)} (default)` : modeLabel(mode)}
      />
    </OverviewSection>
  );
}
